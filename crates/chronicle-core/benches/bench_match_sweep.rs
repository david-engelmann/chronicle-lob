//! INV-LAT-2 (ARCHITECTURE.md section 13): match sweep cost per
//! filled maker order, measured on a 60-level sweep that fills 600
//! resting makers with one incoming order. Throughput is the number
//! of maker orders filled.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};

const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 4096,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// A book with 600 ask orders resting across 60 levels, ten per
/// level, 10 units each.
fn sweep_book() -> (Book, Vec<ExecutionEvent>) {
    let mut book = Book::new(bench_config()).expect("book");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(4096);
    let mut id = 0u64;
    for level in 0..60u64 {
        for _ in 0..10 {
            id += 1;
            let mut events = EventBuffer::new(&mut storage);
            book.add_order(
                OrderId(id),
                ParticipantId(7),
                Side::Ask,
                OrderType::Limit,
                100 + level,
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

fn bench_match_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("match_sweep");
    group.throughput(Throughput::Elements(600));
    group.bench_function("sweep_60_levels_600_makers", |b| {
        b.iter_batched(
            sweep_book,
            |(mut book, mut storage)| {
                let mut events = EventBuffer::new(&mut storage);
                black_box(book.add_order(
                    OrderId(900_001),
                    ParticipantId(9),
                    Side::Bid,
                    OrderType::Limit,
                    159,
                    6000,
                    None,
                    &mut events,
                ))
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_match_sweep);
criterion_main!(benches);
