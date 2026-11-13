//! Public-API integration tests: the crate used exactly as a caller
//! uses it, through re-exported types only.

use chronicle_core::{
    Book, BookConfig, BookError, CancelReason, DepthLevel, EventBuffer, ExecutionEvent, OrderId,
    OrderType, ParticipantId, Side, StpPolicy,
};

fn sample_book() -> Book {
    Book::new(BookConfig {
        max_orders: 128,
        grid_len: 8192,
        tick_size: 5,
        grid_start_tick: 100,
        default_stp: StpPolicy::CancelIncoming,
    })
    .expect("valid config")
}

#[test]
fn public_api_round_trip() {
    let mut book = sample_book();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(256);
    let mut events = EventBuffer::new(&mut storage);

    // Rest a bid at tick 200 (price 1000) and an ask at tick 204.
    book.add_order(
        OrderId(1),
        ParticipantId(7),
        Side::Bid,
        OrderType::Limit,
        1000,
        30,
        None,
        &mut events,
    )
    .expect("bid rests");
    book.add_order(
        OrderId(2),
        ParticipantId(8),
        Side::Ask,
        OrderType::Limit,
        1020,
        10,
        None,
        &mut events,
    )
    .expect("ask rests");
    assert_eq!(book.best_bid(), Some((1000, 30)));
    assert_eq!(book.best_ask(), Some((1020, 10)));

    // Cross the ask fully and part of nothing else.
    let written = book
        .add_order(
            OrderId(3),
            ParticipantId(9),
            Side::Bid,
            OrderType::Limit,
            1020,
            10,
            None,
            &mut events,
        )
        .expect("crossing bid");
    assert_eq!(written, 3); // Trade + Filled(maker) + Filled(taker)
    assert_eq!(book.best_ask(), None);
    assert_eq!(book.live_order_count(), 1);

    // Depth on the bid side shows the resting order.
    let mut out = [DepthLevel {
        price: 0,
        total_qty: 0,
        order_count: 0,
    }; 2];
    assert_eq!(book.depth(Side::Bid, 2, &mut out), 1);
    assert_eq!(
        out[0],
        DepthLevel {
            price: 1000,
            total_qty: 30,
            order_count: 1
        }
    );

    // Cancel it; the book is empty and the checker is satisfied.
    book.cancel_order(OrderId(1), &mut events).expect("cancel");
    assert_eq!(book.live_order_count(), 0);
    book.debug_assert_invariants();
}

#[test]
fn public_api_rejections_are_typed() {
    let mut book = sample_book();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(16);
    let mut events = EventBuffer::new(&mut storage);
    // Price 1001 is not a multiple of tick size 5.
    let err = book
        .add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            1001,
            1,
            None,
            &mut events,
        )
        .expect_err("unaligned price");
    assert!(matches!(err, BookError::InvalidPriceTick { .. }));
    // A Fill-or-Kill order against an empty book reports the exact
    // STP-adjusted executable quantity: zero.
    let err = book
        .add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Fok,
            1000,
            1,
            None,
            &mut events,
        )
        .expect_err("no liquidity for a Fok order");
    assert_eq!(
        err,
        BookError::FokInsufficientLiquidity {
            requested: 1,
            available: 0
        }
    );
}

/// Run one command against `book` with a fresh buffer over the
/// shared `storage`, so the storage stays readable between commands.
fn call<T>(storage: &mut Vec<ExecutionEvent>, f: impl FnOnce(&mut EventBuffer<'_>) -> T) -> T {
    let mut events = EventBuffer::new(storage);
    f(&mut events)
}

#[test]
fn public_api_milestone_2_surface() {
    let mut book = sample_book();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(256);

    // Two asks rest for participant 8 (ticks 204 and 208).
    call(&mut storage, |e| {
        book.add_order(
            OrderId(1),
            ParticipantId(8),
            Side::Ask,
            OrderType::Limit,
            1020,
            10,
            None,
            e,
        )
    })
    .expect("ask 1");
    call(&mut storage, |e| {
        book.add_order(
            OrderId(2),
            ParticipantId(8),
            Side::Ask,
            OrderType::Limit,
            1040,
            10,
            None,
            e,
        )
    })
    .expect("ask 2");

    // A PostOnly buy priced at the best ask is rejected; one tick
    // below it rests.
    let err = call(&mut storage, |e| {
        book.add_order(
            OrderId(3),
            ParticipantId(7),
            Side::Bid,
            OrderType::PostOnly,
            1020,
            5,
            None,
            e,
        )
    })
    .expect_err("post-only at the ask crosses");
    assert_eq!(
        err,
        BookError::PostOnlyWouldCross {
            price: 1020,
            opposite_best: 1020
        }
    );
    call(&mut storage, |e| {
        book.add_order(
            OrderId(3),
            ParticipantId(7),
            Side::Bid,
            OrderType::PostOnly,
            1015,
            5,
            None,
            e,
        )
    })
    .expect("post-only below the ask rests");
    assert_eq!(book.best_bid(), Some((1015, 5)));

    // Amend the resting bid up in quantity; it stays the best bid.
    let written = call(&mut storage, |e| book.amend_order(OrderId(3), 1015, 8, e)).expect("amend");
    assert_eq!(written, 1);
    assert_eq!(book.best_bid(), Some((1015, 8)));
}

#[test]
fn public_api_milestone_2_ioc_and_mass_cancel() {
    let mut book = sample_book();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(256);
    call(&mut storage, |e| {
        book.add_order(
            OrderId(1),
            ParticipantId(8),
            Side::Ask,
            OrderType::Limit,
            1020,
            10,
            None,
            e,
        )
    })
    .expect("ask 1");
    call(&mut storage, |e| {
        book.add_order(
            OrderId(2),
            ParticipantId(8),
            Side::Ask,
            OrderType::Limit,
            1040,
            10,
            None,
            e,
        )
    })
    .expect("ask 2");
    call(&mut storage, |e| {
        book.add_order(
            OrderId(3),
            ParticipantId(7),
            Side::Bid,
            OrderType::PostOnly,
            1015,
            8,
            None,
            e,
        )
    })
    .expect("bid rests below the ask");

    // An Ioc buy sweeps the first ask and expires its remainder
    // instead of resting against the second ask (price 1025 < 1040).
    let before = storage.len();
    call(&mut storage, |e| {
        book.add_order(
            OrderId(4),
            ParticipantId(9),
            Side::Bid,
            OrderType::Ioc,
            1025,
            15,
            None,
            e,
        )
    })
    .expect("ioc sweep");
    assert_eq!(
        storage[before..],
        [
            ExecutionEvent::Trade {
                seq: 4,
                maker_order_id: OrderId(1),
                taker_order_id: OrderId(4),
                price: 1020,
                qty: 10,
            },
            ExecutionEvent::Filled {
                seq: 4,
                order_id: OrderId(1)
            },
            ExecutionEvent::Expired {
                seq: 4,
                order_id: OrderId(4),
                remaining_qty: 5
            },
        ]
    );
    assert_eq!(
        book.best_bid(),
        Some((1015, 8)),
        "the Ioc order did not rest"
    );

    // Mass-cancel participant 8: only ask 2 remains of theirs.
    let written = call(&mut storage, |e| {
        book.cancel_by_participant(ParticipantId(8), e)
    })
    .expect("mass cancel");
    assert_eq!(written, 1);
    assert_eq!(
        storage.last(),
        Some(&ExecutionEvent::Cancelled {
            seq: 5,
            order_id: OrderId(2),
            reason: CancelReason::ByParticipant
        })
    );
    assert_eq!(book.best_ask(), None);
    assert_eq!(book.live_order_count(), 1);
    book.debug_assert_invariants();
}
