# chronicle-wal

The write-ahead log, snapshots, and recovery for chronicle-core order books, written in Rust.

## What it does

chronicle-wal persists accepted commands, never events, as CRC32C framed records with a 32 byte header and fixed width payloads. The reader implements a fixed decision table, so a torn tail left by a crash is a reported boundary and anything worse is a typed corruption error at an exact offset.

Snapshots are a structural dump of the book, saved by an atomic rename protocol with an optional caller-reused buffer (`save_snapshot_with_buffer`) and validated field by field on load. Recovery rebuilds a book from a snapshot plus the log tail that follows it, and the recovered book is proven equal in state to the live run it came from.

Reads and recovery also run in bounded-memory streaming form. `scan_reader_with` and `recover_from_reader` apply the frame decision table incrementally to 8 KiB chunks from any reader, so a multi-gigabyte log never has to sit in memory at once.

The crate forbids unsafe code. Its recovery path reads no clock, uses no randomness, and iterates no hash maps, so recovery is as deterministic as the book it rebuilds.

## How it is used

Add the crate alongside chronicle-core, append accepted commands through a `WalWriter`, and inspect or rebuild state with the streaming reader and recovery entry points.

```toml
[dependencies]
chronicle-core = "0.3"
chronicle-wal = "0.3"
```

```rust
use std::io::{self, Cursor};

use chronicle_core::{BookConfig, OrderId, OrderType, ParticipantId, Side, StpPolicy};
use chronicle_wal::{
    SyncPolicy, WalCommand, WalSink, WalWriter, recover_from_reader, scan_reader_with,
};

#[derive(Default)]
struct MemorySink(Vec<u8>);

impl io::Write for MemorySink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WalSink for MemorySink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

let config = BookConfig {
    max_orders: 64,
    grid_len: 640,
    tick_size: 1,
    grid_start_tick: 1,
    default_stp: StpPolicy::CancelIncoming,
};
let mut writer = WalWriter::new(MemorySink::default(), SyncPolicy::Never);
let add = WalCommand::Add {
    order_id: OrderId(1),
    participant: ParticipantId(1),
    side: Side::Bid,
    order_type: OrderType::Limit,
    stp: None,
    price: 100,
    qty: 10,
};
writer.append(&add, 1)?;
let wal_bytes = writer.into_inner().0;

let report = scan_reader_with(Cursor::new(&wal_bytes), |_frame| Ok(()))?;
assert_eq!(report.frame_count, 1);

let recovery = recover_from_reader(config, None, Cursor::new(&wal_bytes))?;
assert_eq!(recovery.book.best_bid(), Some((100, 10)));
# Ok::<(), chronicle_wal::WalError>(())
```

The log and snapshot formats are specified in `docs/ARCHITECTURE.md` sections 9 and 10 in the repository.

## Where it fits

chronicle-wal is the persistence layer of the chronicle-lob workspace. It persists books built with `chronicle-core`, and `chronicle-lob-cli` exposes its verify, replay, snapshot, and checkpoint operations from the command line.

## License

MIT OR Apache-2.0
