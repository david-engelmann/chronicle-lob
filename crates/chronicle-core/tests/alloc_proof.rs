//! INV-ALLOC-1, the allocation proof for the matching engine: with
//! a thread-local counting allocator, 1,000,000 mixed operations at
//! full capacity perform exactly zero heap allocations in the
//! steady-state window. The counting allocator lives in this
//! dedicated test target, the one place the workspace permits
//! unsafe code, and counts only the thread running this test, so
//! parallel tests cannot pollute the count. The mechanism is the
//! entry condition m6 decision recorded for Milestone 4: the same
//! pattern chronicle-wal's `alloc_proof` target proved at M3,
//! extended here to the core engine.
#![allow(
    unsafe_code,
    reason = "a counting GlobalAlloc is inherently unsafe; it is confined to this test target and only forwards to the system allocator"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Once;

use chronicle_core::{
    Book, BookConfig, BookError, DepthLevel, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
};

const DEFAULT_SEED: u64 = 0xC0FF_EE11_2233_4455;
static SEED_BANNER: Once = Once::new();

fn active_seed() -> u64 {
    let seed = std::env::var("SIM_SEED")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    SEED_BANNER.call_once(|| {
        eprintln!("SIM_SEED={seed}");
    });
    seed
}

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// A global allocator that counts allocations per thread and
/// forwards everything to the system allocator.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: forwards the caller's layout to the system
        // allocator unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwards a pointer and layout this allocator
        // produced, as the GlobalAlloc contract requires of callers.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

/// `SplitMix64`, the workspace's deterministic generator (section 11).
struct SplitMix64(u64);

impl SplitMix64 {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

const fn config() -> BookConfig {
    BookConfig {
        max_orders: 1024,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

struct Counters {
    events: u64,
    trades: u64,
    applied: u64,
    rejected: u64,
}

fn tally(storage: &[ExecutionEvent], written: usize, counters: &mut Counters) {
    counters.events += written as u64;
    for event in &storage[..written] {
        if matches!(event, ExecutionEvent::Trade { .. }) {
            counters.trades += 1;
        }
    }
}

/// Apply one finished book call and tally its events on success.
fn apply_tallied(
    storage: &[ExecutionEvent],
    counters: &mut Counters,
    out: Result<usize, BookError>,
) -> Result<usize, BookError> {
    if let Ok(written) = out {
        tally(storage, written, counters);
    }
    out
}

/// One add operation from the mix: a resting or crossing add of the
/// given order type, with side, price, quantity, and id derived
/// from the draw. `side_shift` picks which draw bit chooses the
/// side, so the two add arms decorrelate.
fn add_op(
    book: &mut Book,
    storage: &mut Vec<ExecutionEvent>,
    counters: &mut Counters,
    r: u64,
    participant: ParticipantId,
    order_type: OrderType,
    side_shift: u32,
) -> Result<usize, BookError> {
    let side = if (r >> side_shift).is_multiple_of(2) {
        Side::Bid
    } else {
        Side::Ask
    };
    let price = 250 + (r >> 9) % 100;
    let qty = 1 + (r >> 17) % 20;
    let order_id = OrderId(2000 + (r >> 32) % 4000);
    let mut events = EventBuffer::new(&mut *storage);
    let out = book.add_order(
        order_id,
        participant,
        side,
        order_type,
        price,
        qty,
        None,
        &mut events,
    );
    apply_tallied(storage, counters, out)
}

/// One rejection-path operation from the mix, rotating by draw:
/// zero quantity, off-grid price, unknown cancel, duplicate id.
/// Each must fail with no side effects (INV-ORD-1 covers the
/// no-side-effect half; here they prove the paths run
/// allocation-free). Each arm scopes its own event buffer so the
/// tallies between calls never fight a live borrow.
fn rejection_op(
    book: &mut Book,
    storage: &mut Vec<ExecutionEvent>,
    counters: &mut Counters,
    r: u64,
    participant: ParticipantId,
) -> Result<usize, BookError> {
    match (r >> 8) % 4 {
        0 => {
            let mut events = EventBuffer::new(&mut *storage);
            book.add_order(
                OrderId(900_001),
                participant,
                Side::Bid,
                OrderType::Limit,
                100,
                0,
                None,
                &mut events,
            )
        }
        1 => {
            let mut events = EventBuffer::new(&mut *storage);
            book.add_order(
                OrderId(900_002),
                participant,
                Side::Bid,
                OrderType::Limit,
                0,
                5,
                None,
                &mut events,
            )
        }
        2 => {
            let mut events = EventBuffer::new(&mut *storage);
            book.cancel_order(OrderId(999_999), &mut events)
        }
        _ => {
            // Duplicate id: plant a live order, then add it again.
            // The plant and the cleanup sit inside the same
            // rejection slot.
            let plant = {
                let mut events = EventBuffer::new(&mut *storage);
                book.add_order(
                    OrderId(900_003),
                    participant,
                    Side::Bid,
                    OrderType::Limit,
                    100,
                    5,
                    None,
                    &mut events,
                )
            };
            let _ = apply_tallied(storage, counters, plant);
            let dup = {
                let mut events = EventBuffer::new(&mut *storage);
                book.add_order(
                    OrderId(900_003),
                    participant,
                    Side::Bid,
                    OrderType::Limit,
                    100,
                    5,
                    None,
                    &mut events,
                )
            };
            let mut cleanup = EventBuffer::new(&mut *storage);
            let _ = book.cancel_order(OrderId(900_003), &mut cleanup);
            dup
        }
    }
}

/// Run `count` mixed operations against the book. The mix covers
/// resting and crossing adds of every order type, cancels, amends,
/// mass cancels, depth and best-price queries, and the rejection
/// paths (zero quantity, off-grid price, unknown cancel, duplicate
/// id). Nothing in this function allocates: the event storage and
/// the depth buffer are caller-owned and reused throughout.
fn run_mixed_ops(
    book: &mut Book,
    storage: &mut Vec<ExecutionEvent>,
    depth_out: &mut [DepthLevel],
    rng: &mut SplitMix64,
    count: u64,
    counters: &mut Counters,
) {
    for _ in 0..count {
        let r = rng.next();
        let slot = r % 100;
        let participant = ParticipantId(u32::try_from((r >> 24) % 4).expect("mod 4 fits u32"));
        let result = if slot < 40 {
            // Resting and crossing limit adds near the touch.
            add_op(book, storage, counters, r, participant, OrderType::Limit, 8)
        } else if slot < 55 {
            // Immediate order types, rotating by draw.
            let order_type = match (r >> 8) % 3 {
                0 => OrderType::Ioc,
                1 => OrderType::Fok,
                _ => OrderType::PostOnly,
            };
            add_op(book, storage, counters, r, participant, order_type, 11)
        } else if slot < 75 {
            let order_id = OrderId(1 + (r >> 8) % 6000);
            let mut events = EventBuffer::new(&mut *storage);
            let out = book.cancel_order(order_id, &mut events);
            apply_tallied(storage, counters, out)
        } else if slot < 85 {
            let order_id = OrderId(1 + (r >> 8) % 6000);
            let new_price = 250 + (r >> 9) % 100;
            let new_qty = 1 + (r >> 17) % 20;
            let mut events = EventBuffer::new(&mut *storage);
            let out = book.amend_order(order_id, new_price, new_qty, &mut events);
            apply_tallied(storage, counters, out)
        } else if slot < 90 {
            let mut events = EventBuffer::new(&mut *storage);
            let out = book.cancel_by_participant(participant, &mut events);
            apply_tallied(storage, counters, out)
        } else if slot < 95 {
            // Queries return values, not events; count them as
            // applied work with zero events written.
            let levels = book.depth(Side::Bid, 64, depth_out);
            let best = book.best_bid().map_or(0, |(_, qty)| qty)
                + book.best_ask().map_or(0, |(_, qty)| qty);
            let spread = book.spread().unwrap_or(0);
            let probe_id = OrderId(1 + (r >> 8) % 6000);
            let info_qty = book.get_order(probe_id).map_or(0, |info| info.qty);
            let present = book.contains_order(probe_id);
            let probe_price = 250 + (r >> 9) % 100;
            let level_qty = book
                .level_at(probe_price)
                .ok()
                .flatten()
                .map_or(0, |(_, d)| d.total_qty);
            let iter_sum: u64 = book
                .orders_at(probe_price)
                .map_or(0, |it| it.map(|o| o.qty).sum());
            let free_slots = book.free_order_count();
            std::hint::black_box((
                levels, best, spread, info_qty, present, level_qty, iter_sum, free_slots,
            ));
            counters.applied += 1;
            continue;
        } else {
            rejection_op(book, storage, counters, r, participant)
        };
        match result {
            Ok(_) => counters.applied += 1,
            Err(_) => counters.rejected += 1,
        }
        storage.clear();
    }
}

#[test]
fn alloc_zero_steady_state_1m_ops() {
    let cfg = config();
    let mut book = Book::new(cfg).expect("book");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(2 * cfg.max_orders as usize + 2);
    let mut depth_out: Vec<DepthLevel> = Vec::new();
    depth_out.resize_with(64, || DepthLevel {
        price: 0,
        total_qty: 0,
        order_count: 0,
    });
    let mut rng = SplitMix64(active_seed());
    let mut counters = Counters {
        events: 0,
        trades: 0,
        applied: 0,
        rejected: 0,
    };

    // Warm-up: fill the book to full capacity, then run a long
    // unmeasured mix so every lazy path is already warm.
    for i in 0..cfg.max_orders {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let offset = u64::from((i / 2) % 200);
        let price = if side == Side::Bid {
            100 + offset
        } else {
            301 + offset
        };
        let mut events = EventBuffer::new(&mut storage);
        book.add_order(
            OrderId(u64::from(i) + 1),
            ParticipantId(i % 4),
            side,
            OrderType::Limit,
            price,
            5,
            None,
            &mut events,
        )
        .expect("warm-up add rests");
        storage.clear();
    }
    assert_eq!(book.live_order_count(), cfg.max_orders);
    run_mixed_ops(
        &mut book,
        &mut storage,
        &mut depth_out,
        &mut rng,
        50_000,
        &mut counters,
    );

    // The measured window: exactly 1,000,000 mixed operations.
    counters = Counters {
        events: 0,
        trades: 0,
        applied: 0,
        rejected: 0,
    };
    let before = allocations();
    run_mixed_ops(
        &mut book,
        &mut storage,
        &mut depth_out,
        &mut rng,
        1_000_000,
        &mut counters,
    );
    let allocated = allocations() - before;
    assert_eq!(
        allocated, 0,
        "1,000,000 steady-state operations must not allocate"
    );
    assert_eq!(counters.applied + counters.rejected, 1_000_000);
    assert!(
        counters.events > 50_000,
        "the mix must produce real matching traffic, saw {} events",
        counters.events
    );
    assert!(
        counters.trades > 5_000,
        "the mix must cross the book, saw {} trades",
        counters.trades
    );
    assert!(
        counters.rejected > 1_000,
        "the mix must exercise rejection paths, saw {} rejections",
        counters.rejected
    );
}
