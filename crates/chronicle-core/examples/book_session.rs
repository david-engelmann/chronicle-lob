//! A worked session against chronicle-core: every order type, an
//! amend, a cancel, both rejection shapes, and all three self-trade
//! prevention policies. CI runs this example and diffs its output
//! against `book_session.expected.txt`, so the printed stream is
//! a pinned contract, not an illustration.

use chronicle_core::{
    Book, BookConfig, BookError, CancelReason, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
};

const fn config() -> BookConfig {
    BookConfig {
        max_orders: 64,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

const fn reason_token(reason: CancelReason) -> &'static str {
    match reason {
        CancelReason::Requested => "requested",
        CancelReason::ByParticipant => "by-participant",
        CancelReason::SelfTradePrevention => "self-trade-prevention",
    }
}

/// The event line format the command line tool also prints
/// (ARCHITECTURE.md section 2.3), one line per event.
fn format_event(event: &ExecutionEvent) -> String {
    match event {
        ExecutionEvent::Accepted { seq, order_id } => {
            format!("{seq} accepted order={}", order_id.0)
        }
        ExecutionEvent::Trade {
            seq,
            maker_order_id,
            taker_order_id,
            price,
            qty,
        } => format!(
            "{seq} trade maker={} taker={} price={price} qty={qty}",
            maker_order_id.0, taker_order_id.0
        ),
        ExecutionEvent::Filled { seq, order_id } => format!("{seq} filled order={}", order_id.0),
        ExecutionEvent::Cancelled {
            seq,
            order_id,
            reason,
        } => format!(
            "{seq} cancelled order={} reason={}",
            order_id.0,
            reason_token(*reason)
        ),
        ExecutionEvent::Amended {
            seq,
            order_id,
            price,
            qty,
        } => format!("{seq} amended order={} price={price} qty={qty}", order_id.0),
        ExecutionEvent::Expired {
            seq,
            order_id,
            remaining_qty,
        } => format!(
            "{seq} expired order={} remaining={remaining_qty}",
            order_id.0
        ),
        ExecutionEvent::SelfTradePrevented { seq, order_id } => {
            format!("{seq} self-trade-prevented order={}", order_id.0)
        }
    }
}

/// Print a command's events, or its rejection. A rejected command
/// mutates nothing and consumes no sequence number (INV-ORD-4), so
/// the stream simply notes it and moves on.
fn report(result: Result<usize, BookError>, storage: &[ExecutionEvent], order_id: u64) {
    match result {
        Ok(written) => {
            for event in &storage[..written] {
                println!("{}", format_event(event));
            }
        }
        Err(error) => println!("rejected order={order_id}: {error}"),
    }
}

/// One add-order command for the session, with the self-trade
/// policy left at the book default unless a step sets it.
#[derive(Clone, Copy)]
struct Add {
    id: u64,
    participant: u32,
    side: Side,
    order_type: OrderType,
    price: u64,
    qty: u64,
    stp: Option<StpPolicy>,
}

const fn add(
    id: u64,
    participant: u32,
    side: Side,
    order_type: OrderType,
    price: u64,
    qty: u64,
) -> Add {
    Add {
        id,
        participant,
        side,
        order_type,
        price,
        qty,
        stp: None,
    }
}

fn apply_add(book: &mut Book, storage: &mut Vec<ExecutionEvent>, order: Add) {
    let result = {
        let mut events = EventBuffer::new(&mut *storage);
        book.add_order(
            OrderId(order.id),
            ParticipantId(order.participant),
            order.side,
            order.order_type,
            order.price,
            order.qty,
            order.stp,
            &mut events,
        )
    };
    report(result, storage.as_slice(), order.id);
    storage.clear();
}

fn session(book: &mut Book, storage: &mut Vec<ExecutionEvent>) {
    // Part one: rest, cross, amend, cancel.
    apply_add(
        book,
        storage,
        add(1, 1, Side::Bid, OrderType::Limit, 100, 10),
    );
    apply_add(
        book,
        storage,
        add(2, 2, Side::Ask, OrderType::Limit, 101, 5),
    );
    apply_add(
        book,
        storage,
        add(3, 2, Side::Ask, OrderType::Limit, 100, 4),
    );
    let result = {
        let mut events = EventBuffer::new(&mut *storage);
        book.amend_order(OrderId(1), 99, 6, &mut events)
    };
    report(result, storage.as_slice(), 1);
    storage.clear();
    let result = {
        let mut events = EventBuffer::new(&mut *storage);
        book.cancel_order(OrderId(2), &mut events)
    };
    report(result, storage.as_slice(), 2);
    storage.clear();

    // Part two: the remaining order types, including both
    // rejections (a post-only that would cross, a fill-or-kill
    // with nothing to fill against).
    apply_add(
        book,
        storage,
        add(4, 2, Side::Ask, OrderType::Limit, 105, 20),
    );
    apply_add(book, storage, add(5, 1, Side::Bid, OrderType::Ioc, 105, 25));
    apply_add(
        book,
        storage,
        add(6, 2, Side::Bid, OrderType::PostOnly, 106, 5),
    );
    apply_add(
        book,
        storage,
        add(7, 1, Side::Ask, OrderType::PostOnly, 105, 5),
    );
    apply_add(book, storage, add(8, 1, Side::Ask, OrderType::Fok, 106, 5));
    apply_add(book, storage, add(9, 2, Side::Bid, OrderType::Fok, 100, 50));

    // Part three: self-trade prevention, one policy at a time.
    // Participant 3 rests an ask, then meets it with each policy
    // from the other side; participant 2 then does the same
    // against its own ask for the remaining two policies.
    apply_add(
        book,
        storage,
        add(10, 3, Side::Ask, OrderType::Limit, 100, 10),
    );
    let mut resting = add(11, 3, Side::Bid, OrderType::Limit, 100, 4);
    resting.stp = Some(StpPolicy::CancelResting);
    apply_add(book, storage, resting);
    apply_add(
        book,
        storage,
        add(12, 2, Side::Ask, OrderType::Limit, 100, 8),
    );
    let mut incoming = add(13, 2, Side::Bid, OrderType::Limit, 100, 3);
    incoming.stp = Some(StpPolicy::CancelIncoming);
    apply_add(book, storage, incoming);
    let mut both = add(14, 2, Side::Bid, OrderType::Limit, 100, 3);
    both.stp = Some(StpPolicy::CancelBoth);
    apply_add(book, storage, both);
}

fn main() {
    let mut book = Book::new(config()).expect("the example configuration is valid");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(128);
    session(&mut book, &mut storage);
    println!("summary: live_orders={}", book.live_order_count());
    match book.best_bid() {
        Some((price, qty)) => println!("summary: best_bid={price} qty={qty}"),
        None => println!("summary: best_bid=none"),
    }
    match book.best_ask() {
        Some((price, qty)) => println!("summary: best_ask={price} qty={qty}"),
        None => println!("summary: best_ask=none"),
    }
}
