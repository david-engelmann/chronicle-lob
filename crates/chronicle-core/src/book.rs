//! The book state machine (ARCHITECTURE.md section 7).
//!
//! Every mutating command runs validation, then preflight, then
//! execution. Validation and preflight never mutate, so every rejection
//! is atomic (INV-ORD-4). Execution cannot fail: all of its failure
//! modes were excluded by the first two phases.
//!
//! Milestone 1 executed Limit orders (add and cancel), best price and
//! L2 depth queries, and the structural invariant checker. Milestone 2
//! executes the full command surface behind these same phases: `Ioc`,
//! `Fok`, and `PostOnly` adds, amend, cancel-by-participant, and self-trade
//! prevention in the match loop.

use crate::arena::{Arena, NIL};
use crate::bitmap::Bitmap;
use crate::config::BookConfig;
use crate::error::BookError;
use crate::events::{CancelReason, EventBuffer, ExecutionEvent};
use crate::level::Levels;
use crate::lookup::Lookup;
use crate::types::{OrderId, OrderType, ParticipantId, Side, StpPolicy};

/// One aggregated price level in an L2 depth snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthLevel {
    /// Raw price in quote units.
    pub price: u64,
    /// Sum of remaining quantities resting at this price.
    pub total_qty: u64,
    /// Number of live orders at this price.
    pub order_count: u32,
}

/// A single-instrument limit order book on one tick grid.
pub struct Book {
    /// The validated configuration this book was built with.
    config: BookConfig,
    /// One past the last tick on the grid, computed once at
    /// construction from the validated config.
    grid_end_tick: u64,
    /// Order storage.
    arena: Arena,
    /// `OrderId` to (slot, generation) index.
    lookup: Lookup,
    /// Per-tick level aggregates and list ends.
    levels: Levels,
    /// Active bid levels.
    bids: Bitmap,
    /// Active ask levels.
    asks: Bitmap,
    /// Sequence number the next accepted command will take. Starts at
    /// 1; rejected commands consume nothing.
    next_seq: u64,
}

impl Book {
    /// Build an empty book.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::InvalidConfig`] when the configuration
    /// fails any rule in ARCHITECTURE.md section 3.2.
    pub fn new(config: BookConfig) -> Result<Self, BookError> {
        config.validate()?;
        let grid_end_tick = config.grid_end_tick()?;
        Ok(Self {
            config,
            grid_end_tick,
            arena: Arena::new(config.max_orders),
            lookup: Lookup::new(config.max_orders),
            levels: Levels::new(config.grid_len),
            bids: Bitmap::new(config.grid_len),
            asks: Bitmap::new(config.grid_len),
            next_seq: 1,
        })
    }

    /// The configuration this book was built with.
    #[must_use]
    pub const fn config(&self) -> BookConfig {
        self.config
    }

    /// Number of live orders.
    #[must_use]
    pub const fn live_order_count(&self) -> u32 {
        self.arena.live_count()
    }

    /// Sequence number the next accepted command will take.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Best bid as `(price, aggregate qty)`, or `None` when no bid
    /// rests.
    #[must_use]
    pub fn best_bid(&self) -> Option<(u64, u64)> {
        self.bids
            .highest()
            .map(|idx| (self.price_of_idx(idx), self.levels.qty[idx as usize]))
    }

    /// Best ask as `(price, aggregate qty)`, or `None` when no ask
    /// rests.
    #[must_use]
    pub fn best_ask(&self) -> Option<(u64, u64)> {
        self.asks
            .lowest()
            .map(|idx| (self.price_of_idx(idx), self.levels.qty[idx as usize]))
    }

    /// Add an order. The order sweeps the opposite side in price-time
    /// priority under its self-trade prevention policy (`stp`, falling
    /// back to the book default when `None`), then the order type
    /// decides the remainder: `Limit` rests it, `Ioc` expires it, `Fok` was
    /// already proven fillable in preflight, and `PostOnly` rests in full
    /// because preflight proved it does not cross.
    ///
    /// Returns the number of events written into `events`.
    ///
    /// # Errors
    ///
    /// Returns a [`BookError`] before mutating anything when validation
    /// or preflight fails: invalid or duplicate order id, zero
    /// quantity, invalid price tick, capacity, a `PostOnly` price that
    /// would cross, a `Fok` quantity the book cannot fill, or an event
    /// buffer below the exact preflight bound.
    #[allow(
        clippy::too_many_arguments,
        reason = "the signature is fixed verbatim by ARCHITECTURE.md section 7.1"
    )]
    pub fn add_order(
        &mut self,
        order_id: OrderId,
        participant: ParticipantId,
        side: Side,
        order_type: OrderType,
        price: u64,
        qty: u64,
        stp: Option<StpPolicy>,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        // Phase 1, validation. No mutation.
        if order_id.0 == 0 {
            return Err(BookError::InvalidOrderId { order_id });
        }
        if qty == 0 {
            return Err(BookError::InvalidQuantity { qty });
        }
        let price_idx = self.price_to_idx(price)?;
        if self.lookup.get(order_id.0).is_some() {
            return Err(BookError::DuplicateOrderId { order_id });
        }
        let effective_stp = stp.unwrap_or(self.config.default_stp);
        self.execute_add(
            order_id,
            participant,
            side,
            order_type,
            price,
            price_idx,
            qty,
            effective_stp,
            events,
        )
    }

    /// Cancel a live order by id.
    ///
    /// Returns the number of events written into `events` (one
    /// `Cancelled` event on success).
    ///
    /// # Errors
    ///
    /// Returns [`BookError::OrderNotFound`] with no side effects when
    /// the id is missing, filled, or already cancelled, and
    /// [`BookError::EventBufferFull`] when the buffer has no free slot.
    pub fn cancel_order(
        &mut self,
        order_id: OrderId,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        let (slot, generation) = self
            .lookup
            .get(order_id.0)
            .ok_or(BookError::OrderNotFound { order_id })?;
        if events.remaining() < 1 {
            return Err(BookError::EventBufferFull {
                needed: 1,
                available: events.remaining(),
            });
        }
        let seq = self.take_seq()?;
        self.remove_live_order(slot, generation, CancelReason::Requested, seq, events);
        Ok(1)
    }

    /// Cancel every live order belonging to one participant
    /// (ARCHITECTURE.md section 7.8). Events are emitted in ascending
    /// slot order, one `Cancelled` with reason
    /// [`CancelReason::ByParticipant`] per order. A participant with no
    /// live orders succeeds with zero events; the command still
    /// consumes a sequence number, as every accepted command does.
    ///
    /// Returns the number of events written into `events`.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::EventBufferFull`] before mutating anything
    /// when the buffer holds fewer free slots than the participant has
    /// live orders.
    pub fn cancel_by_participant(
        &mut self,
        participant: ParticipantId,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        // Phase 1 and 2, preflight count (no mutation): the exact
        // event bound is the participant's live order count.
        let needed = self
            .arena
            .state
            .iter()
            .zip(self.arena.participant.iter())
            .filter(|(state, owner)| {
                **state == crate::arena::STATE_LIVE && **owner == participant.0
            })
            .count();
        if events.remaining() < needed {
            return Err(BookError::EventBufferFull {
                needed,
                available: events.remaining(),
            });
        }
        // Phase 3, execution: ascending slot scan. Detaching the slot
        // being visited cannot disturb the slots not yet visited, so
        // the scan needs no scratch space.
        let seq = self.take_seq()?;
        let mut written = 0;
        for slot in 0..self.config.max_orders {
            let s = slot as usize;
            if self.arena.state[s] == crate::arena::STATE_LIVE
                && self.arena.participant[s] == participant.0
            {
                let generation = self.arena.generation[s];
                self.remove_live_order(slot, generation, CancelReason::ByParticipant, seq, events);
                written += 1;
            }
        }
        debug_assert_eq!(written, needed, "preflight count and scan disagree");
        Ok(written)
    }

    /// Amend a live order to a new price and a new remaining quantity
    /// (ARCHITECTURE.md section 7.4). A smaller quantity at the same
    /// price, or an identical price and quantity, preserves the order's
    /// exact FIFO position; a larger quantity or any price change moves
    /// it to the tail of the target level. An amend never triggers a
    /// match. Every accepted amend emits one `Amended` event and
    /// consumes one sequence number, including the identical no-op.
    ///
    /// Returns the number of events written into `events` (one on
    /// success).
    ///
    /// # Errors
    ///
    /// Returns [`BookError::OrderNotFound`] when the id is not live,
    /// [`BookError::InvalidPriceTick`] or [`BookError::InvalidQuantity`]
    /// for an off-grid price or a zero quantity,
    /// [`BookError::AmendWouldCross`] when the new price would leave
    /// the book crossed, and [`BookError::EventBufferFull`] when the
    /// buffer has no free slot. Every error is returned before any
    /// mutation.
    pub fn amend_order(
        &mut self,
        order_id: OrderId,
        new_price: u64,
        new_qty: u64,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        // Phase 1, validation. No mutation.
        let (slot, _generation) = self
            .lookup
            .get(order_id.0)
            .ok_or(BookError::OrderNotFound { order_id })?;
        let new_idx = self.price_to_idx(new_price)?;
        if new_qty == 0 {
            return Err(BookError::InvalidQuantity { qty: new_qty });
        }
        let s = slot as usize;
        let old_idx = self.arena.price_idx[s];
        let old_qty = self.arena.qty[s];
        let side = Side::from_u8(self.arena.side[s]);
        // Phase 2, preflight. The crossing test comes first (an amend
        // that would cross writes no events), then the one-event
        // buffer bound, then the aggregate check on the target level.
        if let Some(opposite_idx) = self.best_opposite_idx(side)
            && crosses(side, new_idx, opposite_idx)
        {
            return Err(BookError::AmendWouldCross {
                order_id,
                new_price,
                opposite_best: self.price_of_idx(opposite_idx),
            });
        }
        if events.remaining() < 1 {
            return Err(BookError::EventBufferFull {
                needed: 1,
                available: events.remaining(),
            });
        }
        let target_base = self.levels.qty[new_idx as usize]
            - if new_idx == old_idx { old_qty } else { 0 };
        if target_base.checked_add(new_qty).is_none() {
            return Err(BookError::ArithmeticOverflow {
                what: "price level quantity",
            });
        }
        // Phase 3, execution.
        let seq = self.take_seq()?;
        if new_idx == old_idx && new_qty == old_qty {
            // Identical amend: a seq-consuming no-op (section 7.4).
        } else if new_idx == old_idx && new_qty < old_qty {
            // Reduce in place: position and priority are untouched.
            let diff = old_qty - new_qty;
            self.arena.qty[s] -= diff;
            self.levels.qty[old_idx as usize] -= diff;
        } else {
            // Increase or reprice: the order loses its priority and
            // rejoins at the tail of the target level.
            self.unlink_from_level(slot);
            self.arena.price_idx[s] = new_idx;
            self.arena.qty[s] = new_qty;
            self.link_at_tail(slot, side);
        }
        events.emit(ExecutionEvent::Amended {
            seq,
            order_id,
            price: new_price,
            qty: new_qty,
        });
        Ok(1)
    }

    /// Write up to `max_levels` aggregated levels for one side into
    /// `out`, best price first, and return how many were written.
    /// Mutates nothing and allocates nothing.
    pub fn depth(&self, side: Side, max_levels: usize, out: &mut [DepthLevel]) -> usize {
        let limit = max_levels.min(out.len());
        let bitmap = match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        };
        let mut written = 0;
        let mut cursor = match side {
            Side::Bid => bitmap.highest(),
            Side::Ask => bitmap.lowest(),
        };
        while written < limit {
            let Some(idx) = cursor else { break };
            out[written] = DepthLevel {
                price: self.price_of_idx(idx),
                total_qty: self.levels.qty[idx as usize],
                order_count: self.levels.count[idx as usize],
            };
            written += 1;
            cursor = match side {
                Side::Bid => bitmap.highest_below(idx),
                Side::Ask => bitmap.lowest_above(idx),
            };
        }
        written
    }

    /// Full structural self-check (ARCHITECTURE.md section 11): crossed
    /// book, per-level aggregates recomputed from the arena, bitmap
    /// summaries recomputed from level counts, lookup reachability and
    /// generation agreement for every live order, free list exactness,
    /// and level list link symmetry. Intended for tests and debug
    /// builds; it is O(capacity + grid).
    ///
    /// # Panics
    ///
    /// Panics on the first invariant violation found, with a message
    /// naming the broken invariant and the offending slot or level.
    /// That is the intended behaviour: this is a checker, and a failed
    /// check must never be mistaken for a passing one.
    pub fn debug_assert_invariants(&self) {
        // Crossed book (INV-BOOK-1).
        if let (Some((bid_price, _)), Some((ask_price, _))) = (self.best_bid(), self.best_ask()) {
            assert!(
                bid_price < ask_price,
                "book is crossed: bid {bid_price} >= ask {ask_price}"
            );
        }
        let seen_live = self.assert_levels();
        // Bitmap summaries (INV-BITMAP-1) for both sides.
        Self::assert_bitmap_summary(&self.bids);
        Self::assert_bitmap_summary(&self.asks);
        assert_eq!(self.bids.is_empty(), self.best_bid().is_none());
        assert_eq!(self.asks.is_empty(), self.best_ask().is_none());
        self.assert_free_list(&seen_live);
    }

    /// Per-level structure, aggregates, and lookup agreement. Returns
    /// the set of slots seen live.
    fn assert_levels(&self) -> Vec<bool> {
        let mut seen_live = vec![false; self.config.max_orders as usize];
        let mut total_live: u32 = 0;
        assert_eq!(self.arena.capacity(), self.config.max_orders);
        for idx in 0..self.config.grid_len {
            let i = idx as usize;
            let mut count: u32 = 0;
            let mut sum: u64 = 0;
            let mut prev = NIL;
            let mut slot = self.levels.head[i];
            let mut side_seen: Option<Side> = None;
            while slot != NIL {
                let s = slot as usize;
                assert_eq!(
                    self.arena.state[s],
                    crate::arena::STATE_LIVE,
                    "slot {slot} in a level list is not live"
                );
                assert_eq!(
                    self.arena.price_idx[s], idx,
                    "slot {slot} sits at the wrong level"
                );
                assert_eq!(
                    self.arena.prev[s], prev,
                    "slot {slot} prev link is asymmetric"
                );
                let side = Side::from_u8(self.arena.side[s]);
                match side_seen {
                    None => side_seen = Some(side),
                    Some(existing) => {
                        assert_eq!(existing, side, "level {idx} mixes sides");
                    }
                }
                assert!(!seen_live[s], "slot {slot} appears in two level lists");
                seen_live[s] = true;
                assert_eq!(
                    self.lookup.get(self.arena.order_id[s]),
                    Some((slot, self.arena.generation[s])),
                    "lookup disagrees with arena for slot {slot}"
                );
                assert!(self.arena.validate_handle(slot, self.arena.generation[s]));
                count += 1;
                sum = sum
                    .checked_add(self.arena.qty[s])
                    .expect("level quantity overflow in checker");
                prev = slot;
                slot = self.arena.next[s];
            }
            assert_eq!(
                self.levels.count[i], count,
                "level {idx} cached count is wrong"
            );
            assert_eq!(
                self.levels.qty[i], sum,
                "level {idx} cached quantity is wrong"
            );
            assert_eq!(
                self.levels.tail[i], prev,
                "level {idx} cached tail is wrong"
            );
            // Leaf bitmap bits agree with which side rests at the level.
            assert_eq!(
                self.bids.contains(idx),
                side_seen == Some(Side::Bid),
                "bid bitmap bit wrong at {idx}"
            );
            assert_eq!(
                self.asks.contains(idx),
                side_seen == Some(Side::Ask),
                "ask bitmap bit wrong at {idx}"
            );
            total_live += count;
        }
        assert_eq!(
            total_live,
            self.arena.live_count(),
            "arena live count disagrees with levels"
        );
        assert_eq!(
            self.lookup.len(),
            total_live as usize,
            "lookup length disagrees with live count"
        );
        seen_live
    }

    /// Free list exactness (INV-ARENA-1): the chain from the head
    /// visits exactly the free slots, each once, no cycles; every
    /// other slot is live (seen by `assert_levels`) or retired.
    fn assert_free_list(&self, seen_live: &[bool]) {
        let mut free_walked: u32 = 0;
        let mut slot = self.arena.free_head();
        let mut free_seen = vec![false; self.config.max_orders as usize];
        while slot != NIL {
            let s = slot as usize;
            assert_eq!(
                self.arena.state[s],
                crate::arena::STATE_FREE,
                "slot {slot} on the free list is not free"
            );
            assert!(!free_seen[s], "free list cycle at slot {slot}");
            free_seen[s] = true;
            free_walked += 1;
            slot = self.arena.next[s];
        }
        assert_eq!(
            free_walked,
            self.arena.free_count(),
            "free list length disagrees with arena"
        );
        for s in 0..self.config.max_orders as usize {
            if !seen_live[s] && !free_seen[s] {
                assert_eq!(
                    self.arena.state[s],
                    crate::arena::STATE_RETIRED,
                    "slot {s} is neither live, free, nor retired"
                );
            }
        }
    }

    /// Recompute one bitmap hierarchy from its leaf words and compare
    /// every summary bit (INV-BITMAP-1).
    fn assert_bitmap_summary(bitmap: &Bitmap) {
        for level in 1..bitmap.depth() {
            for w in 0..bitmap.words_at(level) {
                let word = bitmap.word(level, w);
                for bit in 0..64usize {
                    let child = w * 64 + bit;
                    let expect = if child < bitmap.words_at(level - 1) {
                        u64::from(bitmap.word(level - 1, child) != 0)
                    } else {
                        0
                    };
                    assert_eq!(
                        (word >> bit) & 1,
                        expect,
                        "bitmap summary mismatch at level {level} word {w} bit {bit}"
                    );
                }
            }
        }
    }

    /// Convert a raw price to its dense tick index.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::InvalidPriceTick`] on a zero price, an
    /// unaligned price, or a tick outside the grid.
    pub(crate) fn price_to_idx(&self, price: u64) -> Result<u32, BookError> {
        let err = || BookError::InvalidPriceTick {
            price,
            tick_size: self.config.tick_size,
            grid_start_tick: self.config.grid_start_tick,
            grid_len: self.config.grid_len,
        };
        if price == 0 || !price.is_multiple_of(self.config.tick_size) {
            return Err(err());
        }
        let tick = price / self.config.tick_size;
        if tick < self.config.grid_start_tick || tick >= self.grid_end_tick {
            return Err(err());
        }
        Ok(u32::try_from(tick - self.config.grid_start_tick).unwrap_or(0))
    }

    /// Convert a dense tick index back to its raw price.
    pub(crate) fn price_of_idx(&self, idx: u32) -> u64 {
        self.config
            .price_of_tick(self.config.grid_start_tick + u64::from(idx))
    }

    /// Take the next sequence number for an accepted command.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::ArithmeticOverflow`] at the unreachable
    /// `u64` sequence ceiling, before any mutation.
    fn take_seq(&mut self) -> Result<u64, BookError> {
        let seq = self.next_seq;
        self.next_seq = seq.checked_add(1).ok_or(BookError::ArithmeticOverflow {
            what: "sequence number",
        })?;
        Ok(seq)
    }

    /// Preflight and execute an add of any order type. Validation
    /// has already run; `stp` is the effective self-trade prevention
    /// policy (the order's own, or the book default).
    ///
    /// Preflight is exact, per ARCHITECTURE.md section 7.2 as extended
    /// by the Milestone 2 entry condition m3. A read-only simulation
    /// of the sweep (`plan_sweep`) yields the exact event count, the
    /// slots the sweep frees, and the taker remainder, so every
    /// rejection is decided before the sequence number is taken:
    ///
    /// | Command | Exact event bound |
    /// | --- | --- |
    /// | `Limit` / `Ioc` / `Fok` add | sweep events (one `Trade` per trade, one `Filled` per maker filled, one `Cancelled` per resting order STP cancels) + 1 terminal event |
    /// | `PostOnly` add | 1 (`Accepted`); the sweep never runs |
    /// | `cancel_order` | 1 |
    /// | `amend_order` | 1 |
    /// | `cancel_by_participant` | the participant's live order count |
    ///
    /// Semantic preflight rejections (`PostOnly` crossing, `Fok`
    /// shortfall) are decided before the event-buffer check, because
    /// those commands would write no events at all; then the buffer
    /// bound, then capacity and aggregate-overflow for a remainder
    /// that would rest.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the public add_order signature fixed by ARCHITECTURE.md section 7.1"
    )]
    fn execute_add(
        &mut self,
        order_id: OrderId,
        participant: ParticipantId,
        side: Side,
        order_type: OrderType,
        price: u64,
        price_idx: u32,
        qty: u64,
        stp: StpPolicy,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        if order_type == OrderType::PostOnly {
            return self.execute_post_only(order_id, participant, side, price, price_idx, qty, events);
        }
        // ---- Preflight (no mutation) ----
        let plan = self.plan_sweep(side, price_idx, qty, participant, stp);
        if order_type == OrderType::Fok && plan.remaining > 0 {
            return Err(BookError::FokInsufficientLiquidity {
                requested: qty,
                available: qty - plan.remaining,
            });
        }
        let needed = plan.events + 1;
        if events.remaining() < needed {
            return Err(BookError::EventBufferFull {
                needed,
                available: events.remaining(),
            });
        }
        let will_rest =
            order_type == OrderType::Limit && plan.remaining > 0 && !plan.stp_stopped;
        if will_rest {
            // A slot must exist once the sweep has freed the slots of
            // the makers it filled or STP-cancelled.
            if self.arena.free_count() as usize + plan.freed_slots == 0 {
                return Err(BookError::CapacityFull {
                    max_orders: self.config.max_orders,
                });
            }
            // The resting aggregate must stay inside u64 (INV-MATH-1).
            if self.levels.qty[price_idx as usize]
                .checked_add(plan.remaining)
                .is_none()
            {
                return Err(BookError::ArithmeticOverflow {
                    what: "price level quantity",
                });
            }
        }

        // ---- Execution (cannot fail; the plan proved every step) ----
        let seq = self.take_seq()?;
        let written_before = events.written().len();
        let (remaining, stp_stopped) = self.execute_sweep(
            order_id, participant, side, price_idx, qty, stp, seq, events,
        );
        debug_assert_eq!(
            remaining, plan.remaining,
            "execution diverged from the preflight plan"
        );
        debug_assert_eq!(stp_stopped, plan.stp_stopped);
        if remaining == 0 {
            events.emit(ExecutionEvent::Filled { seq, order_id });
        } else if stp_stopped {
            events.emit(ExecutionEvent::SelfTradePrevented { seq, order_id });
        } else {
            match order_type {
                OrderType::Limit => {
                    self.rest_order(order_id, participant, side, price_idx, remaining);
                    events.emit(ExecutionEvent::Accepted { seq, order_id });
                }
                OrderType::Ioc => {
                    events.emit(ExecutionEvent::Expired {
                        seq,
                        order_id,
                        remaining_qty: remaining,
                    });
                }
                OrderType::Fok | OrderType::PostOnly => {
                    unreachable!("Fok remainders rejected in preflight; PostOnly never sweeps")
                }
            }
        }
        Ok(events.written().len() - written_before)
    }

    /// Run the sweep the preflight plan proved out, emitting sweep
    /// events in order (section 7.6). Returns the taker remainder and
    /// whether STP stopped the sweep at an own resting order.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the public add_order signature fixed by ARCHITECTURE.md section 7.1"
    )]
    fn execute_sweep(
        &mut self,
        order_id: OrderId,
        participant: ParticipantId,
        side: Side,
        price_idx: u32,
        qty: u64,
        stp: StpPolicy,
        seq: u64,
        events: &mut EventBuffer<'_>,
    ) -> (u64, bool) {
        let mut remaining = qty;
        let mut stp_stopped = false;
        while remaining > 0 {
            let Some(maker_idx) = self.best_opposite_idx(side) else {
                break;
            };
            if !crosses(side, price_idx, maker_idx) {
                break;
            }
            let maker_slot = self.levels.head[maker_idx as usize];
            debug_assert!(maker_slot != NIL, "active level has no head");
            let ms = maker_slot as usize;
            if self.arena.participant[ms] == participant.0 {
                match stp {
                    StpPolicy::CancelIncoming => {
                        stp_stopped = true;
                        break;
                    }
                    StpPolicy::CancelResting => {
                        let generation = self.arena.generation[ms];
                        self.remove_live_order(
                            maker_slot,
                            generation,
                            CancelReason::SelfTradePrevention,
                            seq,
                            events,
                        );
                        continue;
                    }
                    StpPolicy::CancelBoth => {
                        let generation = self.arena.generation[ms];
                        self.remove_live_order(
                            maker_slot,
                            generation,
                            CancelReason::SelfTradePrevention,
                            seq,
                            events,
                        );
                        stp_stopped = true;
                        break;
                    }
                }
            }
            let maker_id = OrderId(self.arena.order_id[ms]);
            let trade_qty = remaining.min(self.arena.qty[ms]);
            let trade_price = self.price_of_idx(maker_idx);
            self.arena.qty[ms] -= trade_qty;
            self.levels.qty[maker_idx as usize] -= trade_qty;
            remaining -= trade_qty;
            events.emit(ExecutionEvent::Trade {
                seq,
                maker_order_id: maker_id,
                taker_order_id: order_id,
                price: trade_price,
                qty: trade_qty,
            });
            if self.arena.qty[ms] == 0 {
                self.detach(maker_slot);
                events.emit(ExecutionEvent::Filled {
                    seq,
                    order_id: maker_id,
                });
            }
        }
        (remaining, stp_stopped)
    }

    /// Preflight and execute a `PostOnly` add: the sweep never runs, so
    /// the whole preflight is the crossing test, the one-event buffer
    /// bound, capacity, and the aggregate check.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the public add_order signature fixed by ARCHITECTURE.md section 7.1"
    )]
    fn execute_post_only(
        &mut self,
        order_id: OrderId,
        participant: ParticipantId,
        side: Side,
        price: u64,
        price_idx: u32,
        qty: u64,
        events: &mut EventBuffer<'_>,
    ) -> Result<usize, BookError> {
        if let Some(opposite_idx) = self.best_opposite_idx(side)
            && crosses(side, price_idx, opposite_idx)
        {
            return Err(BookError::PostOnlyWouldCross {
                price,
                opposite_best: self.price_of_idx(opposite_idx),
            });
        }
        if events.remaining() < 1 {
            return Err(BookError::EventBufferFull {
                needed: 1,
                available: events.remaining(),
            });
        }
        if self.arena.free_count() == 0 {
            return Err(BookError::CapacityFull {
                max_orders: self.config.max_orders,
            });
        }
        if self.levels.qty[price_idx as usize].checked_add(qty).is_none() {
            return Err(BookError::ArithmeticOverflow {
                what: "price level quantity",
            });
        }
        let seq = self.take_seq()?;
        self.rest_order(order_id, participant, side, price_idx, qty);
        events.emit(ExecutionEvent::Accepted { seq, order_id });
        Ok(1)
    }

    /// Read-only simulation of the sweep an incoming order would
    /// take, under its STP policy. This is the preflight plan: it
    /// must agree with execution step for step, and the debug
    /// assertions in `execute_add` check that it does.
    fn plan_sweep(
        &self,
        side: Side,
        price_idx: u32,
        qty: u64,
        participant: ParticipantId,
        stp: StpPolicy,
    ) -> SweepPlan {
        let mut plan = SweepPlan {
            events: 0,
            freed_slots: 0,
            remaining: qty,
            stp_stopped: false,
        };
        let mut cursor = self.best_opposite_idx(side);
        'levels: while plan.remaining > 0 {
            let Some(idx) = cursor else { break };
            if !crosses(side, price_idx, idx) {
                break;
            }
            let mut slot = self.levels.head[idx as usize];
            while slot != NIL {
                let s = slot as usize;
                if self.arena.participant[s] == participant.0 {
                    match stp {
                        StpPolicy::CancelIncoming => {
                            plan.stp_stopped = true;
                            break 'levels;
                        }
                        StpPolicy::CancelResting => {
                            plan.events += 1;
                            plan.freed_slots += 1;
                        }
                        StpPolicy::CancelBoth => {
                            plan.events += 1;
                            plan.freed_slots += 1;
                            plan.stp_stopped = true;
                            break 'levels;
                        }
                    }
                } else {
                    plan.events += 1; // Trade
                    let maker_qty = self.arena.qty[s];
                    if maker_qty <= plan.remaining {
                        plan.remaining -= maker_qty;
                        plan.events += 1; // Filled (maker)
                        plan.freed_slots += 1;
                    } else {
                        plan.remaining = 0;
                    }
                }
                if plan.remaining == 0 {
                    break;
                }
                slot = self.arena.next[s];
            }
            cursor = self.next_opposite_idx(side, idx);
        }
        plan
    }
    /// Best level index on the side opposite to `side`.
    fn best_opposite_idx(&self, side: Side) -> Option<u32> {
        match side {
            Side::Bid => self.asks.lowest(),
            Side::Ask => self.bids.highest(),
        }
    }

    /// Next level index on the opposite side beyond `idx`, moving away
    /// from the best price.
    fn next_opposite_idx(&self, side: Side, idx: u32) -> Option<u32> {
        match side {
            Side::Bid => self.asks.lowest_above(idx),
            Side::Ask => self.bids.highest_below(idx),
        }
    }

    /// Insert a resting remainder at the tail of its level, updating
    /// arena, lookup, level aggregates, and the side bitmap.
    fn rest_order(
        &mut self,
        order_id: OrderId,
        participant: ParticipantId,
        side: Side,
        price_idx: u32,
        qty: u64,
    ) {
        let (slot, generation) = self.arena.alloc().expect("preflight proved a free slot");
        let s = slot as usize;
        self.arena.order_id[s] = order_id.0;
        self.arena.participant[s] = participant.0;
        self.arena.price_idx[s] = price_idx;
        self.arena.qty[s] = qty;
        self.arena.side[s] = side.as_u8();
        self.lookup
            .insert(order_id.0, slot, generation)
            .expect("preflight proved lookup capacity");
        self.link_at_tail(slot, side);
    }

    /// Link a slot whose arena columns are already set at the tail of
    /// its price level, updating level aggregates and the side bitmap.
    /// Shared by `rest_order` (fresh allocations) and `amend_order`
    /// (an order that lost its priority rejoins at the tail).
    fn link_at_tail(&mut self, slot: u32, side: Side) {
        let s = slot as usize;
        let idx = self.arena.price_idx[s];
        let i = idx as usize;
        let tail = self.levels.tail[i];
        self.arena.prev[s] = tail;
        self.arena.next[s] = NIL;
        if tail == NIL {
            self.levels.head[i] = slot;
            match side {
                Side::Bid => self.bids.set(idx),
                Side::Ask => self.asks.set(idx),
            }
        } else {
            self.arena.next[tail as usize] = slot;
        }
        self.levels.tail[i] = slot;
        self.levels.qty[i] += self.arena.qty[s];
        self.levels.count[i] += 1;
    }

    /// Unlink a live order from its level list and aggregates without
    /// releasing its slot or its lookup entry. The mirror of
    /// `link_at_tail`; `detach` is this plus deregistration and
    /// release. Used by amend, where the order keeps its identity and
    /// its slot and only its position changes.
    fn unlink_from_level(&mut self, slot: u32) {
        let s = slot as usize;
        let idx = self.arena.price_idx[s];
        let i = idx as usize;
        let side = Side::from_u8(self.arena.side[s]);
        let prev = self.arena.prev[s];
        let next = self.arena.next[s];
        if prev == NIL {
            self.levels.head[i] = next;
        } else {
            self.arena.next[prev as usize] = next;
        }
        if next == NIL {
            self.levels.tail[i] = prev;
        } else {
            self.arena.prev[next as usize] = prev;
        }
        self.levels.qty[i] -= self.arena.qty[s];
        self.levels.count[i] -= 1;
        if self.levels.count[i] == 0 {
            match side {
                Side::Bid => self.bids.clear(idx),
                Side::Ask => self.asks.clear(idx),
            }
        }
        self.arena.prev[s] = NIL;
        self.arena.next[s] = NIL;
    }

    /// Detach a live order from its level list, level aggregates, the
    /// side bitmap, and the lookup, and release its slot. Emits no
    /// event; callers emit the lifecycle event that fits their path.
    fn detach(&mut self, slot: u32) {
        let s = slot as usize;
        let idx = self.arena.price_idx[s];
        let i = idx as usize;
        let side = Side::from_u8(self.arena.side[s]);
        let order_id = self.arena.order_id[s];
        let prev = self.arena.prev[s];
        let next = self.arena.next[s];
        if prev == NIL {
            self.levels.head[i] = next;
        } else {
            self.arena.next[prev as usize] = next;
        }
        if next == NIL {
            self.levels.tail[i] = prev;
        } else {
            self.arena.prev[next as usize] = prev;
        }
        self.levels.qty[i] -= self.arena.qty[s];
        self.levels.count[i] -= 1;
        if self.levels.count[i] == 0 {
            match side {
                Side::Bid => self.bids.clear(idx),
                Side::Ask => self.asks.clear(idx),
            }
        }
        let removed = self.lookup.remove(order_id);
        debug_assert_eq!(
            removed.map(|(sl, _)| sl),
            Some(slot),
            "lookup and arena disagree"
        );
        self.arena.release(slot);
    }

    /// Unlink, deregister, and release one live order, emitting its
    /// `Cancelled` event. Shared by cancel and (from Milestone 2)
    /// mass cancel and self-trade prevention.
    fn remove_live_order(
        &mut self,
        slot: u32,
        generation: u32,
        reason: CancelReason,
        seq: u64,
        events: &mut EventBuffer<'_>,
    ) {
        debug_assert!(self.arena.validate_handle(slot, generation));
        let order_id = OrderId(self.arena.order_id[slot as usize]);
        self.detach(slot);
        events.emit(ExecutionEvent::Cancelled {
            seq,
            order_id,
            reason,
        });
    }
}

/// The preflight plan for one sweep (see `Book::plan_sweep`): the
/// exact event count before the terminal event, the number of arena
/// slots the sweep frees (makers filled plus resting orders STP
/// cancels), the taker remainder the sweep leaves, and whether STP
/// stopped the sweep at an own order.
struct SweepPlan {
    /// Sweep events only; the caller adds one terminal event.
    events: usize,
    /// Slots freed by the sweep.
    freed_slots: usize,
    /// Taker quantity left when the sweep stops.
    remaining: u64,
    /// True when STP stopped the sweep at an own resting order.
    stp_stopped: bool,
}

/// Crossing test in dense tick index space: an incoming bid crosses an
/// ask level at or below its own index; an incoming ask crosses a bid
/// level at or above its own index.
const fn crosses(side: Side, taker_idx: u32, maker_idx: u32) -> bool {
    match side {
        Side::Bid => maker_idx <= taker_idx,
        Side::Ask => maker_idx >= taker_idx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> BookConfig {
        BookConfig {
            max_orders: 64,
            grid_len: 4096,
            tick_size: 1,
            grid_start_tick: 1,
            default_stp: crate::types::StpPolicy::CancelIncoming,
        }
    }

    fn book() -> Book {
        Book::new(test_config()).expect("valid config")
    }

    fn buffer() -> Vec<ExecutionEvent> {
        Vec::with_capacity(1024)
    }

    /// Run one Limit add against a fresh buffer and return its events.
    fn add(
        b: &mut Book,
        id: u64,
        participant: u32,
        side: Side,
        price: u64,
        qty: u64,
    ) -> Vec<ExecutionEvent> {
        add_typed(b, id, participant, side, OrderType::Limit, price, qty, None)
    }

    /// Run one add of any type against a fresh buffer, with an explicit
    /// STP policy (`None` exercises the book default), and return its
    /// events.
    #[allow(
        clippy::too_many_arguments,
        reason = "test helper mirrors the public add_order parameter list"
    )]
    fn add_typed(
        b: &mut Book,
        id: u64,
        participant: u32,
        side: Side,
        order_type: OrderType,
        price: u64,
        qty: u64,
        stp: Option<StpPolicy>,
    ) -> Vec<ExecutionEvent> {
        let mut storage = Vec::with_capacity(64);
        {
            let mut events = EventBuffer::new(&mut storage);
            b.add_order(
                OrderId(id),
                ParticipantId(participant),
                side,
                order_type,
                price,
                qty,
                stp,
                &mut events,
            )
            .expect("add");
        }
        storage
    }

    /// Run one amend against a fresh buffer and return its events.
    fn amend(b: &mut Book, id: u64, new_price: u64, new_qty: u64) -> Vec<ExecutionEvent> {
        let mut storage = Vec::with_capacity(4);
        {
            let mut events = EventBuffer::new(&mut storage);
            b.amend_order(OrderId(id), new_price, new_qty, &mut events)
                .expect("amend");
        }
        storage
    }

    /// Full observable state, for atomicity assertions: sequence,
    /// liveness, best prices, and complete depth on both sides.
    type StateSnapshot = (
        u64,
        u32,
        Option<(u64, u64)>,
        Option<(u64, u64)>,
        Vec<(u64, u64, u32)>,
        Vec<(u64, u64, u32)>,
    );

    fn snapshot(b: &Book) -> StateSnapshot {
        fn depth_of(b: &Book, side: Side) -> Vec<(u64, u64, u32)> {
            let mut out = [DepthLevel {
                price: 0,
                total_qty: 0,
                order_count: 0,
            }; 64];
            let n = b.depth(side, 64, &mut out);
            out[..n]
                .iter()
                .map(|d| (d.price, d.total_qty, d.order_count))
                .collect()
        }
        (
            b.next_seq(),
            b.live_order_count(),
            b.best_bid(),
            b.best_ask(),
            depth_of(b, Side::Bid),
            depth_of(b, Side::Ask),
        )
    }

    #[test]
    fn guard_zero_order_id() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .add_order(
                OrderId(0),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                5,
                None,
                &mut events,
            )
            .expect_err("zero id must be rejected");
        assert_eq!(
            err,
            BookError::InvalidOrderId {
                order_id: OrderId(0)
            }
        );
        assert_eq!(b.live_order_count(), 0);
        assert_eq!(b.next_seq(), 1);
    }

    #[test]
    fn guard_zero_qty() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .add_order(
                OrderId(1),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                0,
                None,
                &mut events,
            )
            .expect_err("zero qty must be rejected");
        assert_eq!(err, BookError::InvalidQuantity { qty: 0 });
        assert_eq!(b.live_order_count(), 0);
    }

    #[test]
    fn guard_duplicate_order_id() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(7),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            5,
            None,
            &mut events,
        )
        .expect("first add");
        let err = b
            .add_order(
                OrderId(7),
                ParticipantId(2),
                Side::Ask,
                OrderType::Limit,
                101,
                5,
                None,
                &mut events,
            )
            .expect_err("duplicate id must be rejected");
        assert_eq!(
            err,
            BookError::DuplicateOrderId {
                order_id: OrderId(7)
            }
        );
        assert_eq!(b.live_order_count(), 1);
    }

    #[test]
    fn guard_invalid_price_tick_each_rule() {
        let mut cfg = test_config();
        cfg.tick_size = 5;
        cfg.grid_start_tick = 100;
        cfg.grid_len = 640;
        let mut b = Book::new(cfg).expect("valid config");
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let cases = [
            0u64,  // zero price
            502,   // unaligned: 502 is not a multiple of 5
            495,   // aligned tick 99, below grid start 100
            3_700, // aligned tick 740, grid end is 100 + 640 = 740, excluded
        ];
        for price in cases {
            let err = b
                .add_order(
                    OrderId(1),
                    ParticipantId(1),
                    Side::Bid,
                    OrderType::Limit,
                    price,
                    5,
                    None,
                    &mut events,
                )
                .expect_err("each tick rule must reject");
            assert!(
                matches!(err, BookError::InvalidPriceTick { .. }),
                "price {price} gave {err:?}"
            );
        }
        assert_eq!(b.live_order_count(), 0);
        assert_eq!(b.next_seq(), 1);
    }

    #[test]
    fn limit_orders_rest_and_report_best_prices() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            5,
            None,
            &mut events,
        )
        .expect("bid 1");
        b.add_order(
            OrderId(2),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            101,
            3,
            None,
            &mut events,
        )
        .expect("bid 2");
        b.add_order(
            OrderId(3),
            ParticipantId(2),
            Side::Ask,
            OrderType::Limit,
            105,
            4,
            None,
            &mut events,
        )
        .expect("ask");
        assert_eq!(b.best_bid(), Some((101, 3)));
        assert_eq!(b.best_ask(), Some((105, 4)));
        let mut out = [DepthLevel {
            price: 0,
            total_qty: 0,
            order_count: 0,
        }; 4];
        let n = b.depth(Side::Bid, 4, &mut out);
        assert_eq!(n, 2);
        assert_eq!(
            out[0],
            DepthLevel {
                price: 101,
                total_qty: 3,
                order_count: 1
            }
        );
        assert_eq!(
            out[1],
            DepthLevel {
                price: 100,
                total_qty: 5,
                order_count: 1
            }
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn fifo_partial_fill_keeps_head_and_residual_rests_at_tail() {
        let mut b = book();
        // Two makers at the same ask price; arrival order 1 then 2.
        add(&mut b, 1, 1, Side::Ask, 100, 10);
        add(&mut b, 2, 1, Side::Ask, 100, 10);
        // A third maker at a worse ask price.
        add(&mut b, 3, 1, Side::Ask, 101, 5);
        // Taker buys 15: fills maker 1 fully, maker 2 partially.
        let stream = add(&mut b, 9, 2, Side::Bid, 100, 15);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 10,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(2),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(9)
                },
            ]
        );
        // Maker 2 still rests at the head of the 100 level with 5 left.
        assert_eq!(b.best_ask(), Some((100, 5)));
        // A small taker now hits maker 2 (still head), not maker 3.
        add(&mut b, 10, 2, Side::Bid, 101, 5);
        assert_eq!(b.best_ask(), Some((101, 5)));
        b.debug_assert_invariants();
    }

    #[test]
    fn level_aggregates_track_fill_cancel_amend() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            10,
            None,
            &mut events,
        )
        .expect("add 1");
        b.add_order(
            OrderId(2),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            6,
            None,
            &mut events,
        )
        .expect("add 2");
        assert_eq!(b.best_bid(), Some((100, 16)));
        b.add_order(
            OrderId(3),
            ParticipantId(2),
            Side::Ask,
            OrderType::Limit,
            100,
            4,
            None,
            &mut events,
        )
        .expect("crossing ask fills 4 from the head bid");
        assert_eq!(b.best_bid(), Some((100, 12)));
        b.cancel_order(OrderId(2), &mut events).expect("cancel");
        assert_eq!(b.best_bid(), Some((100, 6)));
        // Amend legs (Milestone 2): reduce, increase, reprice. Order 1
        // is the only order left, holding 6 at price 100.
        amend(&mut b, 1, 100, 4);
        assert_eq!(b.best_bid(), Some((100, 4)), "reduce drops the aggregate");
        amend(&mut b, 1, 100, 9);
        assert_eq!(b.best_bid(), Some((100, 9)), "increase raises the aggregate");
        amend(&mut b, 1, 99, 9);
        assert_eq!(b.best_bid(), Some((99, 9)), "reprice moves the aggregate");
        let mut out = [DepthLevel {
            price: 0,
            total_qty: 0,
            order_count: 0,
        }; 4];
        assert_eq!(b.depth(Side::Bid, 4, &mut out), 1);
        assert_eq!(
            out[0],
            DepthLevel {
                price: 99,
                total_qty: 9,
                order_count: 1
            }
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn cancel_missing_order_is_an_error_without_side_effects() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .cancel_order(OrderId(42), &mut events)
            .expect_err("missing order");
        assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(42)
            }
        );
        assert_eq!(b.next_seq(), 1, "rejected cancel consumes no seq");
    }

    #[test]
    fn best_prices_clear_after_last_cancel() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            5,
            None,
            &mut events,
        )
        .expect("add");
        b.cancel_order(OrderId(1), &mut events).expect("cancel");
        assert_eq!(b.best_bid(), None);
        assert_eq!(b.live_order_count(), 0);
        b.debug_assert_invariants();
    }

    #[test]
    fn capacity_full_rejects_an_order_that_would_rest() {
        let mut cfg = test_config();
        cfg.max_orders = 2;
        let mut b = Book::new(cfg).expect("valid config");
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            5,
            None,
            &mut events,
        )
        .expect("add 1");
        b.add_order(
            OrderId(2),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            99,
            5,
            None,
            &mut events,
        )
        .expect("add 2");
        let err = b
            .add_order(
                OrderId(3),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                98,
                5,
                None,
                &mut events,
            )
            .expect_err("third resting order exceeds capacity");
        assert_eq!(err, BookError::CapacityFull { max_orders: 2 });
        assert_eq!(b.live_order_count(), 2);
    }

    #[test]
    fn math_checked_paths_at_u64_edges() {
        // INV-MATH-1: aggregate arithmetic is checked at the boundary.
        // A level already holding u64::MAX cannot take one more unit:
        // the add fails in preflight with a typed error, atomically.
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            u64::MAX,
            None,
            &mut events,
        )
        .expect("one maximal order rests");
        assert_eq!(b.best_bid(), Some((100, u64::MAX)));
        let err = b
            .add_order(
                OrderId(2),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                1,
                None,
                &mut events,
            )
            .expect_err("level aggregate would overflow u64");
        assert_eq!(
            err,
            BookError::ArithmeticOverflow {
                what: "price level quantity"
            }
        );
        assert_eq!(
            b.best_bid(),
            Some((100, u64::MAX)),
            "book unchanged after overflow rejection"
        );
        assert_eq!(b.live_order_count(), 1);
        assert_eq!(b.next_seq(), 2, "rejected command consumed no seq");
        // Amend leg (Milestone 2): repricing a second order onto the
        // maximal level would overflow the same aggregate, and is
        // rejected the same way, before any mutation.
        b.add_order(
            OrderId(3),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            101,
            5,
            None,
            &mut events,
        )
        .expect("second order rests at its own level");
        let before = snapshot(&b);
        let mut storage2 = buffer();
        let mut events2 = EventBuffer::new(&mut storage2);
        let err = b
            .amend_order(OrderId(3), 100, 5, &mut events2)
            .expect_err("amend onto a maximal level overflows the aggregate");
        assert_eq!(
            err,
            BookError::ArithmeticOverflow {
                what: "price level quantity"
            }
        );
        assert_eq!(snapshot(&b), before, "overflowing amend mutated nothing");
        b.debug_assert_invariants();
    }

    #[test]
    fn event_buffer_full_is_atomic() {
        let mut b = book();
        let mut big_storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
        let mut big = EventBuffer::new(&mut big_storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Ask,
            OrderType::Limit,
            100,
            5,
            None,
            &mut big,
        )
        .expect("maker rests");
        // A crossing taker emits Trade + Filled(maker) + Filled(taker) =
        // 3 events. Offer a buffer with room for 2.
        let mut small_storage: Vec<ExecutionEvent> = Vec::with_capacity(2);
        let err = {
            let mut small = EventBuffer::new(&mut small_storage);
            b.add_order(
                OrderId(2),
                ParticipantId(2),
                Side::Bid,
                OrderType::Limit,
                100,
                5,
                None,
                &mut small,
            )
            .expect_err("buffer below the preflight bound")
        };
        assert_eq!(
            err,
            BookError::EventBufferFull {
                needed: 3,
                available: 2
            }
        );
        assert_eq!(b.best_ask(), Some((100, 5)), "maker untouched");
        assert_eq!(b.next_seq(), 2, "rejected command consumed no seq");
        assert_eq!(small_storage.len(), 0, "no partial events written");
        // Milestone 2 legs: the same bound discipline for the new
        // command types, each with its exact per-command bound
        // (ARCHITECTURE.md section 7.2).
        //
        // Amend bound is exactly 1.
        let mut zero_storage: Vec<ExecutionEvent> = Vec::new();
        let err = {
            let mut zero = EventBuffer::new(&mut zero_storage);
            b.amend_order(OrderId(1), 100, 3, &mut zero)
                .expect_err("amend needs exactly one event slot")
        };
        assert_eq!(
            err,
            BookError::EventBufferFull {
                needed: 1,
                available: 0
            }
        );
        assert_eq!(b.best_ask(), Some((100, 5)), "amend left the order alone");
        // Ioc bound for this sweep is the same 3 events.
        let mut two_storage: Vec<ExecutionEvent> = Vec::with_capacity(2);
        let err = {
            let mut two = EventBuffer::new(&mut two_storage);
            b.add_order(
                OrderId(2),
                ParticipantId(2),
                Side::Bid,
                OrderType::Ioc,
                100,
                5,
                None,
                &mut two,
            )
            .expect_err("ioc sweep needs three event slots")
        };
        assert_eq!(
            err,
            BookError::EventBufferFull {
                needed: 3,
                available: 2
            }
        );
        assert_eq!(b.best_ask(), Some((100, 5)), "maker still untouched");
        b.debug_assert_invariants();
    }

    #[test]
    fn event_buffer_full_cancel_by_participant_bound() {
        let mut b = book();
        let mut storage = buffer();
        let mut setup = EventBuffer::new(&mut storage);
        b.add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100,
            5,
            None,
            &mut setup,
        )
        .expect("setup bid rests");
        b.add_order(
            OrderId(3),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            99,
            5,
            None,
            &mut setup,
        )
        .expect("setup bid rests");
        let mut tiny_storage: Vec<ExecutionEvent> = Vec::with_capacity(1);
        let mut tiny = EventBuffer::new(&mut tiny_storage);
        let err = b
            .cancel_by_participant(ParticipantId(1), &mut tiny)
            .expect_err("bound is the live order count");
        assert_eq!(
            err,
            BookError::EventBufferFull {
                needed: 2,
                available: 1
            }
        );
        assert_eq!(b.live_order_count(), 2, "nothing was cancelled");
        b.debug_assert_invariants();
    }

    #[test]
    fn ioc_expires_remainder_and_never_rests() {
        let mut b = book();
        add(&mut b, 1, 2, Side::Ask, 100, 5);
        let stream = add_typed(&mut b, 2, 3, Side::Bid, OrderType::Ioc, 100, 8, None);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 2,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(2),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 2,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Expired {
                    seq: 2,
                    order_id: OrderId(2),
                    remaining_qty: 3
                },
            ]
        );
        assert_eq!(b.live_order_count(), 0, "an Ioc order never rests");
        // The expired id is terminal: cancelling it finds nothing.
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .cancel_order(OrderId(2), &mut events)
            .expect_err("expired order is not live");
        assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(2)
            }
        );
        // An Ioc order that crosses nothing expires in full.
        let stream = add_typed(&mut b, 3, 3, Side::Bid, OrderType::Ioc, 100, 4, None);
        assert_eq!(
            stream,
            [ExecutionEvent::Expired {
                seq: 3,
                order_id: OrderId(3),
                remaining_qty: 4
            }]
        );
        assert_eq!(b.best_bid(), None);
        b.debug_assert_invariants();
    }

    #[test]
    fn ioc_fills_fully_when_liquidity_covers() {
        let mut b = book();
        add(&mut b, 1, 2, Side::Ask, 100, 5);
        let stream = add_typed(&mut b, 2, 3, Side::Bid, OrderType::Ioc, 100, 5, None);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 2,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(2),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 2,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Filled {
                    seq: 2,
                    order_id: OrderId(2)
                },
            ]
        );
        assert_eq!(b.live_order_count(), 0);
        b.debug_assert_invariants();
    }

    #[test]
    fn ioc_and_fok_do_not_need_a_slot_on_a_full_book() {
        let mut cfg = test_config();
        cfg.max_orders = 1;
        let mut b = Book::new(cfg).expect("valid config");
        add(&mut b, 1, 2, Side::Ask, 100, 5);
        assert_eq!(b.live_order_count(), 1, "the book is full");
        // Neither order rests, so neither needs the slot the filled
        // maker is about to free.
        let stream = add_typed(&mut b, 2, 3, Side::Bid, OrderType::Ioc, 100, 5, None);
        assert_eq!(stream.len(), 3);
        add(&mut b, 3, 2, Side::Ask, 100, 5);
        let stream = add_typed(&mut b, 4, 3, Side::Bid, OrderType::Fok, 100, 5, None);
        assert_eq!(stream.len(), 3);
        assert_eq!(b.live_order_count(), 0);
        b.debug_assert_invariants();
    }

    #[test]
    fn fok_fills_fully_or_not_at_all() {
        let mut b = book();
        add(&mut b, 1, 2, Side::Ask, 100, 5);
        add(&mut b, 2, 2, Side::Ask, 101, 5);
        let before = snapshot(&b);
        let mut storage = buffer();
        let err = {
            let mut events = EventBuffer::new(&mut storage);
            b.add_order(
                OrderId(3),
                ParticipantId(3),
                Side::Bid,
                OrderType::Fok,
                101,
                11,
                None,
                &mut events,
            )
            .expect_err("eleven requested, ten available")
        };
        assert_eq!(
            err,
            BookError::FokInsufficientLiquidity {
                requested: 11,
                available: 10
            }
        );
        assert_eq!(snapshot(&b), before, "a failed Fok mutates nothing");
        let stream = add_typed(&mut b, 3, 3, Side::Bid, OrderType::Fok, 101, 10, None);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 3,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(3),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 3,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Trade {
                    seq: 3,
                    maker_order_id: OrderId(2),
                    taker_order_id: OrderId(3),
                    price: 101,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 3,
                    order_id: OrderId(2)
                },
                ExecutionEvent::Filled {
                    seq: 3,
                    order_id: OrderId(3)
                },
            ]
        );
        assert_eq!(b.live_order_count(), 0);
        b.debug_assert_invariants();
    }

    /// Resting layout for the STP Fok cases: the taker (participant 1)
    /// owns the middle ask.
    fn fok_stp_layout(b: &mut Book) {
        add(b, 1, 9, Side::Ask, 100, 5);
        add(b, 2, 1, Side::Ask, 100, 5);
        add(b, 3, 9, Side::Ask, 101, 5);
    }

    #[test]
    fn fok_available_accounts_for_stp() {
        // INV-ORD-1: `available` is the exact STP-adjusted executable
        // quantity of ARCHITECTURE.md section 7.5, per policy.
        for (stp, available) in [
            (StpPolicy::CancelIncoming, 5),
            (StpPolicy::CancelResting, 10),
            (StpPolicy::CancelBoth, 5),
        ] {
            let mut b = book();
            fok_stp_layout(&mut b);
            let before = snapshot(&b);
            let mut storage = buffer();
            let err = {
                let mut events = EventBuffer::new(&mut storage);
                b.add_order(
                    OrderId(9),
                    ParticipantId(1),
                    Side::Bid,
                    OrderType::Fok,
                    101,
                    12,
                    Some(stp),
                    &mut events,
                )
                .expect_err("twelve is not executable under any policy")
            };
            assert_eq!(
                err,
                BookError::FokInsufficientLiquidity {
                    requested: 12,
                    available
                },
                "policy {stp:?}"
            );
            assert_eq!(
                snapshot(&b),
                before,
                "policy {stp:?}: a failed Fok mutates nothing, not even STP cancels"
            );
        }
        // Under CancelResting the same layout fills ten: the own order
        // is cancelled in the sweep and the deeper order completes it.
        let mut b = book();
        fok_stp_layout(&mut b);
        let stream = add_typed(
            &mut b,
            9,
            1,
            Side::Bid,
            OrderType::Fok,
            101,
            10,
            Some(StpPolicy::CancelResting),
        );
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Cancelled {
                    seq: 4,
                    order_id: OrderId(2),
                    reason: CancelReason::SelfTradePrevention
                },
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(3),
                    taker_order_id: OrderId(9),
                    price: 101,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(3)
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(9)
                },
            ]
        );
        assert_eq!(b.live_order_count(), 0);
        b.debug_assert_invariants();
    }

    #[test]
    fn post_only_rejection_is_atomic() {
        let mut b = book();
        add(&mut b, 1, 2, Side::Ask, 100, 5);
        let before = snapshot(&b);
        let mut storage = buffer();
        let err = {
            let mut events = EventBuffer::new(&mut storage);
            b.add_order(
                OrderId(2),
                ParticipantId(1),
                Side::Bid,
                OrderType::PostOnly,
                100,
                5,
                None,
                &mut events,
            )
            .expect_err("priced exactly at the best ask, a PostOnly buy crosses")
        };
        assert_eq!(
            err,
            BookError::PostOnlyWouldCross {
                price: 100,
                opposite_best: 100
            }
        );
        assert_eq!(snapshot(&b), before, "a rejected PostOnly mutates nothing");
        // One tick below the ask it rests in full.
        let stream = add_typed(&mut b, 2, 1, Side::Bid, OrderType::PostOnly, 99, 5, None);
        assert_eq!(
            stream,
            [ExecutionEvent::Accepted {
                seq: 2,
                order_id: OrderId(2)
            }]
        );
        assert_eq!(b.best_bid(), Some((99, 5)));
        // Equality crosses on the sell side too.
        let mut storage = buffer();
        let err = {
            let mut events = EventBuffer::new(&mut storage);
            b.add_order(
                OrderId(4),
                ParticipantId(2),
                Side::Ask,
                OrderType::PostOnly,
                99,
                5,
                None,
                &mut events,
            )
            .expect_err("priced exactly at the best bid, a PostOnly sell crosses")
        };
        assert_eq!(
            err,
            BookError::PostOnlyWouldCross {
                price: 99,
                opposite_best: 99
            }
        );
        b.debug_assert_invariants();
    }

    /// Three asks at one price: other, own, other, in arrival order.
    fn stp_layout(b: &mut Book) {
        add(b, 1, 9, Side::Ask, 100, 5);
        add(b, 2, 1, Side::Ask, 100, 5);
        add(b, 3, 9, Side::Ask, 100, 5);
    }

    #[test]
    fn stp_cancel_incoming_stops_at_own_order() {
        let mut b = book();
        stp_layout(&mut b);
        let stream = add_typed(
            &mut b,
            9,
            1,
            Side::Bid,
            OrderType::Limit,
            100,
            20,
            Some(StpPolicy::CancelIncoming),
        );
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::SelfTradePrevented {
                    seq: 4,
                    order_id: OrderId(9)
                },
            ]
        );
        // The own order is untouched and the third maker was never
        // reached; the taker remainder was cancelled, not rested.
        assert_eq!(b.best_ask(), Some((100, 10)));
        assert_eq!(b.best_bid(), None);
        assert_eq!(b.live_order_count(), 2);
        b.debug_assert_invariants();
    }

    #[test]
    fn stp_cancel_resting_skips_own_orders() {
        let mut b = book();
        stp_layout(&mut b);
        let stream = add_typed(
            &mut b,
            9,
            1,
            Side::Bid,
            OrderType::Limit,
            100,
            12,
            Some(StpPolicy::CancelResting),
        );
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Cancelled {
                    seq: 4,
                    order_id: OrderId(2),
                    reason: CancelReason::SelfTradePrevention
                },
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(3),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(3)
                },
                ExecutionEvent::Accepted {
                    seq: 4,
                    order_id: OrderId(9)
                },
            ]
        );
        // Both other makers filled, the own order was cancelled without
        // consuming taker quantity, and the remainder of 2 rests.
        assert_eq!(b.best_bid(), Some((100, 2)));
        assert_eq!(b.best_ask(), None);
        b.debug_assert_invariants();
    }

    #[test]
    fn stp_cancel_both_cancels_each_side() {
        let mut b = book();
        stp_layout(&mut b);
        let stream = add_typed(
            &mut b,
            9,
            1,
            Side::Bid,
            OrderType::Limit,
            100,
            20,
            Some(StpPolicy::CancelBoth),
        );
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(9),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Cancelled {
                    seq: 4,
                    order_id: OrderId(2),
                    reason: CancelReason::SelfTradePrevention
                },
                ExecutionEvent::SelfTradePrevented {
                    seq: 4,
                    order_id: OrderId(9)
                },
            ]
        );
        assert_eq!(b.best_ask(), Some((100, 5)), "the third maker survives");
        assert_eq!(b.best_bid(), None);
        b.debug_assert_invariants();
    }

    #[test]
    fn stp_default_applies_when_order_names_none() {
        let mut cfg = test_config();
        cfg.default_stp = StpPolicy::CancelResting;
        let mut b = Book::new(cfg).expect("valid config");
        stp_layout(&mut b);
        let stream = add(&mut b, 9, 1, Side::Bid, 100, 12);
        assert!(
            stream.contains(&ExecutionEvent::Cancelled {
                seq: 4,
                order_id: OrderId(2),
                reason: CancelReason::SelfTradePrevention
            }),
            "the book default CancelResting policy cancelled the own order: {stream:?}"
        );
        assert_eq!(b.best_bid(), Some((100, 2)));
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_reduce_preserves_priority() {
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 10);
        add(&mut b, 2, 1, Side::Bid, 100, 10);
        let stream = amend(&mut b, 1, 100, 4);
        assert_eq!(
            stream,
            [ExecutionEvent::Amended {
                seq: 3,
                order_id: OrderId(1),
                price: 100,
                qty: 4
            }]
        );
        // Order 1 is still the head: a sell of 5 fills it first.
        let stream = add(&mut b, 3, 2, Side::Ask, 100, 5);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(3),
                    price: 100,
                    qty: 4,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(1)
                },
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(2),
                    taker_order_id: OrderId(3),
                    price: 100,
                    qty: 1,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(3)
                },
            ]
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_identical_is_noop() {
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 10);
        add(&mut b, 2, 1, Side::Bid, 100, 10);
        let before = snapshot(&b);
        let stream = amend(&mut b, 1, 100, 10);
        assert_eq!(
            stream,
            [ExecutionEvent::Amended {
                seq: 3,
                order_id: OrderId(1),
                price: 100,
                qty: 10
            }]
        );
        let after = snapshot(&b);
        assert_eq!(after.0, before.0 + 1, "the no-op amend consumed one seq");
        assert_eq!(after.1, before.1, "live count unchanged");
        assert_eq!(after.2, before.2, "best bid unchanged");
        assert_eq!(after.4, before.4, "depth unchanged");
        // Position is untouched: order 1 still fills first.
        let stream = add(&mut b, 3, 2, Side::Ask, 100, 10);
        assert_eq!(
            stream[0],
            ExecutionEvent::Trade {
                seq: 4,
                maker_order_id: OrderId(1),
                taker_order_id: OrderId(3),
                price: 100,
                qty: 10
            }
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_increase_or_reprice_goes_to_tail() {
        // Increase at the same price.
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        add(&mut b, 2, 1, Side::Bid, 100, 5);
        amend(&mut b, 1, 100, 8);
        let stream = add(&mut b, 3, 2, Side::Ask, 100, 6);
        assert_eq!(
            stream,
            [
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(2),
                    taker_order_id: OrderId(3),
                    price: 100,
                    qty: 5,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(2)
                },
                ExecutionEvent::Trade {
                    seq: 4,
                    maker_order_id: OrderId(1),
                    taker_order_id: OrderId(3),
                    price: 100,
                    qty: 1,
                },
                ExecutionEvent::Filled {
                    seq: 4,
                    order_id: OrderId(3)
                },
            ],
            "the increased order fills behind the order that was behind it"
        );
        // Reprice onto another level: tail of the target level.
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        add(&mut b, 2, 1, Side::Bid, 99, 5);
        amend(&mut b, 1, 99, 5);
        let stream = add(&mut b, 3, 2, Side::Ask, 99, 6);
        assert_eq!(
            stream[0],
            ExecutionEvent::Trade {
                seq: 4,
                maker_order_id: OrderId(2),
                taker_order_id: OrderId(3),
                price: 99,
                qty: 5
            },
            "the repriced order sits behind the order already at 99"
        );
        assert_eq!(
            stream[2],
            ExecutionEvent::Trade {
                seq: 4,
                maker_order_id: OrderId(1),
                taker_order_id: OrderId(3),
                price: 99,
                qty: 1
            }
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_that_would_cross_is_rejected() {
        let mut b = book();
        add(&mut b, 9, 2, Side::Ask, 105, 5);
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        let before = snapshot(&b);
        let mut storage = buffer();
        let err = {
            let mut events = EventBuffer::new(&mut storage);
            b.amend_order(OrderId(1), 105, 5, &mut events)
                .expect_err("amending a bid to the best ask locks the book")
        };
        assert_eq!(
            err,
            BookError::AmendWouldCross {
                order_id: OrderId(1),
                new_price: 105,
                opposite_best: 105
            }
        );
        assert_eq!(snapshot(&b), before, "a crossing amend mutates nothing");
        // One tick below the ask is fine.
        let stream = amend(&mut b, 1, 104, 5);
        assert_eq!(
            stream,
            [ExecutionEvent::Amended {
                seq: 3,
                order_id: OrderId(1),
                price: 104,
                qty: 5
            }]
        );
        assert_eq!(b.best_bid(), Some((104, 5)));
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_missing_and_terminal_ids_are_not_found() {
        let mut b = book();
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .amend_order(OrderId(42), 100, 5, &mut events)
            .expect_err("never-seen id");
        assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(42)
            }
        );
        // A filled order is terminal.
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        add(&mut b, 2, 2, Side::Ask, 100, 5);
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .amend_order(OrderId(1), 100, 3, &mut events)
            .expect_err("filled order");
        assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(1)
            }
        );
        // A cancelled order is terminal.
        add(&mut b, 3, 1, Side::Bid, 100, 5);
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.cancel_order(OrderId(3), &mut events).expect("cancel");
        let err = b
            .amend_order(OrderId(3), 100, 3, &mut events)
            .expect_err("cancelled order");
        assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(3)
            }
        );
        b.debug_assert_invariants();
    }

    #[test]
    fn amend_zero_qty_and_bad_price_are_rejected() {
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        let before = snapshot(&b);
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        let err = b
            .amend_order(OrderId(1), 100, 0, &mut events)
            .expect_err("zero quantity");
        assert_eq!(err, BookError::InvalidQuantity { qty: 0 });
        let err = b
            .amend_order(OrderId(1), 0, 5, &mut events)
            .expect_err("zero price");
        assert!(matches!(err, BookError::InvalidPriceTick { .. }));
        assert_eq!(snapshot(&b), before);
        b.debug_assert_invariants();
    }

    #[test]
    fn cancel_by_participant_event_order_is_slot_order() {
        // INV-CANCEL-1: the snapshot format relies on this order.
        // Slots are allocated 0, 1, 2, ... and freed slots are reused
        // LIFO, so after cancelling order 1 (slot 0) the next add
        // (order 5) takes slot 0, ahead of orders 3 and 4.
        let mut b = book();
        add(&mut b, 1, 1, Side::Bid, 100, 5);
        add(&mut b, 2, 2, Side::Bid, 100, 5);
        add(&mut b, 3, 1, Side::Bid, 99, 5);
        add(&mut b, 4, 1, Side::Ask, 105, 5);
        let mut storage = buffer();
        let mut events = EventBuffer::new(&mut storage);
        b.cancel_order(OrderId(1), &mut events).expect("cancel");
        add(&mut b, 5, 1, Side::Bid, 98, 5);
        let mut storage = Vec::with_capacity(16);
        let written = {
            let mut events = EventBuffer::new(&mut storage);
            b.cancel_by_participant(ParticipantId(1), &mut events)
                .expect("mass cancel")
        };
        assert_eq!(written, 3);
        assert_eq!(
            storage,
            [
                ExecutionEvent::Cancelled {
                    seq: 7,
                    order_id: OrderId(5),
                    reason: CancelReason::ByParticipant
                },
                ExecutionEvent::Cancelled {
                    seq: 7,
                    order_id: OrderId(3),
                    reason: CancelReason::ByParticipant
                },
                ExecutionEvent::Cancelled {
                    seq: 7,
                    order_id: OrderId(4),
                    reason: CancelReason::ByParticipant
                },
            ],
            "ascending slot order: slot 0 (order 5), slot 2 (order 3), slot 3 (order 4)"
        );
        // Only participant 2's order survives.
        assert_eq!(b.live_order_count(), 1);
        assert_eq!(b.best_bid(), Some((100, 5)));
        assert_eq!(b.best_ask(), None);
        b.debug_assert_invariants();
    }

    #[test]
    fn cancel_by_participant_with_no_orders_succeeds_empty() {
        let mut b = book();
        add(&mut b, 1, 2, Side::Bid, 100, 5);
        let mut storage = Vec::with_capacity(4);
        let written = {
            let mut events = EventBuffer::new(&mut storage);
            b.cancel_by_participant(ParticipantId(7), &mut events)
                .expect("an empty mass cancel is an accepted command")
        };
        assert_eq!(written, 0);
        assert_eq!(storage.len(), 0);
        assert_eq!(b.next_seq(), 3, "the accepted command consumed a seq");
        assert_eq!(b.best_bid(), Some((100, 5)), "other participants untouched");
        b.debug_assert_invariants();
    }

    #[test]
    fn event_shape_golden() {
        // INV-EVENT-3: one scripted session exercising every command
        // type, with the complete event stream pinned.
        let mut b = book();
        let mut storage = buffer();
        let mut run = |b: &mut Book, f: &dyn Fn(&mut Book, &mut EventBuffer<'_>)| {
            let mut events = EventBuffer::new(&mut storage);
            f(b, &mut events);
        };
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(1),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                100,
                10,
                None,
                e,
            )
            .expect("bid rests");
        });
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(2),
                ParticipantId(2),
                Side::Ask,
                OrderType::Limit,
                105,
                10,
                None,
                e,
            )
            .expect("ask rests");
        });
        run(&mut b, &|b, e| {
            b.amend_order(OrderId(2), 103, 10, e).expect("amend");
        });
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(3),
                ParticipantId(3),
                Side::Bid,
                OrderType::Limit,
                103,
                4,
                None,
                e,
            )
            .expect("crossing bid fills");
        });
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(4),
                ParticipantId(3),
                Side::Ask,
                OrderType::Ioc,
                100,
                15,
                None,
                e,
            )
            .expect("ioc sweeps and expires");
        });
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(5),
                ParticipantId(1),
                Side::Ask,
                OrderType::Limit,
                102,
                7,
                None,
                e,
            )
            .expect("ask rests");
        });
        run(&mut b, &|b, e| {
            b.add_order(
                OrderId(6),
                ParticipantId(1),
                Side::Bid,
                OrderType::Limit,
                102,
                7,
                Some(StpPolicy::CancelIncoming),
                e,
            )
            .expect("self-trade prevented");
        });
        run(&mut b, &|b, e| {
            b.cancel_by_participant(ParticipantId(1), e)
                .expect("mass cancel");
        });
        run(&mut b, &|b, e| {
            b.cancel_order(OrderId(2), e).expect("cancel");
        });
        assert_eq!(storage, golden_expected());

        assert_eq!(b.live_order_count(), 0, "every order reached a terminal state");
        assert_eq!(b.next_seq(), 10);
        b.debug_assert_invariants();
    }

    /// The pinned event stream for `event_shape_golden` (INV-EVENT-3):
    /// every variant, in order, with exact seqs and payloads.
    fn golden_expected() -> Vec<ExecutionEvent> {
        vec![
            ExecutionEvent::Accepted {
                seq: 1,
                order_id: OrderId(1)
            },
            ExecutionEvent::Accepted {
                seq: 2,
                order_id: OrderId(2)
            },
            ExecutionEvent::Amended {
                seq: 3,
                order_id: OrderId(2),
                price: 103,
                qty: 10
            },
            ExecutionEvent::Trade {
                seq: 4,
                maker_order_id: OrderId(2),
                taker_order_id: OrderId(3),
                price: 103,
                qty: 4,
            },
            ExecutionEvent::Filled {
                seq: 4,
                order_id: OrderId(3)
            },
            ExecutionEvent::Trade {
                seq: 5,
                maker_order_id: OrderId(1),
                taker_order_id: OrderId(4),
                price: 100,
                qty: 10,
            },
            ExecutionEvent::Filled {
                seq: 5,
                order_id: OrderId(1)
            },
            ExecutionEvent::Expired {
                seq: 5,
                order_id: OrderId(4),
                remaining_qty: 5
            },
            ExecutionEvent::Accepted {
                seq: 6,
                order_id: OrderId(5)
            },
            ExecutionEvent::SelfTradePrevented {
                seq: 7,
                order_id: OrderId(6)
            },
            ExecutionEvent::Cancelled {
                seq: 8,
                order_id: OrderId(5),
                reason: CancelReason::ByParticipant
            },
            ExecutionEvent::Cancelled {
                seq: 9,
                order_id: OrderId(2),
                reason: CancelReason::Requested
            },
        ]
    }
}
