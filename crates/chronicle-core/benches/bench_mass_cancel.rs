//! INV-LAT-7 (ARCHITECTURE.md section 13): mass cancel latency,
//! one `cancel_by_participant` call against a book at realistic
//! occupancy, 12,000 live orders across 240 active levels. The
//! measured participant holds exactly 800 of those orders, and
//! throughput is counted per cancelled order.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use std::hint::black_box;
use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};

const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 32_768,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// A book holding 12,000 resting orders: 120 bid levels and 120 ask
/// levels with 50 orders each and a two-tick spread. The participant
/// of level `level` is `level % 16`, so participant 1 holds the
/// orders of levels 1, 17, 33, 49, 65, 81, 97, and 113, which is
/// 8 levels of 100 orders (50 bids and 50 asks) each.
fn occupied_book() -> (Book, Vec<ExecutionEvent>) {
    let mut book = Book::new(bench_config()).expect("book");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut id = 0u64;
    for level in 0..120u64 {
        for _ in 0..50 {
            id += 1;
            let mut events = EventBuffer::new(&mut storage);
            book.add_order(
                OrderId(id),
                ParticipantId(u32::try_from(level % 16).expect("level mod fits")),
                Side::Bid,
                OrderType::Limit,
                100 + level,
                10,
                None,
                &mut events,
            )
            .expect("setup bid rests");
            storage.clear();
        }
        for _ in 0..50 {
            id += 1;
            let mut events = EventBuffer::new(&mut storage);
            book.add_order(
                OrderId(id),
                ParticipantId(u32::try_from(level % 16).expect("level mod fits")),
                Side::Ask,
                OrderType::Limit,
                221 + level,
                10,
                None,
                &mut events,
            )
            .expect("setup ask rests");
            storage.clear();
        }
    }
    (book, storage)
}

/// The participant the mass cancel removes, and the exact number
/// of orders it holds on the occupied book.
const TARGET: ParticipantId = ParticipantId(1);
const TARGET_ORDERS: u64 = 800;

/// The occupied book with event storage reserved for the whole
/// mass cancel: every cancelled order emits one event, so the
/// buffer must hold all 800 at once.
fn occupied_book_for_mass_cancel() -> (Book, Vec<ExecutionEvent>) {
    let (book, mut storage) = occupied_book();
    storage.reserve(usize::try_from(TARGET_ORDERS).expect("order count fits"));
    (book, storage)
}

fn bench_mass_cancel(c: &mut Criterion) {
    {
        let mut group = c.benchmark_group("mass_cancel");
        group.throughput(Throughput::Elements(TARGET_ORDERS));
        group.bench_function("cancel_by_participant", |b| {
            b.iter_batched(
                occupied_book_for_mass_cancel,
                |(mut book, mut storage)| {
                    let mut events = EventBuffer::new(&mut storage);
                    let cancelled = book
                        .cancel_by_participant(TARGET, &mut events)
                        .expect("mass cancel applies");
                    assert_eq!(
                        u64::try_from(cancelled).expect("count fits"),
                        TARGET_ORDERS,
                        "the target participant holds exactly 800 orders"
                    );
                    black_box(cancelled);
                    storage.clear();
                    (book, storage)
                },
                BatchSize::LargeInput,
            );
        });
        group.finish();
    }
}

criterion_group!(benches, bench_mass_cancel);
criterion_main!(benches);
