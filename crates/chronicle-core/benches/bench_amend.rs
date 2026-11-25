//! INV-LAT-6 (ARCHITECTURE.md section 13): amend latency on a
//! book at realistic occupancy, 12,000 live orders across 240
//! active levels. Amends are measured as a stream of 1,000
//! price-and-quantity amends against resting orders on one
//! pre-built occupied book (the lane S1 stream shape), and
//! throughput is counted per amend.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

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
/// the occupied bid levels, as the amend bench's targets.
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
/// the amend bench's setup.
fn occupied_book_with_targets() -> (Book, Vec<ExecutionEvent>) {
    let (mut book, mut storage) = occupied_book();
    add_stream(&mut book, &mut storage);
    (book, storage)
}

/// Amend each stream order once: the price moves 60 levels within
/// the bid band (never across the spread, so every amend stays
/// resting) and the quantity rises from 5 to 7.
fn amend_stream(book: &mut Book, storage: &mut Vec<ExecutionEvent>) {
    for i in 0..STREAM {
        let mut events = EventBuffer::new(&mut *storage);
        let written = book
            .amend_order(
                black_box(OrderId(STREAM_FIRST_ID + i)),
                black_box(100 + ((i + 60) % 120)),
                black_box(7),
                &mut events,
            )
            .expect("stream amend applies");
        black_box(written);
        storage.clear();
    }
}

fn bench_amend(c: &mut Criterion) {
    {
        let mut group = c.benchmark_group("amend");
        group.throughput(Throughput::Elements(STREAM));
        group.bench_function("amend_resting", |b| {
            b.iter_batched(
                occupied_book_with_targets,
                |(mut book, mut storage)| {
                    amend_stream(&mut book, &mut storage);
                    (book, storage)
                },
                BatchSize::LargeInput,
            );
        });
        group.finish();
    }
}

criterion_group!(benches, bench_amend);
criterion_main!(benches);
