//! Public-API integration tests: the crate used exactly as a caller
//! uses it, through re-exported types only.

use chronicle_core::{
    Book, BookConfig, BookError, DepthLevel, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
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
            &mut events,
        )
        .expect_err("unaligned price");
    assert!(matches!(err, BookError::InvalidPriceTick { .. }));
    // IOC is part of the contract but executes from Milestone 2.
    let err = book
        .add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Ioc,
            1000,
            1,
            &mut events,
        )
        .expect_err("IOC not executed at M1");
    assert_eq!(
        err,
        BookError::UnsupportedOrderType {
            order_type: OrderType::Ioc
        }
    );
}
