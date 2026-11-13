//! A deliberately simple reference order book, compiled only under
//! `cfg(test)`, used as the property-test oracle.
//!
//! It favours obviousness over speed: orders sit in a `BTreeMap`, and
//! every match step rescans all orders to find the best maker by
//! (price, arrival). It shares no code with the engine, so a bug in the
//! arena, bitmap, or lookup cannot hide by being made twice. It works
//! in raw price space and assumes its inputs were validated (the
//! generators only produce valid, grid-aligned commands).
//!
//! Milestone 2: the oracle implements the full command set
//! independently: every order type, the three self-trade prevention
//! policies, amend, and cancel-by-participant. Where the engine rejects
//! in preflight (`PostOnly` crossing, `Fok` shortfall, amend crossing), the
//! oracle reports the same rejection through [`OracleReject`] and
//! mutates nothing, so the harness can compare errors as well as event
//! streams.

use std::collections::BTreeMap;

use crate::events::{CancelReason, ExecutionEvent};
use crate::types::{OrderId, OrderType, Side, StpPolicy};

/// One resting order in the reference book.
#[derive(Clone, Copy, Debug)]
pub struct RefOrder {
    /// Participant that owns the order.
    pub participant: u32,
    /// Side the order rests on.
    pub side: Side,
    /// Raw price.
    pub price: u64,
    /// Remaining quantity.
    pub qty: u64,
    /// Monotonic arrival counter, the FIFO tiebreak.
    pub arrival: u64,
}

/// A preflight rejection the oracle computes independently, mirroring
/// the engine's typed errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleReject {
    /// `PostOnly` price would cross the opposite best.
    PostOnlyWouldCross {
        /// The raw opposite best price.
        opposite_best: u64,
    },
    /// `Fok` order cannot fill in full; `available` is the STP-adjusted
    /// executable quantity.
    FokInsufficient {
        /// STP-adjusted executable quantity.
        available: u64,
    },
    /// Amend price would cross the opposite best.
    AmendWouldCross {
        /// The raw opposite best price.
        opposite_best: u64,
    },
    /// Cancel or amend named an order that is not live.
    NotFound,
}

/// The oracle book.
#[derive(Default)]
pub struct ReferenceBook {
    /// Live orders by id.
    pub orders: BTreeMap<u64, RefOrder>,
    /// Next arrival counter value.
    arrival: u64,
    /// Next sequence number, mirroring the engine rule (starts at 1,
    /// one per accepted command).
    next_seq: u64,
}

impl ReferenceBook {
    /// An empty oracle.
    pub fn new() -> Self {
        Self {
            orders: BTreeMap::new(),
            arrival: 0,
            next_seq: 1,
        }
    }

    /// True when an order with this id is live.
    pub fn contains(&self, order_id: u64) -> bool {
        self.orders.contains_key(&order_id)
    }

    /// Add a Limit order and return its full event stream. Inputs are
    /// assumed valid and the id unused.
    pub fn add_limit(
        &mut self,
        order_id: u64,
        participant: u32,
        side: Side,
        price: u64,
        qty: u64,
    ) -> Vec<ExecutionEvent> {
        self.add_typed(
            order_id,
            participant,
            side,
            OrderType::Limit,
            price,
            qty,
            StpPolicy::CancelIncoming,
        )
        .expect("a Limit add never rejects in preflight")
    }

    /// Add an order of any type under an already-resolved STP policy
    /// (the harness resolves the engine's `Option` against the book
    /// default before calling). On an `Err` the oracle has mutated
    /// nothing, including its sequence number.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the engine add_order parameter list"
    )]
    pub fn add_typed(
        &mut self,
        order_id: u64,
        participant: u32,
        side: Side,
        order_type: OrderType,
        price: u64,
        qty: u64,
        stp: StpPolicy,
    ) -> Result<Vec<ExecutionEvent>, OracleReject> {
        if order_type == OrderType::PostOnly
            && let Some(best) = self.best(side.opposite())
        {
            let crosses = match side {
                Side::Bid => price >= best.0,
                Side::Ask => price <= best.0,
            };
            if crosses {
                return Err(OracleReject::PostOnlyWouldCross {
                    opposite_best: best.0,
                });
            }
        }
        // Work on a scratch copy so a Fok shortfall mutates nothing,
        // not even the resting orders STP would have cancelled.
        let mut orders = self.orders.clone();
        let seq = self.next_seq;
        let (mut events, remaining, stp_stopped) = if order_type == OrderType::PostOnly {
            (Vec::new(), qty, false)
        } else {
            Self::sweep(
                &mut orders,
                seq,
                order_id,
                participant,
                side,
                price,
                qty,
                stp,
            )
        };
        if order_type == OrderType::Fok && remaining > 0 {
            return Err(OracleReject::FokInsufficient {
                available: qty - remaining,
            });
        }
        self.next_seq += 1;
        if remaining == 0 {
            events.push(ExecutionEvent::Filled {
                seq,
                order_id: OrderId(order_id),
            });
        } else if stp_stopped {
            events.push(ExecutionEvent::SelfTradePrevented {
                seq,
                order_id: OrderId(order_id),
            });
        } else {
            match order_type {
                OrderType::Limit | OrderType::PostOnly => {
                    self.arrival += 1;
                    orders.insert(
                        order_id,
                        RefOrder {
                            participant,
                            side,
                            price,
                            qty: remaining,
                            arrival: self.arrival,
                        },
                    );
                    events.push(ExecutionEvent::Accepted {
                        seq,
                        order_id: OrderId(order_id),
                    });
                }
                OrderType::Ioc => {
                    events.push(ExecutionEvent::Expired {
                        seq,
                        order_id: OrderId(order_id),
                        remaining_qty: remaining,
                    });
                }
                OrderType::Fok => {
                    unreachable!("a Fok order with a remainder rejected above")
                }
            }
        }
        self.orders = orders;
        Ok(events)
    }

    /// The matching sweep over a scratch order map: trades in
    /// price-time order at maker prices, STP applied at own orders.
    /// Returns the sweep events, the taker remainder, and whether STP
    /// stopped the sweep.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the engine add_order parameter list"
    )]
    fn sweep(
        orders: &mut BTreeMap<u64, RefOrder>,
        seq: u64,
        order_id: u64,
        participant: u32,
        side: Side,
        price: u64,
        qty: u64,
        stp: StpPolicy,
    ) -> (Vec<ExecutionEvent>, u64, bool) {
        let mut events = Vec::new();
        let mut remaining = qty;
        let mut stp_stopped = false;
        while remaining > 0 {
            let Some(maker_id) = Self::best_maker_in(orders, side, price) else {
                break;
            };
            let maker = *orders.get(&maker_id).expect("maker exists");
            if maker.participant == participant {
                match stp {
                    StpPolicy::CancelIncoming => {
                        stp_stopped = true;
                        break;
                    }
                    StpPolicy::CancelResting => {
                        orders.remove(&maker_id);
                        events.push(ExecutionEvent::Cancelled {
                            seq,
                            order_id: OrderId(maker_id),
                            reason: CancelReason::SelfTradePrevention,
                        });
                        continue;
                    }
                    StpPolicy::CancelBoth => {
                        orders.remove(&maker_id);
                        events.push(ExecutionEvent::Cancelled {
                            seq,
                            order_id: OrderId(maker_id),
                            reason: CancelReason::SelfTradePrevention,
                        });
                        stp_stopped = true;
                        break;
                    }
                }
            }
            let trade_qty = remaining.min(maker.qty);
            remaining -= trade_qty;
            events.push(ExecutionEvent::Trade {
                seq,
                maker_order_id: OrderId(maker_id),
                taker_order_id: OrderId(order_id),
                price: maker.price,
                qty: trade_qty,
            });
            if trade_qty == maker.qty {
                orders.remove(&maker_id);
                events.push(ExecutionEvent::Filled {
                    seq,
                    order_id: OrderId(maker_id),
                });
            } else if let Some(m) = orders.get_mut(&maker_id) {
                m.qty -= trade_qty;
            }
        }
        (events, remaining, stp_stopped)
    }

    /// Amend a live order. Inputs are assumed validated. Amending to a
    /// larger quantity or any different price moves the order behind
    /// every order already resting at the target level (a fresh arrival
    /// stamp); a reduction or an identical amend keeps its stamp.
    pub fn amend(
        &mut self,
        order_id: u64,
        new_price: u64,
        new_qty: u64,
    ) -> Result<Vec<ExecutionEvent>, OracleReject> {
        let order = *self
            .orders
            .get(&order_id)
            .ok_or(OracleReject::NotFound)?;
        if let Some(best) = self.best(order.side.opposite()) {
            let crosses = match order.side {
                Side::Bid => new_price >= best.0,
                Side::Ask => new_price <= best.0,
            };
            if crosses {
                return Err(OracleReject::AmendWouldCross {
                    opposite_best: best.0,
                });
            }
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let entry = self.orders.get_mut(&order_id).expect("checked above");
        if new_price != order.price || new_qty > order.qty {
            self.arrival += 1;
            entry.arrival = self.arrival;
        }
        entry.price = new_price;
        entry.qty = new_qty;
        Ok(vec![ExecutionEvent::Amended {
            seq,
            order_id: OrderId(order_id),
            price: new_price,
            qty: new_qty,
        }])
    }

    /// Cancel every live order of one participant. The command consumes
    /// a sequence number even when nothing is cancelled. The oracle
    /// emits in ascending id order; the engine emits in ascending slot
    /// order, and the harness compares the two as sets (INV-CANCEL-1).
    pub fn cancel_by_participant(&mut self, participant: u32) -> Vec<ExecutionEvent> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let ids: Vec<u64> = self
            .orders
            .iter()
            .filter(|(_, o)| o.participant == participant)
            .map(|(id, _)| *id)
            .collect();
        let mut events = Vec::new();
        for id in ids {
            self.orders.remove(&id);
            events.push(ExecutionEvent::Cancelled {
                seq,
                order_id: OrderId(id),
                reason: CancelReason::ByParticipant,
            });
        }
        events
    }

    /// Cancel a live order, returning its events and its remaining
    /// quantity at cancel time.
    pub fn cancel(&mut self, order_id: u64) -> Option<(Vec<ExecutionEvent>, u64)> {
        let order = self.orders.remove(&order_id)?;
        let seq = self.next_seq;
        self.next_seq += 1;
        Some((
            vec![ExecutionEvent::Cancelled {
                seq,
                order_id: OrderId(order_id),
                reason: CancelReason::Requested,
            }],
            order.qty,
        ))
    }

    /// Find the best maker for an incoming order in an arbitrary order
    /// map: on the opposite side, crossing the limit price, best price
    /// first, then earliest arrival.
    fn best_maker_in(
        orders: &BTreeMap<u64, RefOrder>,
        taker_side: Side,
        taker_price: u64,
    ) -> Option<u64> {
        let mut best: Option<(u64, u64, u64)> = None; // (price key, arrival, id)
        for (id, order) in orders {
            if order.side == taker_side {
                continue;
            }
            let crosses = match taker_side {
                Side::Bid => order.price <= taker_price,
                Side::Ask => order.price >= taker_price,
            };
            if !crosses {
                continue;
            }
            // Price key sorts best first for the taker's perspective:
            // a buyer wants the lowest ask, a seller the highest bid.
            let key = match taker_side {
                Side::Bid => order.price,
                Side::Ask => u64::MAX - order.price,
            };
            let candidate = (key, order.arrival, *id);
            if best.is_none_or(|b| candidate < b) {
                best = Some(candidate);
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// Best price and aggregate quantity for one side, by naive scan.
    pub fn best(&self, side: Side) -> Option<(u64, u64)> {
        let mut prices: Vec<u64> = self
            .orders
            .values()
            .filter(|o| o.side == side)
            .map(|o| o.price)
            .collect();
        prices.sort_unstable();
        let price = match side {
            Side::Bid => prices.last().copied(),
            Side::Ask => prices.first().copied(),
        }?;
        let qty = self
            .orders
            .values()
            .filter(|o| o.side == side && o.price == price)
            .map(|o| o.qty)
            .sum();
        Some((price, qty))
    }

    /// Full depth for one side as (price, total qty, order count),
    /// best price first, by naive grouping.
    pub fn depth(&self, side: Side) -> Vec<(u64, u64, u32)> {
        let mut levels: BTreeMap<u64, (u64, u32)> = BTreeMap::new();
        for order in self.orders.values().filter(|o| o.side == side) {
            let entry = levels.entry(order.price).or_insert((0, 0));
            entry.0 += order.qty;
            entry.1 += 1;
        }
        let mut out: Vec<(u64, u64, u32)> = levels
            .into_iter()
            .map(|(price, (qty, count))| (price, qty, count))
            .collect();
        if side == Side::Bid {
            out.reverse();
        }
        out
    }
}
