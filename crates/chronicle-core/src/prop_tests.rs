//! Property tests driving the engine and the reference oracle with one
//! generated command stream (ARCHITECTURE.md section 13).
//!
//! After every command the two books must agree on the complete event
//! stream, best bid and ask, and full L2 depth, and the engine must
//! pass its own structural invariant checker. Case volume scales with
//! `PROPTEST_CASES` (1,000 on PRs, 10,000 on main); no case count is
//! hardcoded in the macros.

use proptest::prelude::*;

use crate::book::{Book, DepthLevel};
use crate::config::BookConfig;
use crate::error::BookError;
use crate::events::{EventBuffer, ExecutionEvent};
use crate::reference_book::ReferenceBook;
use crate::types::{OrderId, OrderType, ParticipantId, Side, StpPolicy};

/// One generated command. Prices stay in a tight band around 100 so
/// orders cross often; ids come from a small pool so duplicates and
/// cancels of live orders are common.
#[derive(Clone, Debug)]
enum Op {
    /// Add a Limit order.
    Add {
        /// Order id, from a small pool.
        id: u64,
        /// Participant, from a small pool.
        participant: u32,
        /// True for bid, false for ask.
        is_bid: bool,
        /// Raw price in the crossing band.
        price: u64,
        /// Quantity.
        qty: u64,
    },
    /// Cancel by id.
    Cancel {
        /// Order id, from the same pool.
        id: u64,
    },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u64..=24, 0u32..3, any::<bool>(), 95u64..=106, 1u64..=20).prop_map(
            |(id, participant, is_bid, price, qty)| Op::Add {
                id,
                participant,
                is_bid,
                price,
                qty
            }
        ),
        (1u64..=24).prop_map(|id| Op::Cancel { id }),
    ]
}

fn test_book() -> Book {
    Book::new(BookConfig {
        max_orders: 256,
        grid_len: 4096,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    })
    .expect("valid config")
}

/// Engine depth as plain tuples, for comparison against the oracle.
fn engine_depth(book: &Book, side: Side) -> Vec<(u64, u64, u32)> {
    let mut out = [DepthLevel {
        price: 0,
        total_qty: 0,
        order_count: 0,
    }; 64];
    let n = book.depth(side, 64, &mut out);
    out[..n]
        .iter()
        .map(|d| (d.price, d.total_qty, d.order_count))
        .collect()
}

/// Assert full agreement between engine and oracle right now.
fn assert_agreement(book: &Book, oracle: &ReferenceBook) {
    assert_eq!(book.best_bid(), oracle.best(Side::Bid), "best bid diverged");
    assert_eq!(book.best_ask(), oracle.best(Side::Ask), "best ask diverged");
    assert_eq!(
        engine_depth(book, Side::Bid),
        oracle.depth(Side::Bid),
        "bid depth diverged"
    );
    assert_eq!(
        engine_depth(book, Side::Ask),
        oracle.depth(Side::Ask),
        "ask depth diverged"
    );
    assert_eq!(
        book.live_order_count() as usize,
        oracle.orders.len(),
        "live count diverged"
    );
    book.debug_assert_invariants();
}

proptest! {
    /// INV-BOOK-2 and INV-EVENT-3 at engine scale: the whole event
    /// stream matches the oracle after every command, which pins FIFO
    /// priority, trade prices, and emission order together.
    #[test]
    fn prop_book_matches_reference(ops in prop::collection::vec(op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in ops {
            match op {
                Op::Add { id, participant, is_bid, price, qty } => {
                    let side = if is_bid { Side::Bid } else { Side::Ask };
                    if oracle.contains(id) {
                        let mut storage = Vec::with_capacity(64);
                        let mut events = EventBuffer::new(&mut storage);
                        let err = book
                            .add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, &mut events)
                            .expect_err("duplicate id must be rejected");
                        prop_assert_eq!(err, BookError::DuplicateOrderId { order_id: OrderId(id) });
                    } else {
                        let expected =
                            oracle.add_limit(id, participant, side, price, qty);
                        let mut storage = Vec::with_capacity(1024);
                        let got = {
                            let mut events = EventBuffer::new(&mut storage);
                            book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, &mut events)
                                .expect("valid add");
                            storage.clone()
                        };
                        prop_assert_eq!(got, expected, "event stream diverged");
                    }
                }
                Op::Cancel { id } => {
                    let mut storage = Vec::with_capacity(64);
                    if let Some((expected, _remaining)) = oracle.cancel(id) {
                        let got = {
                            let mut events = EventBuffer::new(&mut storage);
                            book.cancel_order(OrderId(id), &mut events).expect("cancel");
                            storage.clone()
                        };
                        prop_assert_eq!(got, expected, "cancel events diverged");
                    } else {
                        let mut events = EventBuffer::new(&mut storage);
                        let err = book
                            .cancel_order(OrderId(id), &mut events)
                            .expect_err("missing id must be rejected");
                        prop_assert_eq!(err, BookError::OrderNotFound { order_id: OrderId(id) });
                    }
                }
            }
            assert_agreement(&book, &oracle);
        }
    }

    /// INV-BOOK-4: quantity is conserved exactly. Every unit added is
    /// resting, was cancelled while resting, or left in a trade, and a
    /// trade removes the same quantity from the maker side as it
    /// consumes from the taker, so:
    /// added = resting + cancelled + 2 * traded.
    #[test]
    fn prop_trade_quantity_conservation(ops in prop::collection::vec(op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        let mut added: u128 = 0;
        let mut cancelled: u128 = 0;
        let mut traded: u128 = 0;
        for op in ops {
            match op {
                Op::Add { id, participant, is_bid, price, qty } => {
                    if oracle.contains(id) {
                        continue;
                    }
                    let side = if is_bid { Side::Bid } else { Side::Ask };
                    let events = oracle.add_limit(id, participant, side, price, qty);
                    let mut storage = Vec::with_capacity(1024);
                    {
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, &mut buffer)
                            .expect("valid add");
                    }
                    added += u128::from(qty);
                    for event in events {
                        if let ExecutionEvent::Trade { qty: trade_qty, .. } = event {
                            traded += u128::from(trade_qty);
                        }
                    }
                }
                Op::Cancel { id } => {
                    if let Some((_events, remaining)) = oracle.cancel(id) {
                        let mut storage = Vec::with_capacity(64);
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.cancel_order(OrderId(id), &mut buffer).expect("cancel");
                        cancelled += u128::from(remaining);
                    }
                }
            }
            let resting: u128 = [Side::Bid, Side::Ask]
                .iter()
                .flat_map(|side| oracle.depth(*side))
                .map(|(_, qty, _)| u128::from(qty))
                .sum();
            prop_assert_eq!(added, resting + cancelled + 2 * traded, "quantity not conserved");
        }
    }

    /// INV-BITMAP-2: best prices from the bitmap hierarchy equal a
    /// naive scan over every resting order.
    #[test]
    fn prop_best_price_matches_brute_force(ops in prop::collection::vec(op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in ops {
            match op {
                Op::Add { id, participant, is_bid, price, qty } => {
                    if !oracle.contains(id) {
                        let side = if is_bid { Side::Bid } else { Side::Ask };
                        oracle.add_limit(id, participant, side, price, qty);
                        let mut storage = Vec::with_capacity(1024);
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, &mut buffer)
                            .expect("valid add");
                    }
                }
                Op::Cancel { id } => {
                    if oracle.cancel(id).is_some() {
                        let mut storage = Vec::with_capacity(64);
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.cancel_order(OrderId(id), &mut buffer).expect("cancel");
                    }
                }
            }
            prop_assert_eq!(book.best_bid(), oracle.best(Side::Bid));
            prop_assert_eq!(book.best_ask(), oracle.best(Side::Ask));
            book.debug_assert_invariants();
        }
    }
}
