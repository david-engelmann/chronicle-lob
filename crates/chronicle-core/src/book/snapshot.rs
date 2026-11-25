//! Snapshot capture and restore (ARCHITECTURE.md sections 10.2 and
//! 10.4). Capture reads the arena, the level arrays, and the free list
//! into the plain [`SnapshotState`] transfer shape. Restore rebuilds a
//! book from a state that `chronicle-wal` has already validated byte
//! by byte, reproducing slot assignment, FIFO links, generations, and
//! the exact free list, so a restored book is indistinguishable from
//! the captured one, including in the order `cancel_by_participant`
//! emits its events and in the slots later allocations take.

use crate::arena::{NIL, STATE_FREE, STATE_LIVE, STATE_RETIRED};
use crate::config::BookConfig;
use crate::error::BookError;
use crate::types::{LevelState, LiveOrderState, OrderId, ParticipantId, Side, SnapshotState};

use super::Book;

impl Book {
    /// Capture the complete structural state of this book for
    /// persistence. Live orders are reported in ascending slot order;
    /// the free list is reported exactly, head and per-slot links, so
    /// a restore allocates slots in the same order this book would
    /// have.
    #[must_use]
    pub fn capture_snapshot_state(&self) -> SnapshotState {
        let slots = self.config.max_orders as usize;
        let mut generations = Vec::with_capacity(slots);
        let mut free_links = Vec::with_capacity(slots);
        let mut live_orders = Vec::with_capacity(self.arena.live_count() as usize);
        for s in 0..slots {
            generations.push(self.arena.generation[s]);
            if self.arena.state[s] == STATE_LIVE {
                free_links.push(NIL);
                live_orders.push(LiveOrderState {
                    slot: u32::try_from(s).unwrap_or(NIL),
                    generation: self.arena.generation[s],
                    order_id: OrderId(self.arena.order_id[s]),
                    participant: ParticipantId(self.arena.participant[s]),
                    price_idx: self.arena.price_idx[s],
                    qty: self.arena.qty[s],
                    side: Side::from_u8(self.arena.side[s]),
                    prev: self.arena.prev[s],
                    next: self.arena.next[s],
                });
            } else {
                // A free slot's `next` column is its free list link; a
                // retired slot's is NIL by arena construction. Both
                // are exactly what the format persists.
                free_links.push(self.arena.next[s]);
            }
        }
        let levels = (0..self.config.grid_len as usize)
            .map(|i| LevelState {
                head: self.levels.head[i],
                tail: self.levels.tail[i],
                qty: self.levels.qty[i],
                count: self.levels.count[i],
            })
            .collect();
        SnapshotState {
            config: self.config,
            next_seq: self.next_seq,
            generations,
            free_list_head: self.arena.free_head(),
            free_links,
            levels,
            live_orders,
        }
    }

    /// Rebuild a book from a captured state. Every structural invariant
    /// (configuration, sequence number, section lengths, generations,
    /// live record bounds and uniqueness, free-list exactness and
    /// cycle-freedom, symmetric acyclic level lists, exact level
    /// aggregates, single side per level, and an uncrossed book) is
    /// validated unconditionally in both debug and release builds
    /// before returning the rebuilt book.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::InvalidConfig`] when the state's own
    /// configuration fails validation, [`BookError::LookupCapacityFull`]
    /// if the lookup load cap is exceeded, and
    /// [`BookError::InvalidSnapshot`] when any structural invariant of
    /// [`SnapshotState`] fails.
    pub fn from_snapshot_state(state: SnapshotState) -> Result<Self, BookError> {
        let SnapshotState {
            config,
            next_seq,
            generations,
            free_list_head,
            free_links,
            levels,
            live_orders,
        } = state;
        config.validate()?;
        let _ = config.grid_end_tick()?;
        if next_seq == 0 {
            return Err(BookError::InvalidSnapshot {
                reason: "next_seq must be at least 1",
            });
        }
        let max_orders = config.max_orders;
        let grid_len = config.grid_len;
        let slots = max_orders as usize;
        if generations.len() != slots || free_links.len() != slots {
            return Err(BookError::InvalidSnapshot {
                reason: "generation and free link sections must cover every slot",
            });
        }
        if levels.len() != grid_len as usize {
            return Err(BookError::InvalidSnapshot {
                reason: "the level section must cover the whole grid",
            });
        }
        if live_orders.len() > slots {
            return Err(BookError::InvalidSnapshot {
                reason: "live order count exceeds max_orders",
            });
        }
        if generations.contains(&0) {
            return Err(BookError::InvalidSnapshot {
                reason: "slot generation must be at least 1",
            });
        }
        let is_live = check_live_records(&live_orders, &generations, &free_links, &config)?;
        let (on_chain, free_len) = walk_free_chain(
            &free_links,
            &generations,
            &is_live,
            free_list_head,
            max_orders,
        )?;
        let mut book = Self::new(config)?;
        // Order ids must be unique; the lookup is the witness, and it
        // ends up holding exactly the live set.
        for record in &live_orders {
            if book.lookup.get(record.order_id.0).is_some() {
                return Err(BookError::InvalidSnapshot {
                    reason: "live order ids must be unique",
                });
            }
            book.lookup
                .insert(record.order_id.0, record.slot, record.generation)?;
        }
        // Generations for every slot, live and free and retired.
        book.arena.generation.copy_from_slice(&generations);
        // Live records into the arena columns, links included.
        for record in &live_orders {
            let s = record.slot as usize;
            book.arena.state[s] = STATE_LIVE;
            book.arena.order_id[s] = record.order_id.0;
            book.arena.participant[s] = record.participant.0;
            book.arena.price_idx[s] = record.price_idx;
            book.arena.qty[s] = record.qty;
            book.arena.side[s] = record.side.as_u8();
            book.arena.prev[s] = record.prev;
            book.arena.next[s] = record.next;
        }
        // Non-live slots: on the persisted chain means free, with the
        // persisted link; off it means retired (section 10.4's restore
        // rule, with the state the byte format cannot carry derived
        // from chain membership).
        for s in 0..slots {
            if is_live[s] {
                continue;
            }
            if on_chain[s] {
                book.arena.state[s] = STATE_FREE;
                book.arena.next[s] = free_links[s];
            } else {
                book.arena.state[s] = STATE_RETIRED;
                book.arena.next[s] = NIL;
            }
            book.arena.prev[s] = NIL;
        }
        let live = u32::try_from(live_orders.len()).unwrap_or(0);
        book.arena
            .restore_bookkeeping(free_list_head, free_len, live);
        validate_and_restore_levels(&mut book, &levels, &is_live, live_orders.len())?;
        book.next_seq = next_seq;
        if cfg!(debug_assertions) {
            book.debug_assert_invariants();
        }
        Ok(book)
    }
}

/// Walk a persisted free chain once, returning which slots it
/// visits and how many there are. The chain must stay inside the
/// slot range, avoid live slots, visit no slot twice, and account
/// for every non-live non-retired slot.
fn walk_free_chain(
    free_links: &[u32],
    generations: &[u32],
    is_live: &[bool],
    head: u32,
    max_orders: u32,
) -> Result<(Vec<bool>, u32), BookError> {
    let mut on_chain = vec![false; max_orders as usize];
    let mut free_len = 0u32;
    let mut cursor = head;
    while cursor != NIL {
        if cursor >= max_orders {
            return Err(BookError::InvalidSnapshot {
                reason: "free list link leaves the slot range",
            });
        }
        let idx = cursor as usize;
        if is_live[idx] {
            return Err(BookError::InvalidSnapshot {
                reason: "free list visits a live slot",
            });
        }
        if on_chain[idx] {
            return Err(BookError::InvalidSnapshot {
                reason: "free list contains a cycle",
            });
        }
        on_chain[idx] = true;
        free_len += 1;
        cursor = free_links[idx];
    }
    for s in 0..max_orders as usize {
        if !is_live[s] && !on_chain[s] {
            if generations[s] != u32::MAX {
                return Err(BookError::InvalidSnapshot {
                    reason: "free slot is missing from the free list",
                });
            }
            if free_links[s] != NIL {
                return Err(BookError::InvalidSnapshot {
                    reason: "retired slot carries a non-NIL free link",
                });
            }
        }
    }
    Ok((on_chain, free_len))
}

/// Validate the live records of a snapshot state, returning which
/// slots they occupy. Records must appear in strictly ascending
/// slot order, carry positive order ids and quantities, name
/// in-range slots, prices, and neighbour links, carry NIL free
/// links, and agree with the slot's persisted generation.
fn check_live_records(
    live_orders: &[LiveOrderState],
    generations: &[u32],
    free_links: &[u32],
    config: &BookConfig,
) -> Result<Vec<bool>, BookError> {
    let max_orders = config.max_orders;
    let grid_len = config.grid_len;
    let mut is_live = vec![false; max_orders as usize];
    let mut previous_slot: Option<u32> = None;
    for record in live_orders {
        if record.slot >= max_orders
            || record.price_idx >= grid_len
            || (config.grid_start_tick == 0 && record.price_idx == 0)
        {
            return Err(BookError::InvalidSnapshot {
                reason: "live records must name an in-range slot and positive price",
            });
        }
        if let Some(prev_slot) = previous_slot
            && record.slot <= prev_slot
        {
            return Err(BookError::InvalidSnapshot {
                reason: "live records must be unique and in ascending slot order",
            });
        }
        if record.order_id.0 == 0 {
            return Err(BookError::InvalidSnapshot {
                reason: "live order id must be non-zero",
            });
        }
        if record.qty == 0 {
            return Err(BookError::InvalidSnapshot {
                reason: "live order quantity must be positive",
            });
        }
        if (record.prev != NIL && record.prev >= max_orders)
            || (record.next != NIL && record.next >= max_orders)
        {
            return Err(BookError::InvalidSnapshot {
                reason: "live order prev and next links must be NIL or in-range",
            });
        }
        let s = record.slot as usize;
        if record.generation != generations[s] {
            return Err(BookError::InvalidSnapshot {
                reason: "live record generation disagrees with slot generation",
            });
        }
        if free_links[s] != NIL {
            return Err(BookError::InvalidSnapshot {
                reason: "live slot carries a non-NIL free link",
            });
        }
        is_live[s] = true;
        previous_slot = Some(record.slot);
    }
    Ok(is_live)
}

/// Walk and validate one non-empty price level list, returning the
/// side of its orders.
fn walk_level_list(
    arena: &crate::arena::Arena,
    idx: u32,
    level: &LevelState,
    is_live: &[bool],
    seen_in_levels: &mut [bool],
) -> Result<Side, BookError> {
    let max_orders = arena.capacity();
    if level.head >= max_orders || level.tail >= max_orders {
        return Err(BookError::InvalidSnapshot {
            reason: "non-empty level must name in-range head and tail slots",
        });
    }
    let mut cursor = level.head;
    let mut expected_prev = NIL;
    let mut walked_count = 0u32;
    let mut walked_qty = 0u64;
    let mut level_side: Option<Side> = None;
    while cursor != NIL {
        if cursor >= max_orders || !is_live[cursor as usize] {
            return Err(BookError::InvalidSnapshot {
                reason: "level list visits a non-live or out-of-range slot",
            });
        }
        let s = cursor as usize;
        if seen_in_levels[s] {
            return Err(BookError::InvalidSnapshot {
                reason: "level list contains a cycle or shared slot",
            });
        }
        seen_in_levels[s] = true;
        if arena.price_idx[s] != idx {
            return Err(BookError::InvalidSnapshot {
                reason: "live order price_idx disagrees with its level",
            });
        }
        if arena.prev[s] != expected_prev {
            return Err(BookError::InvalidSnapshot {
                reason: "level list prev link is asymmetric",
            });
        }
        let side = Side::from_u8(arena.side[s]);
        match level_side {
            None => level_side = Some(side),
            Some(existing) if existing != side => {
                return Err(BookError::InvalidSnapshot {
                    reason: "level mixes bid and ask orders",
                });
            }
            Some(_) => {}
        }
        walked_count += 1;
        walked_qty = walked_qty
            .checked_add(arena.qty[s])
            .ok_or(BookError::InvalidSnapshot {
                reason: "level quantity overflows u64",
            })?;
        expected_prev = cursor;
        cursor = arena.next[s];
    }
    if level.tail != expected_prev || level.count != walked_count || level.qty != walked_qty {
        return Err(BookError::InvalidSnapshot {
            reason: "level tail, count, or quantity disagrees with list walk",
        });
    }
    Ok(level_side.unwrap_or(Side::Bid))
}

/// Validate every price level's doubly-linked list, cached count,
/// cached quantity, single-side membership, total live reachability,
/// and the uncrossed-book invariant before installing levels and
/// bitmaps into `book`.
fn validate_and_restore_levels(
    book: &mut Book,
    levels: &[LevelState],
    is_live: &[bool],
    expected_live: usize,
) -> Result<(), BookError> {
    let mut seen_in_levels = vec![false; book.config.max_orders as usize];
    let mut total_walked = 0usize;
    for (i, level) in levels.iter().enumerate() {
        let idx = u32::try_from(i).unwrap_or(0);
        if level.count == 0 {
            if level.head != NIL || level.tail != NIL || level.qty != 0 {
                return Err(BookError::InvalidSnapshot {
                    reason: "empty level must have NIL ends and zero quantity",
                });
            }
            continue;
        }
        let side = walk_level_list(&book.arena, idx, level, is_live, &mut seen_in_levels)?;
        book.levels.head[i] = level.head;
        book.levels.tail[i] = level.tail;
        book.levels.qty[i] = level.qty;
        book.levels.count[i] = level.count;
        match side {
            Side::Bid => book.bids.set(idx),
            Side::Ask => book.asks.set(idx),
        }
        total_walked += level.count as usize;
    }
    if total_walked != expected_live {
        return Err(BookError::InvalidSnapshot {
            reason: "live order is not reachable from any level list",
        });
    }
    if let (Some((bid_price, _)), Some((ask_price, _))) = (book.best_bid(), book.best_ask())
        && bid_price >= ask_price
    {
        return Err(BookError::InvalidSnapshot {
            reason: "snapshot book is crossed",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::BookConfig;
    use crate::events::{EventBuffer, ExecutionEvent};
    use crate::types::{OrderType, StpPolicy};

    use super::*;

    fn test_config() -> BookConfig {
        BookConfig {
            max_orders: 16,
            grid_len: 640,
            tick_size: 1,
            grid_start_tick: 1,
            default_stp: StpPolicy::CancelIncoming,
        }
    }

    fn add(book: &mut Book, id: u64, participant: u32, side: Side, price: u64, qty: u64) {
        let mut storage = Vec::with_capacity(64);
        let mut events = EventBuffer::new(&mut storage);
        book.add_order(
            OrderId(id),
            ParticipantId(participant),
            side,
            OrderType::Limit,
            price,
            qty,
            None,
            &mut events,
        )
        .expect("add");
    }

    fn cancel(book: &mut Book, id: u64) {
        let mut storage = Vec::with_capacity(4);
        let mut events = EventBuffer::new(&mut storage);
        book.cancel_order(OrderId(id), &mut events).expect("cancel");
    }

    /// A book with churn: fills that free slots, a cancel, and reused
    /// slots, so generations, the free list, and FIFO links are all
    /// nontrivial.
    fn churned_book() -> Book {
        let mut book = Book::new(test_config()).expect("valid config");
        add(&mut book, 1, 1, Side::Bid, 100, 10);
        add(&mut book, 2, 1, Side::Bid, 100, 5);
        add(&mut book, 3, 2, Side::Ask, 105, 7);
        add(&mut book, 4, 2, Side::Ask, 100, 4); // fills 4 of order 1
        cancel(&mut book, 2); // frees slot 1, generation bumps
        add(&mut book, 5, 3, Side::Bid, 99, 3); // reuses slot 1 (LIFO)
        add(&mut book, 6, 3, Side::Ask, 106, 2);
        book
    }

    #[test]
    fn capture_of_empty_book_is_exact() {
        let book = Book::new(test_config()).expect("valid config");
        let state = book.capture_snapshot_state();
        assert_eq!(state.next_seq, 1);
        assert_eq!(state.generations, vec![1; 16]);
        assert_eq!(state.free_list_head, 0);
        // The fresh free list is ascending: slot i links to slot i + 1.
        for (i, link) in state.free_links.iter().enumerate() {
            let expected = if i + 1 < 16 {
                u32::try_from(i + 1).unwrap_or(NIL)
            } else {
                NIL
            };
            assert_eq!(*link, expected, "free link of slot {i}");
        }
        assert_eq!(state.levels.len(), 640);
        assert!(
            state
                .levels
                .iter()
                .all(|l| l.head == NIL && l.tail == NIL && l.qty == 0 && l.count == 0)
        );
        assert_eq!(state.live_orders.len(), 0);
    }

    #[test]
    fn capture_reports_live_orders_in_slot_order() {
        let book = churned_book();
        let state = book.capture_snapshot_state();
        let slots: Vec<u32> = state.live_orders.iter().map(|o| o.slot).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        assert_eq!(slots, sorted, "live records are in ascending slot order");
        // Order 5 reused the slot order 2 was cancelled from, with the
        // bumped generation.
        let reused = state
            .live_orders
            .iter()
            .find(|o| o.order_id == OrderId(5))
            .expect("order 5 is live");
        assert_eq!(reused.slot, 1);
        assert_eq!(reused.generation, 2);
        assert_eq!(state.generations[1], 2);
        // Live slots carry NIL free links; the two orders at price 100
        // on the bid side are linked head to tail in arrival order.
        for order in &state.live_orders {
            assert_eq!(state.free_links[order.slot as usize], NIL);
        }
        let level = state.levels[99]; // price 100 at tick size 1, start 1
        assert_eq!(level.count, 1);
        assert_eq!(level.qty, 6, "order 1 rests with 6 after filling 4");
    }

    #[test]
    fn capture_restore_roundtrip() {
        let book = churned_book();
        let state = book.capture_snapshot_state();
        let restored = Book::from_snapshot_state(state.clone()).expect("restore");
        restored.debug_assert_invariants();
        assert_eq!(restored.capture_snapshot_state(), state);
        assert_eq!(restored.next_seq(), book.next_seq());
        assert_eq!(restored.live_order_count(), book.live_order_count());
        assert_eq!(restored.best_bid(), book.best_bid());
        assert_eq!(restored.best_ask(), book.best_ask());
    }

    #[test]
    fn restore_preserves_allocation_and_event_order() {
        // INV-SNAP-3 and INV-SNAP-4 at unit scale: after restore, the
        // same commands produce the same events and the same slot
        // assignments as on the original book.
        let mut original = churned_book();
        let state = original.capture_snapshot_state();
        let mut restored = Book::from_snapshot_state(state).expect("restore");
        let run = |book: &mut Book| -> Vec<ExecutionEvent> {
            let mut storage = Vec::with_capacity(64);
            {
                let mut events = EventBuffer::new(&mut storage);
                book.cancel_by_participant(ParticipantId(1), &mut events)
                    .expect("mass cancel");
            }
            add(book, 7, 1, Side::Bid, 100, 5);
            add(book, 8, 2, Side::Ask, 104, 5);
            storage
        };
        let original_events = run(&mut original);
        let restored_events = run(&mut restored);
        assert_eq!(restored_events, original_events);
        assert_eq!(
            restored.capture_snapshot_state(),
            original.capture_snapshot_state(),
            "post-restore behaviour, slots included, matches the original"
        );
    }

    #[test]
    fn book_construction_allocates_levels_and_lookup_once() {
        // INV-ALLOC-2 at book level: construction allocates each
        // arena column, level array, and lookup column exactly
        // once, at the configured capacity. The arena-level half is
        // the arena unit test of the same invariant, and the
        // 1,000,000-operation counter proof (INV-ALLOC-1) covers
        // the no-growth half at full capacity.
        let cfg = test_config();
        let book = Book::new(cfg).expect("valid config builds");
        let max_orders = cfg.max_orders as usize;
        let grid_len = cfg.grid_len as usize;
        assert_eq!(book.arena.generation.capacity(), max_orders);
        assert_eq!(book.arena.state.capacity(), max_orders);
        assert_eq!(book.arena.order_id.capacity(), max_orders);
        assert_eq!(book.arena.participant.capacity(), max_orders);
        assert_eq!(book.arena.price_idx.capacity(), max_orders);
        assert_eq!(book.arena.qty.capacity(), max_orders);
        assert_eq!(book.arena.side.capacity(), max_orders);
        assert_eq!(book.arena.prev.capacity(), max_orders);
        assert_eq!(book.arena.next.capacity(), max_orders);
        assert_eq!(book.levels.head.capacity(), grid_len);
        assert_eq!(book.levels.tail.capacity(), grid_len);
        assert_eq!(book.levels.qty.capacity(), grid_len);
        assert_eq!(book.levels.count.capacity(), grid_len);
        // The lookup table is the smallest power of two holding
        // twice the order capacity (the 50 percent load cap).
        assert_eq!(book.lookup.capacity(), (2 * max_orders).next_power_of_two());
    }

    #[test]
    fn from_snapshot_state_rejects_malformed_states_unconditionally() {
        let valid = churned_book().capture_snapshot_state();

        // next_seq == 0 rejected.
        let mut bad = valid.clone();
        bad.next_seq = 0;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        // Out-of-bounds level.head cannot panic on indexing.
        let mut bad = valid.clone();
        bad.levels[99].head = bad.config.max_orders;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        // Self-loop next = slot rejected without hanging.
        let mut bad = valid.clone();
        let slot = bad.live_orders[0].slot;
        bad.live_orders[0].next = slot;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        // Zero order_id, zero qty, and out-of-bounds prev/next rejected.
        let mut bad = valid.clone();
        bad.live_orders[0].order_id = OrderId(0);
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        let mut bad = valid.clone();
        bad.live_orders[0].qty = 0;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        let mut bad = valid.clone();
        bad.live_orders[0].prev = bad.config.max_orders;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        let mut bad = valid.clone();
        bad.live_orders[0].next = bad.config.max_orders;
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));

        // Crossed book rejected.
        let mut bad = valid;
        for order in &mut bad.live_orders {
            if order.side == Side::Ask {
                let old_idx = order.price_idx as usize;
                let crossed_idx = 50usize; // below bid at 98..99
                bad.levels[crossed_idx] = bad.levels[old_idx];
                bad.levels[old_idx] = LevelState {
                    head: NIL,
                    tail: NIL,
                    qty: 0,
                    count: 0,
                };
                order.price_idx = u32::try_from(crossed_idx).unwrap();
                break;
            }
        }
        assert!(matches!(
            Book::from_snapshot_state(bad),
            Err(BookError::InvalidSnapshot { .. })
        ));
    }
}
