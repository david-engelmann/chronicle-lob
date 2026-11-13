//! Property tests driving the engine and the reference oracle with
//! one generated command stream (ARCHITECTURE.md section 13).
//!
//! After every command the two books must agree on the complete event
//! stream, best bid and ask, and full L2 depth, and the engine must
//! pass its own structural invariant checker. Case volume scales with
//! `PROPTEST_CASES` (1,000 on PRs, 10,000 on main); no case count is
//! hardcoded in the macros.
//!
//! Milestone 2 widens the generated stream to the full command
//! surface: every order type, explicit STP policies (and the book
//! default), amend, and cancel-by-participant. The oracle implements
//! the same semantics independently, including its preflight
//! rejections, so streams and errors are compared, never sampled.

use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;

use crate::book::{Book, DepthLevel};
use crate::config::BookConfig;
use crate::error::BookError;
use crate::events::{CancelReason, EventBuffer, ExecutionEvent};
use crate::reference_book::{OracleReject, ReferenceBook};
use crate::types::{OrderId, OrderType, ParticipantId, Side, StpPolicy};

/// The default STP policy of [`test_book`], used to resolve an add's
/// `None` policy for the oracle.
const DEFAULT_STP: StpPolicy = StpPolicy::CancelIncoming;

/// One generated command. Prices stay in a tight band around 100 so
/// orders cross often; ids come from a small pool so duplicates,
/// self-trades, and cancels of live orders are all common.
#[derive(Clone, Debug)]
enum Op {
    /// Add an order of any type.
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
        /// Order type code: 0-3 `Limit`, 4 `Ioc`, 5 `Fok`, 6 `PostOnly`.
        kind: u8,
        /// STP code: 0 none (book default), 1 `CancelIncoming`,
        /// 2 `CancelResting`, 3 `CancelBoth`.
        stp_code: u8,
    },
    /// Cancel by id.
    Cancel {
        /// Order id, from the same pool.
        id: u64,
    },
    /// Amend a (possibly missing) order.
    Amend {
        /// Order id, from the same pool.
        id: u64,
        /// New raw price in the crossing band.
        new_price: u64,
        /// New quantity.
        new_qty: u64,
    },
    /// Cancel every order of one participant.
    CancelByParticipant {
        /// Participant, from the same pool.
        participant: u32,
    },
}

fn kind_of(code: u8) -> OrderType {
    match code {
        4 => OrderType::Ioc,
        5 => OrderType::Fok,
        6 => OrderType::PostOnly,
        _ => OrderType::Limit,
    }
}

fn stp_of(code: u8) -> Option<StpPolicy> {
    match code {
        1 => Some(StpPolicy::CancelIncoming),
        2 => Some(StpPolicy::CancelResting),
        3 => Some(StpPolicy::CancelBoth),
        _ => None,
    }
}

fn add_strategy() -> impl Strategy<Value = Op> {
    (
        1u64..=24,
        0u32..3,
        any::<bool>(),
        95u64..=106,
        1u64..=20,
        0u8..=6,
        0u8..=3,
    )
        .prop_map(
            |(id, participant, is_bid, price, qty, kind, stp_code)| Op::Add {
                id,
                participant,
                is_bid,
                price,
                qty,
                kind,
                stp_code,
            },
        )
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => add_strategy(),
        2 => (1u64..=24).prop_map(|id| Op::Cancel { id }),
        2 => (1u64..=24, 95u64..=106, 1u64..=20).prop_map(|(id, new_price, new_qty)| Op::Amend {
            id,
            new_price,
            new_qty,
        }),
        1 => (0u32..3).prop_map(|participant| Op::CancelByParticipant { participant }),
    ]
}

/// Limit adds and cancels only: the Milestone 1 stream shape, kept for
/// the conservation and best-price properties whose accounting is
/// stated over plain Limit flow.
fn limit_op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u64..=24, 0u32..3, any::<bool>(), 95u64..=106, 1u64..=20).prop_map(
            |(id, participant, is_bid, price, qty)| Op::Add {
                id,
                participant,
                is_bid,
                price,
                qty,
                kind: 0,
                stp_code: 0,
            }
        ),
        (1u64..=24).prop_map(|id| Op::Cancel { id }),
    ]
}

/// Adds (Limit), cancels, and amends: the storm stream of INV-BOOK-1.
fn storm_op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (1u64..=24, 0u32..3, any::<bool>(), 95u64..=106, 1u64..=20).prop_map(
            |(id, participant, is_bid, price, qty)| Op::Add {
                id,
                participant,
                is_bid,
                price,
                qty,
                kind: 0,
                stp_code: 0,
            }
        ),
        2 => (1u64..=24).prop_map(|id| Op::Cancel { id }),
        3 => (1u64..=24, 95u64..=106, 1u64..=20).prop_map(|(id, new_price, new_qty)| Op::Amend {
            id,
            new_price,
            new_qty,
        }),
    ]
}

fn test_book() -> Book {
    Book::new(BookConfig {
        max_orders: 256,
        grid_len: 4096,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: DEFAULT_STP,
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

/// The full observable engine state tuple used by `state_of`.
type StateTuple = (
    u64,
    u32,
    Option<(u64, u64)>,
    Option<(u64, u64)>,
    Vec<(u64, u64, u32)>,
    Vec<(u64, u64, u32)>,
);

/// Full observable engine state, for atomicity assertions.
fn state_of(book: &Book) -> StateTuple {
    (
        book.next_seq(),
        book.live_order_count(),
        book.best_bid(),
        book.best_ask(),
        engine_depth(book, Side::Bid),
        engine_depth(book, Side::Ask),
    )
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

/// Run one command against the engine with a buffer of the given
/// capacity and return exactly what the engine returned.
fn run_op(book: &mut Book, op: &Op, capacity: usize) -> Result<Vec<ExecutionEvent>, BookError> {
    let mut storage = Vec::with_capacity(capacity);
    let result = {
        let mut events = EventBuffer::new(&mut storage);
        match *op {
            Op::Add {
                id,
                participant,
                is_bid,
                price,
                qty,
                kind,
                stp_code,
            } => book.add_order(
                OrderId(id),
                ParticipantId(participant),
                if is_bid { Side::Bid } else { Side::Ask },
                kind_of(kind),
                price,
                qty,
                stp_of(stp_code),
                &mut events,
            ),
            Op::Cancel { id } => book.cancel_order(OrderId(id), &mut events),
            Op::Amend {
                id,
                new_price,
                new_qty,
            } => book.amend_order(OrderId(id), new_price, new_qty, &mut events),
            Op::CancelByParticipant { participant } => {
                book.cancel_by_participant(ParticipantId(participant), &mut events)
            }
        }
    };
    result.map(|_| storage)
}

/// Apply one generated add to both books, comparing the full event
/// stream or the exact rejection.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the public add_order parameter list"
)]
fn apply_add(
    book: &mut Book,
    oracle: &mut ReferenceBook,
    id: u64,
    participant: u32,
    is_bid: bool,
    price: u64,
    qty: u64,
    kind: u8,
    stp_code: u8,
) -> TestCaseResult {
    let side = if is_bid { Side::Bid } else { Side::Ask };
    let order_type = kind_of(kind);
    let stp = stp_of(stp_code);
    if oracle.contains(id) {
        let mut storage = Vec::with_capacity(64);
        let mut events = EventBuffer::new(&mut storage);
        let err = book
            .add_order(
                OrderId(id),
                ParticipantId(participant),
                side,
                order_type,
                price,
                qty,
                stp,
                &mut events,
            )
            .expect_err("duplicate id must be rejected");
        prop_assert_eq!(
            err,
            BookError::DuplicateOrderId {
                order_id: OrderId(id)
            }
        );
        prop_assert!(storage.is_empty());
        return Ok(());
    }
    match oracle.add_typed(
        id,
        participant,
        side,
        order_type,
        price,
        qty,
        stp.unwrap_or(DEFAULT_STP),
    ) {
        Ok(expected) => {
            let got = run_op(
                book,
                &Op::Add {
                    id,
                    participant,
                    is_bid,
                    price,
                    qty,
                    kind,
                    stp_code,
                },
                4096,
            );
            prop_assert_eq!(
                got.as_deref(),
                Ok(expected.as_slice()),
                "add stream mismatch"
            );
        }
        Err(OracleReject::PostOnlyWouldCross { opposite_best }) => {
            let err = add_rejection(book, id, participant, side, order_type, price, qty, stp);
            prop_assert_eq!(
                err,
                BookError::PostOnlyWouldCross {
                    price,
                    opposite_best
                }
            );
        }
        Err(OracleReject::FokInsufficient { available }) => {
            let err = add_rejection(book, id, participant, side, order_type, price, qty, stp);
            prop_assert_eq!(
                err,
                BookError::FokInsufficientLiquidity {
                    requested: qty,
                    available
                }
            );
        }
        Err(other) => {
            return Err(TestCaseError::fail(format!(
                "unexpected add rejection: {other:?}"
            )));
        }
    }
    Ok(())
}

/// Run an add the oracle rejected and return the engine's error,
/// asserting the rejection wrote no events.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the public add_order parameter list"
)]
fn add_rejection(
    book: &mut Book,
    id: u64,
    participant: u32,
    side: Side,
    order_type: OrderType,
    price: u64,
    qty: u64,
    stp: Option<StpPolicy>,
) -> BookError {
    let mut storage = Vec::with_capacity(64);
    let mut events = EventBuffer::new(&mut storage);
    let err = book
        .add_order(
            OrderId(id),
            ParticipantId(participant),
            side,
            order_type,
            price,
            qty,
            stp,
            &mut events,
        )
        .expect_err("the oracle rejected this add");
    assert!(storage.is_empty(), "a rejection wrote events");
    err
}

/// Apply one mass-cancel to both implementations and compare the
/// cancelled set (engine order is slot order, oracle order is id
/// order; the exact order is pinned by the unit test) and reasons.
fn apply_cbp(book: &mut Book, oracle: &mut ReferenceBook, participant: u32) -> TestCaseResult {
    let expected = oracle.cancel_by_participant(participant);
    let got = run_op(book, &Op::CancelByParticipant { participant }, 4096)
        .expect("mass cancel never rejects with an ample buffer");
    let mut got_keyed: Vec<(u64, u64)> = got
        .iter()
        .map(|e| match *e {
            ExecutionEvent::Cancelled {
                seq,
                order_id,
                reason,
            } => {
                assert_eq!(reason, CancelReason::ByParticipant);
                (order_id.0, seq)
            }
            other => panic!("mass cancel emitted a non-Cancelled event: {other:?}"),
        })
        .collect();
    let mut expected_keyed: Vec<(u64, u64)> = expected
        .iter()
        .map(|e| match *e {
            ExecutionEvent::Cancelled { seq, order_id, .. } => (order_id.0, seq),
            other => panic!("oracle mass cancel emitted {other:?}"),
        })
        .collect();
    got_keyed.sort_unstable();
    expected_keyed.sort_unstable();
    prop_assert_eq!(got_keyed, expected_keyed, "mass cancel set diverged");
    Ok(())
}

/// Apply one amend to both implementations and compare the event
/// stream or the exact rejection.
fn apply_amend(
    book: &mut Book,
    oracle: &mut ReferenceBook,
    id: u64,
    new_price: u64,
    new_qty: u64,
) -> TestCaseResult {
    if oracle.contains(id) {
        match oracle.amend(id, new_price, new_qty) {
            Ok(expected) => {
                let got = run_op(
                    book,
                    &Op::Amend {
                        id,
                        new_price,
                        new_qty,
                    },
                    64,
                );
                prop_assert_eq!(
                    got.as_deref(),
                    Ok(expected.as_slice()),
                    "amend stream mismatch"
                );
            }
            Err(OracleReject::AmendWouldCross { opposite_best }) => {
                let mut storage = Vec::with_capacity(64);
                let mut events = EventBuffer::new(&mut storage);
                let err = book
                    .amend_order(OrderId(id), new_price, new_qty, &mut events)
                    .expect_err("oracle said the amend crosses");
                prop_assert_eq!(
                    err,
                    BookError::AmendWouldCross {
                        order_id: OrderId(id),
                        new_price,
                        opposite_best
                    }
                );
                prop_assert!(storage.is_empty());
            }
            Err(other) => {
                return Err(TestCaseError::fail(format!(
                    "unexpected amend rejection from a live id: {other:?}"
                )));
            }
        }
    } else {
        let mut storage = Vec::with_capacity(64);
        let mut events = EventBuffer::new(&mut storage);
        let err = book
            .amend_order(OrderId(id), new_price, new_qty, &mut events)
            .expect_err("amend of a missing id must fail");
        prop_assert_eq!(
            err,
            BookError::OrderNotFound {
                order_id: OrderId(id)
            }
        );
        prop_assert!(storage.is_empty());
    }
    Ok(())
}

/// Apply one generated command to both books, then assert the books
/// agree on best prices, depth, and live count.
fn apply_op(book: &mut Book, oracle: &mut ReferenceBook, op: &Op) -> TestCaseResult {
    match *op {
        Op::Add {
            id,
            participant,
            is_bid,
            price,
            qty,
            kind,
            stp_code,
        } => apply_add(
            book,
            oracle,
            id,
            participant,
            is_bid,
            price,
            qty,
            kind,
            stp_code,
        )?,
        Op::Cancel { id } => {
            let mut storage = Vec::with_capacity(64);
            if let Some((expected, _remaining)) = oracle.cancel(id) {
                let got = {
                    let mut events = EventBuffer::new(&mut storage);
                    book.cancel_order(OrderId(id), &mut events)
                };
                prop_assert_eq!(got, Ok(1));
                prop_assert_eq!(storage, expected, "cancel stream mismatch");
            } else {
                let mut events = EventBuffer::new(&mut storage);
                let err = book
                    .cancel_order(OrderId(id), &mut events)
                    .expect_err("cancel of a missing id must fail");
                prop_assert_eq!(
                    err,
                    BookError::OrderNotFound {
                        order_id: OrderId(id)
                    }
                );
                prop_assert!(storage.is_empty());
            }
        }
        Op::Amend {
            id,
            new_price,
            new_qty,
        } => apply_amend(book, oracle, id, new_price, new_qty)?,
        Op::CancelByParticipant { participant } => apply_cbp(book, oracle, participant)?,
    }
    assert_agreement(book, oracle);
    Ok(())
}

proptest! {
    /// The full command surface against the oracle: every event
    /// stream and every rejection must match, after every command.
    /// This one property carries INV-BOOK-2, INV-BOOK-4, INV-EVENT-3,
    /// INV-AMEND-1 and 2, INV-ORD-3, and the oracle half of
    /// INV-STP-3 and INV-CANCEL-1.
    #[test]
    fn prop_book_matches_reference(ops in prop::collection::vec(op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &ops {
            apply_op(&mut book, &mut oracle, op)?;
        }
    }

    /// INV-BOOK-1 under an add/cancel/amend storm: the structural
    /// checker (which asserts the uncrossed book) runs after every
    /// command through `assert_agreement`.
    #[test]
    fn prop_add_cancel_amend_storm(ops in prop::collection::vec(storm_op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &ops {
            apply_op(&mut book, &mut oracle, op)?;
        }
    }

    /// INV-BOOK-4: quantity is conserved exactly. Every unit added is
    /// resting, was cancelled while resting, or left in a trade, and a
    /// trade removes the same quantity from the maker side as it
    /// consumes from the taker, so:
    /// added = resting + cancelled + 2 * traded.
    /// Participants are fixed per side so no self-trade ever fires:
    /// the identity is stated over plain Limit flow.
    #[test]
    fn prop_trade_quantity_conservation(ops in prop::collection::vec(limit_op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        let mut added: u128 = 0;
        let mut cancelled: u128 = 0;
        let mut traded: u128 = 0;
        for op in ops {
            match op {
                Op::Add { id, is_bid, price, qty, .. } => {
                    if oracle.contains(id) {
                        continue;
                    }
                    let side = if is_bid { Side::Bid } else { Side::Ask };
                    let participant = if is_bid { 1 } else { 2 };
                    let events = oracle.add_limit(id, participant, side, price, qty);
                    let mut storage = Vec::with_capacity(1024);
                    {
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, None, &mut buffer)
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
                _ => unreachable!("the limit stream has no other commands"),
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
    fn prop_best_price_matches_brute_force(ops in prop::collection::vec(limit_op_strategy(), 0..150)) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in ops {
            match op {
                Op::Add { id, participant, is_bid, price, qty, .. } => {
                    if !oracle.contains(id) {
                        let side = if is_bid { Side::Bid } else { Side::Ask };
                        oracle.add_limit(id, participant, side, price, qty);
                        let mut storage = Vec::with_capacity(1024);
                        let mut buffer = EventBuffer::new(&mut storage);
                        book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Limit, price, qty, None, &mut buffer)
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
                _ => unreachable!("the limit stream has no other commands"),
            }
            prop_assert_eq!(book.best_bid(), oracle.best(Side::Bid));
            prop_assert_eq!(book.best_ask(), oracle.best(Side::Ask));
            book.debug_assert_invariants();
        }
    }

    /// INV-ORD-1: a Fok order that cannot fill in full mutates
    /// nothing at all: full observable state and the sequence number
    /// are bit-identical after the rejection. A Fok order that does
    /// fill matches the oracle stream exactly.
    #[test]
    fn prop_fok_is_atomic(
        prelude in prop::collection::vec(limit_op_strategy(), 0..40),
        attempts in prop::collection::vec((25u64..=32, 0u32..3, any::<bool>(), 95u64..=106, 30u64..=120, 0u8..=3), 1..8)
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &prelude {
            apply_op(&mut book, &mut oracle, op)?;
        }
        for (id, participant, is_bid, price, qty, stp_code) in attempts {
            let side = if is_bid { Side::Bid } else { Side::Ask };
            let before = state_of(&book);
            let mut storage = Vec::with_capacity(1024);
            let result = {
                let mut events = EventBuffer::new(&mut storage);
                book.add_order(OrderId(id), ParticipantId(participant), side, OrderType::Fok, price, qty, stp_of(stp_code), &mut events)
            };
            match result {
                Err(BookError::FokInsufficientLiquidity { .. }) => {
                    prop_assert_eq!(state_of(&book), before, "a failed Fok mutated the book");
                    prop_assert!(storage.is_empty(), "a failed Fok wrote events");
                }
                Ok(_) => {
                    let expected = oracle
                        .add_typed(id, participant, side, OrderType::Fok, price, qty, stp_of(stp_code).unwrap_or(DEFAULT_STP))
                        .expect("engine filled it, so the oracle fills it");
                    prop_assert_eq!(storage, expected, "filled Fok stream diverged");
                    assert_agreement(&book, &oracle);
                }
                Err(other) => {
                    prop_assert!(false, "unexpected Fok error: {other:?}");
                }
            }
        }
    }

    /// INV-ORD-2: a PostOnly order either rests in full (matching the
    /// oracle) or is rejected with full state equality; after every
    /// attempt the book is uncrossed.
    #[test]
    fn prop_post_only_never_crosses(
        prelude in prop::collection::vec(op_strategy(), 0..60),
        attempts in prop::collection::vec((25u64..=32, 0u32..3, any::<bool>(), 95u64..=106, 1u64..=20), 1..8)
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &prelude {
            apply_op(&mut book, &mut oracle, op)?;
        }
        for (id, participant, is_bid, price, qty) in attempts {
            if oracle.contains(id) {
                continue;
            }
            let side = if is_bid { Side::Bid } else { Side::Ask };
            let before = state_of(&book);
            match oracle.add_typed(id, participant, side, OrderType::PostOnly, price, qty, DEFAULT_STP) {
                Err(OracleReject::PostOnlyWouldCross { opposite_best }) => {
                    let mut storage = Vec::with_capacity(64);
                    let mut events = EventBuffer::new(&mut storage);
                    let err = book
                        .add_order(OrderId(id), ParticipantId(participant), side, OrderType::PostOnly, price, qty, None, &mut events)
                        .expect_err("crossing PostOnly must be rejected");
                    prop_assert_eq!(err, BookError::PostOnlyWouldCross { price, opposite_best });
                    prop_assert_eq!(state_of(&book), before, "a rejected PostOnly mutated the book");
                }
                Ok(expected) => {
                    let got = run_op(&mut book, &Op::Add { id, participant, is_bid, price, qty, kind: 6, stp_code: 0 }, 64)
                        .expect("non-crossing PostOnly rests");
                    prop_assert_eq!(got, expected);
                    assert_agreement(&book, &oracle);
                }
                Err(other) => prop_assert!(false, "unexpected oracle verdict: {other:?}"),
            }
            if let (Some((bid, _)), Some((ask, _))) = (book.best_bid(), book.best_ask()) {
                prop_assert!(bid < ask, "book crossed after a PostOnly attempt");
            }
        }
    }

    /// INV-ORD-3: an Ioc order never rests: after the command its id
    /// is not live in either book, whatever it traded.
    #[test]
    fn prop_ioc_never_rests(
        prelude in prop::collection::vec(op_strategy(), 0..60),
        attempts in prop::collection::vec((25u64..=32, 0u32..3, any::<bool>(), 95u64..=106, 1u64..=30, 0u8..=3), 1..8)
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &prelude {
            apply_op(&mut book, &mut oracle, op)?;
        }
        for (id, participant, is_bid, price, qty, stp_code) in attempts {
            if oracle.contains(id) {
                continue;
            }
            let side = if is_bid { Side::Bid } else { Side::Ask };
            let expected = oracle
                .add_typed(id, participant, side, OrderType::Ioc, price, qty, stp_of(stp_code).unwrap_or(DEFAULT_STP))
                .expect("an Ioc add never rejects in preflight");
            let got = run_op(&mut book, &Op::Add { id, participant, is_bid, price, qty, kind: 4, stp_code }, 1024)
                .expect("ioc add");
            prop_assert_eq!(got, expected, "ioc stream diverged");
            prop_assert!(!oracle.contains(id), "oracle rests an Ioc order");
            let mut storage = Vec::with_capacity(4);
            let mut events = EventBuffer::new(&mut storage);
            let err = book
                .cancel_order(OrderId(id), &mut events)
                .expect_err("an Ioc order is never live after its command");
            prop_assert_eq!(err, BookError::OrderNotFound { order_id: OrderId(id) });
            assert_agreement(&book, &oracle);
        }
    }

    /// INV-ORD-4: the umbrella atomicity property. Invalid-biased
    /// commands of every rejection kind leave full state and seq
    /// bit-identical.
    #[test]
    fn prop_rejections_are_atomic(
        prelude in prop::collection::vec(limit_op_strategy(), 0..40),
        bad in prop::collection::vec(0u8..=10, 1..10)
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &prelude {
            apply_op(&mut book, &mut oracle, op)?;
        }
        for kind in bad {
            let live_id = oracle.orders.keys().next().copied();
            let before = state_of(&book);
            let mut storage = Vec::with_capacity(1024);
            let result = {
                let mut events = EventBuffer::new(&mut storage);
                match kind {
                    // Zero quantity add.
                    0 => book.add_order(OrderId(40), ParticipantId(0), Side::Bid, OrderType::Limit, 100, 0, None, &mut events),
                    // Zero order id add.
                    1 => book.add_order(OrderId(0), ParticipantId(0), Side::Bid, OrderType::Limit, 100, 5, None, &mut events),
                    // Off-grid price add (zero and far above the grid).
                    2 => book.add_order(OrderId(40), ParticipantId(0), Side::Bid, OrderType::Limit, 9999, 5, None, &mut events),
                    // Duplicate id add (skips when the book is empty).
                    3 => match live_id {
                        Some(id) => book.add_order(OrderId(id), ParticipantId(0), Side::Bid, OrderType::Limit, 100, 5, None, &mut events),
                        None => book.add_order(OrderId(0), ParticipantId(0), Side::Bid, OrderType::Limit, 100, 5, None, &mut events),
                    },
                    // Fok far beyond any liquidity this book can hold.
                    4 => book.add_order(OrderId(40), ParticipantId(0), Side::Bid, OrderType::Fok, 106, 500, None, &mut events),
                    // PostOnly priced through the opposite best (skips
                    // when the opposite side is empty).
                    5 => match oracle.best(Side::Ask) {
                        Some((best, _)) => book.add_order(OrderId(40), ParticipantId(0), Side::Bid, OrderType::PostOnly, best + 2, 5, None, &mut events),
                        None => book.add_order(OrderId(0), ParticipantId(0), Side::Bid, OrderType::Limit, 100, 5, None, &mut events),
                    },
                    // Resting add with a zero-capacity buffer.
                    6 => {
                        let mut empty = Vec::new();
                        let mut tiny = EventBuffer::new(&mut empty);
                        book.add_order(OrderId(41), ParticipantId(0), Side::Bid, OrderType::Limit, 90, 5, None, &mut tiny)
                    }
                    // Amend of a never-seen id.
                    7 => book.amend_order(OrderId(45), 100, 5, &mut events),
                    // Cancel of a never-seen id.
                    8 => book.cancel_order(OrderId(45), &mut events),
                    // Amend of a live order to zero quantity (skips
                    // when the book is empty).
                    9 => match live_id {
                        Some(id) => book.amend_order(OrderId(id), 100, 0, &mut events),
                        None => book.cancel_order(OrderId(45), &mut events),
                    },
                    // Amend of a live order to an off-grid price.
                    _ => match live_id {
                        Some(id) => book.amend_order(OrderId(id), 0, 5, &mut events),
                        None => book.cancel_order(OrderId(45), &mut events),
                    },
                }
            };
            prop_assert!(result.is_err(), "bad command kind {} was accepted", kind);
            prop_assert!(storage.is_empty(), "rejected kind {} wrote events", kind);
            prop_assert_eq!(state_of(&book), before, "rejected kind {} mutated the book", kind);
        }
    }

    /// INV-AMEND-3: cancel and amend of missing or terminal ids are
    /// exact no-ops. Probes use ids the stream may or may not have
    /// made terminal; live ids are skipped (they are not missing).
    #[test]
    fn prop_cancel_amend_missing_id_is_noop(
        ops in prop::collection::vec(op_strategy(), 0..80),
        probes in prop::collection::vec((1u64..=48, any::<bool>()), 1..10)
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &ops {
            apply_op(&mut book, &mut oracle, op)?;
        }
        for (id, is_cancel) in probes {
            if oracle.contains(id) {
                continue;
            }
            let before = state_of(&book);
            let mut storage = Vec::with_capacity(4);
            let mut events = EventBuffer::new(&mut storage);
            let err = if is_cancel {
                book.cancel_order(OrderId(id), &mut events)
                    .expect_err("cancel of a missing id")
            } else {
                book.amend_order(OrderId(id), 100, 5, &mut events)
                    .expect_err("amend of a missing id")
            };
            prop_assert_eq!(err, BookError::OrderNotFound { order_id: OrderId(id) });
            prop_assert_eq!(state_of(&book), before, "a missing-id command mutated the book");
        }
    }

    /// INV-CANCEL-1 with a two-participant pool, so mass cancels bite
    /// often: the cancelled set matches the oracle exactly and the
    /// survivors agree.
    #[test]
    fn prop_cancel_by_participant_exact_set(
        ops in prop::collection::vec(
            prop_oneof![
                5 => (1u64..=24, 0u32..2, any::<bool>(), 95u64..=106, 1u64..=20, 0u8..=6, 0u8..=3).prop_map(
                    |(id, participant, is_bid, price, qty, kind, stp_code)| Op::Add {
                        id, participant, is_bid, price, qty, kind, stp_code
                    }
                ),
                2 => (1u64..=24).prop_map(|id| Op::Cancel { id }),
                2 => (0u32..2).prop_map(|participant| Op::CancelByParticipant { participant }),
            ],
            0..150
        )
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &ops {
            apply_op(&mut book, &mut oracle, op)?;
        }
    }

    /// INV-STP-3 property coverage: a two-participant pool and an
    /// explicit policy on every add, so self-trades fire constantly
    /// and every policy is exercised against the oracle.
    #[test]
    fn prop_stp_policies_match_oracle(
        ops in prop::collection::vec(
            prop_oneof![
                6 => (1u64..=24, 0u32..2, any::<bool>(), 95u64..=106, 1u64..=20, 0u8..=6, 1u8..=3).prop_map(
                    |(id, participant, is_bid, price, qty, kind, stp_code)| Op::Add {
                        id, participant, is_bid, price, qty, kind, stp_code
                    }
                ),
                2 => (1u64..=24).prop_map(|id| Op::Cancel { id }),
                1 => (1u64..=24, 95u64..=106, 1u64..=20).prop_map(|(id, new_price, new_qty)| Op::Amend {
                    id, new_price, new_qty
                }),
                1 => (0u32..2).prop_map(|participant| Op::CancelByParticipant { participant }),
            ],
            0..150
        )
    ) {
        let mut book = test_book();
        let mut oracle = ReferenceBook::new();
        for op in &ops {
            apply_op(&mut book, &mut oracle, op)?;
        }
    }

    /// INV-EVENT-2: starving a command with a too-small buffer and
    /// then retrying it with a sufficient one produces the identical
    /// stream and state as running it once with a sufficient buffer.
    #[test]
    fn prop_event_buffer_retry_is_identical(
        ops in prop::collection::vec(
            (op_strategy(), 0usize..=3).prop_map(|(op, cap)| (op, cap)),
            0..120
        )
    ) {
        let mut starved = test_book();
        let mut fed = test_book();
        for (op, cap) in &ops {
            let expected = run_op(&mut fed, op, 4096);
            match run_op(&mut starved, op, *cap) {
                Err(BookError::EventBufferFull { .. }) => {
                    let retried = run_op(&mut starved, op, 4096);
                    prop_assert_eq!(retried, expected, "retry after EventBufferFull diverged");
                }
                other => {
                    prop_assert_eq!(other, expected, "small-buffer run diverged without starving");
                }
            }
            prop_assert_eq!(state_of(&starved), state_of(&fed), "state diverged after retry");
            starved.debug_assert_invariants();
        }
    }
}
