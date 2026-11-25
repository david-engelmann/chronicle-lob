//! INV-LAT-3 (ARCHITECTURE.md section 13): depth query cost
//! against the number of levels returned, measured on a book with
//! 200 active levels. Throughput is the levels returned per query.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use chronicle_core::{
    Book, BookConfig, DepthLevel, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId,
    Side, StpPolicy,
};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 8192,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

/// A book with 100 bid levels and 100 ask levels, ten orders each.
fn depth_book() -> (Book, Vec<DepthLevel>) {
    let mut book = Book::new(bench_config()).expect("book");
    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut id = 0u64;
    for level in 0..100u64 {
        for _ in 0..10 {
            id += 1;
            let mut events = EventBuffer::new(&mut storage);
            book.add_order(
                OrderId(id),
                ParticipantId(3),
                Side::Bid,
                OrderType::Limit,
                100 + level,
                10,
                None,
                &mut events,
            )
            .expect("setup bid rests");
            storage.clear();
            id += 1;
            let mut events = EventBuffer::new(&mut storage);
            book.add_order(
                OrderId(id),
                ParticipantId(3),
                Side::Ask,
                OrderType::Limit,
                201 + level,
                10,
                None,
                &mut events,
            )
            .expect("setup ask rests");
            storage.clear();
        }
    }
    let mut out: Vec<DepthLevel> = Vec::new();
    out.resize_with(100, || DepthLevel {
        price: 0,
        total_qty: 0,
        order_count: 0,
    });
    (book, out)
}

fn bench_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("depth");
    group.throughput(Throughput::Elements(100));
    group.bench_function("depth_100_levels", |b| {
        b.iter_batched(
            depth_book,
            |(book, mut out)| black_box(book.depth(Side::Bid, 100, &mut out)),
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_depth);
criterion_main!(benches);
