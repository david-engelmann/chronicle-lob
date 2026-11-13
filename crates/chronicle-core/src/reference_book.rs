//! A deliberately simple reference order book, compiled only under
//! `cfg(test)`, used as the property-test oracle.
//!
//! It favours obviousness over speed: orders sit in a `BTreeMap`, and
//! every match step rescans all orders to find the best maker by
//! (price, arrival). It shares no code with the engine, so a bug in the
//! arena, bitmap, or lookup cannot hide by being made twice. It works
//! in raw price space and assumes its inputs were validated (the
//! generators only produce valid, grid-aligned commands).

use std::collections::BTreeMap;

use crate::events::{CancelReason, ExecutionEvent};
use crate::types::{OrderId, Side};

/// One resting order in the reference book.
#[derive(Clone, Copy, Debug)]
pub struct RefOrder {
    /// Participant that owns the order. Read from Milestone 2, when
    /// the oracle grows self-trade prevention paths.
    #[allow(dead_code, reason = "consumed by the Milestone 2 STP oracle paths")]
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
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut events = Vec::new();
        let mut remaining = qty;
        while remaining > 0 {
            let Some(maker_id) = self.best_maker(side, price) else {
                break;
            };
            let maker = self.orders.get_mut(&maker_id).expect("maker exists");
            let trade_qty = remaining.min(maker.qty);
            let maker_price = maker.price;
            maker.qty -= trade_qty;
            remaining -= trade_qty;
            let maker_filled = maker.qty == 0;
            events.push(ExecutionEvent::Trade {
                seq,
                maker_order_id: OrderId(maker_id),
                taker_order_id: OrderId(order_id),
                price: maker_price,
                qty: trade_qty,
            });
            if maker_filled {
                self.orders.remove(&maker_id);
                events.push(ExecutionEvent::Filled {
                    seq,
                    order_id: OrderId(maker_id),
                });
            }
        }
        if remaining == 0 {
            events.push(ExecutionEvent::Filled {
                seq,
                order_id: OrderId(order_id),
            });
        } else {
            self.arrival += 1;
            self.orders.insert(
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

    /// Find the best maker for an incoming order: on the opposite side,
    /// crossing the limit price, best price first, then earliest
    /// arrival.
    fn best_maker(&self, taker_side: Side, taker_price: u64) -> Option<u64> {
        let mut best: Option<(u64, u64, u64)> = None; // (price key, arrival, id)
        for (id, order) in &self.orders {
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
