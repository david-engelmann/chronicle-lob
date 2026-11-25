//! Public-API integration tests: the crate used exactly as a caller
//! uses it, through re-exported types only.

use chronicle_core::{
    Book, BookConfig, BookError, CancelReason, DepthLevel, EventBuffer, ExecutionEvent, OrderId,
    OrderInfo, OrderType, ParticipantId, Side, StpPolicy,
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

#[test]
fn public_api_config_bounds_and_event_clear() {
    let book = sample_book();
    let cfg = book.config();
    assert_eq!(cfg.min_price(), 500);
    assert_eq!(cfg.max_price().expect("valid max price"), 41_455);
    assert_eq!(book.free_order_count(), 128);
    assert_eq!(book.spread(), None);
    assert!(!book.contains_order(OrderId(0)));
    assert_eq!(book.get_order(OrderId(0)), None);

    // Zero grid_start_tick still reports tick 1 as min_price because 0 is off-grid.
    let zero_start = BookConfig {
        max_orders: 16,
        grid_len: 64,
        tick_size: 5,
        grid_start_tick: 0,
        default_stp: StpPolicy::CancelIncoming,
    };
    let mut zero_book = Book::new(zero_start).expect("valid zero-start config");
    assert_eq!(zero_start.min_price(), 5);
    assert_eq!(zero_start.max_price().unwrap(), 315);

    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut events = EventBuffer::new(&mut storage);
    zero_book
        .add_order(
            OrderId(1),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            zero_start.min_price(),
            3,
            None,
            &mut events,
        )
        .expect("min_price is accepted");
    events.clear();
    assert_eq!(events.written(), []);
}

#[test]
fn public_api_domain_queries_and_orders_at() {
    let mut book = sample_book();
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut events = EventBuffer::new(&mut storage);

    // Add three bids at 1000 and an ask at 1015 to exercise get_order,
    // contains_order, spread, level_at, orders_at (FIFO and reverse),
    // EventBuffer::clear, and Book Debug formatting.
    for (id, participant, qty) in [(10u64, 1u32, 5u64), (11, 2, 7), (12, 3, 9)] {
        book.add_order(
            OrderId(id),
            ParticipantId(participant),
            Side::Bid,
            OrderType::Limit,
            1000,
            qty,
            None,
            &mut events,
        )
        .expect("bid rests");
        assert_eq!(events.written().len(), 1);
        events.clear();
    }
    book.add_order(
        OrderId(20),
        ParticipantId(4),
        Side::Ask,
        OrderType::Limit,
        1015,
        12,
        None,
        &mut events,
    )
    .expect("ask rests");
    events.clear();

    assert_eq!(book.live_order_count(), 4);
    assert_eq!(book.free_order_count(), 124);
    assert_eq!(book.spread(), Some(15));
    assert!(book.contains_order(OrderId(11)));
    assert!(!book.contains_order(OrderId(999)));
    assert_eq!(
        book.get_order(OrderId(11)),
        Some(OrderInfo {
            order_id: OrderId(11),
            participant: ParticipantId(2),
            side: Side::Bid,
            price: 1000,
            qty: 7,
        })
    );

    assert_eq!(
        book.level_at(1000),
        Ok(Some((
            Side::Bid,
            DepthLevel {
                price: 1000,
                total_qty: 21,
                order_count: 3,
            }
        )))
    );
    assert_eq!(book.level_at(1005), Ok(None));
    assert!(matches!(
        book.level_at(1001),
        Err(BookError::InvalidPriceTick { .. })
    ));

    let fifo: Vec<OrderId> = book
        .orders_at(1000)
        .expect("valid tick")
        .map(|o| o.order_id)
        .collect();
    assert_eq!(fifo, vec![OrderId(10), OrderId(11), OrderId(12)]);

    let mut iter = book.orders_at(1000).expect("valid tick");
    assert_eq!(iter.len(), 3);
    assert_eq!(iter.next().map(|o| o.order_id), Some(OrderId(10)));
    assert_eq!(iter.next_back().map(|o| o.order_id), Some(OrderId(12)));
    assert_eq!(iter.len(), 1);
    assert_eq!(iter.next_back().map(|o| o.order_id), Some(OrderId(11)));
    assert_eq!(iter.len(), 0);
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);

    let dbg = format!("{book:?}");
    assert!(dbg.contains("Book"));
    assert!(dbg.contains("live_orders: 4"));
    assert!(dbg.contains("free_orders: 124"));
}
