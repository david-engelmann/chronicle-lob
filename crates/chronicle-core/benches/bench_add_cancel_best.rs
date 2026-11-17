//! INV-LAT-1 (ARCHITECTURE.md section 13): add, cancel, and
//! best-price latency on a book at realistic occupancy, 12,000 live
//! orders across 240 active levels. Throughput is counted per
//! operation. Add and cancel are measured as streams of 1,000
//! operations on one pre-built occupied book (programme 2, lane
//! S1): the first baseline's single-operation batches rebuilt the
//! book per batch and charged the setup to the operation.

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

/// Operations per measured stream (lane S1).
const STREAM: u64 = 1_000;

/// First id of the stream window. The occupied book uses ids up to
/// 12,000 and its capacity is 32,768, so the window always fits
/// and never collides.
const STREAM_FIRST_ID: u64 = 20_000;

/// Add the stream's 1,000 resting bids to `book`, spread across
/// the occupied bid levels so the stream touches the same level
/// machinery a mixed flow would.
fn add_stream(book: &mut Book, storage: &mut Vec<ExecutionEvent>) {
    for i in 0..STREAM {
        let mut events = EventBuffer::new(&mut *storage);
        book.add_order(
            OrderId(STREAM_FIRST_ID + i),
            ParticipantId(1),
            Side::Bid,
            OrderType::Limit,
            100 + (i % 120),
            5,
            None,
            &mut events,
        )
        .expect("stream add rests");
        storage.clear();
    }
}

/// The occupied book with the stream's orders resting in it, as
/// the cancel bench's setup.
fn occupied_book_with_targets() -> (Book, Vec<ExecutionEvent>) {
    let (mut book, mut storage) = occupied_book();
    add_stream(&mut book, &mut storage);
    (book, storage)
}

fn bench_add_cancel_best(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_cancel_best");
    group.throughput(Throughput::Elements(STREAM));
    group.bench_function("add_resting", |b| {
        b.iter_batched(
            occupied_book,
            |(mut book, mut storage)| {
                add_stream(&mut book, &mut storage);
                (book, storage)
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function("cancel", |b| {
        b.iter_batched(
            occupied_book_with_targets,
            |(mut book, mut storage)| {
                for i in 0..STREAM {
                    let mut events = EventBuffer::new(&mut storage);
                    book.cancel_order(OrderId(STREAM_FIRST_ID + i), &mut events)
                        .expect("stream cancel removes its target");
                    storage.clear();
                }
                (book, storage)
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();

    let (book, _storage) = occupied_book();
    let mut group = c.benchmark_group("best_bid_ask");
    group.throughput(Throughput::Elements(1));
    group.bench_function("best_bid_ask", |b| {
        b.iter(|| black_box((book.best_bid(), book.best_ask())));
    });
    group.finish();
}

criterion_group!(benches, bench_add_cancel_best);
criterion_main!(benches);
