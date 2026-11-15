//! INV-LAT-1 (ARCHITECTURE.md section 13): add, cancel, and
//! best-price latency on a book at realistic occupancy, 12,000 live
//! orders across 240 active levels. Throughput is one element per
//! operation.

#![forbid(unsafe_code)]

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};

fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 32_768,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// A book holding 12,000 resting orders: 120 bid levels and 120 ask
/// levels with 50 orders each and a two-tick spread.
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

fn bench_add_cancel_best(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_cancel_best");
    group.throughput(Throughput::Elements(1));
    group.bench_function("add_resting", |b| {
        b.iter_batched(
            occupied_book,
            |(mut book, mut storage)| {
                let mut events = EventBuffer::new(&mut storage);
                black_box(book.add_order(
                    OrderId(900_001),
                    ParticipantId(1),
                    Side::Bid,
                    OrderType::Limit,
                    219,
                    5,
                    None,
                    &mut events,
                ))
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("cancel", |b| {
        b.iter_batched(
            || {
                let (mut book, mut storage) = occupied_book();
                let mut events = EventBuffer::new(&mut storage);
                book.add_order(
                    OrderId(900_001),
                    ParticipantId(1),
                    Side::Bid,
                    OrderType::Limit,
                    219,
                    5,
                    None,
                    &mut events,
                )
                .expect("setup add rests");
                storage.clear();
                (book, storage)
            },
            |(mut book, mut storage)| {
                let mut events = EventBuffer::new(&mut storage);
                black_box(book.cancel_order(OrderId(900_001), &mut events))
            },
            BatchSize::SmallInput,
        );
    });
    let (book, _storage) = occupied_book();
    group.bench_function("best_bid_ask", |b| {
        b.iter(|| black_box((book.best_bid(), book.best_ask())));
    });
    group.finish();
}

criterion_group!(benches, bench_add_cancel_best);
criterion_main!(benches);
