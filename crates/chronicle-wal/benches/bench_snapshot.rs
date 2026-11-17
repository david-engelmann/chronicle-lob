//! INV-LAT-8 (ARCHITECTURE.md section 13): snapshot round trip
//! throughput, one encode and decode of a snapshot state captured
//! from a book holding 12,000 live orders, measured in bytes per
//! second over the encoded snapshot length.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType, ParticipantId, Side,
    StpPolicy,
};
use chronicle_wal::{decode_snapshot, encode_snapshot};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

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
/// levels with 50 orders each and a two-tick spread, the same
/// occupancy shape the chronicle-core benches use.
fn occupied_book() -> Book {
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
    book
}

fn bench_snapshot(c: &mut Criterion) {
    let config = bench_config();
    let state = occupied_book().capture_snapshot_state();
    let mut probe: Vec<u8> = Vec::new();
    encode_snapshot(&state, &mut probe);
    let encoded_len = u64::try_from(probe.len()).expect("snapshot length fits");
    {
        let mut group = c.benchmark_group("snapshot");
        group.throughput(Throughput::Bytes(encoded_len));
        group.bench_function("encode_decode_round_trip", |b| {
            b.iter(|| {
                let mut out: Vec<u8> = Vec::new();
                encode_snapshot(black_box(&state), &mut out);
                let decoded = decode_snapshot(black_box(&out), black_box(&config))
                    .expect("the encoded snapshot decodes");
                black_box(decoded);
            });
        });
        group.finish();
    }
}

criterion_group!(benches, bench_snapshot);
criterion_main!(benches);
