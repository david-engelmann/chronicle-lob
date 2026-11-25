//! INV-LAT-4 (ARCHITECTURE.md section 13): full recovery
//! throughput, measured in bytes per second over a 5,000-frame log
//! replayed into a fresh book. Throughput is the log bytes read.

#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "criterion_group! generates the bench entry function without documentation"
)]

use std::hint::black_box;
use std::io;

use chronicle_core::{BookConfig, OrderId, OrderType, ParticipantId, Side, StpPolicy};
use chronicle_wal::{SyncPolicy, WalCommand, WalSink, WalWriter, recover};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

/// A sink that collects the encoded log in memory.
struct VecSink(Vec<u8>);

impl io::Write for VecSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WalSink for VecSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const fn bench_config() -> BookConfig {
    BookConfig {
        max_orders: 8192,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    }
}

fn sample_log(frames: u64) -> Vec<u8> {
    let mut writer = WalWriter::new(VecSink(Vec::new()), SyncPolicy::Never);
    for seq in 1..=frames {
        let command = WalCommand::Add {
            order_id: OrderId(seq),
            participant: ParticipantId(1),
            side: Side::Bid,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 1,
        };
        writer.append(&command, seq).expect("append");
    }
    writer.into_inner().0
}

fn bench_recovery(c: &mut Criterion) {
    let bytes = sample_log(5_000);
    let mut group = c.benchmark_group("recovery");
    group.throughput(Throughput::Bytes(
        u64::try_from(bytes.len()).expect("log length fits u64"),
    ));
    group.bench_function("recover_5000_frames", |b| {
        b.iter(|| black_box(recover(bench_config(), None, &bytes)));
    });
    group.finish();
}

criterion_group!(benches, bench_recovery);
criterion_main!(benches);
