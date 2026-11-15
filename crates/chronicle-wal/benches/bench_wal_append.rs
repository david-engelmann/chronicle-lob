//! INV-LAT-4 (ARCHITECTURE.md section 13): WAL append throughput,
//! measured in bytes per second against a discarding sink so the
//! number reflects encoding and framing, not the storage device.
//! Throughput is the frame bytes written per append.

#![forbid(unsafe_code)]

use std::io;

use chronicle_core::{OrderId, OrderType, ParticipantId, Side};
use chronicle_wal::{SyncPolicy, WalCommand, WalSink, WalWriter};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

/// A sink that discards bytes and never allocates.
struct NullSink;

impl io::Write for NullSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WalSink for NullSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bench_wal_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal_append");
    // One Add frame is 32 header bytes plus a 40-byte payload.
    group.throughput(Throughput::Bytes(72));
    group.bench_function("append_add_frame", |b| {
        let mut writer = WalWriter::new(NullSink, SyncPolicy::Never);
        let mut seq = 0u64;
        b.iter(|| {
            seq += 1;
            let command = WalCommand::Add {
                order_id: OrderId(seq),
                participant: ParticipantId(1),
                side: Side::Bid,
                order_type: OrderType::Limit,
                stp: None,
                price: 100,
                qty: 1,
            };
            black_box(writer.append(&command, seq))
        });
    });
    group.finish();
}

criterion_group!(benches, bench_wal_append);
criterion_main!(benches);
