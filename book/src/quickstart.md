# Quickstart

You can use `chronicle-lob` in three ways: install the standalone `chronicle-cli` binary for inspecting and replaying logs, install the `chronicle-lob` Python package for scripting and research workflows, or embed `chronicle-core` and `chronicle-wal` directly inside a Rust application.

## 1. Install the CLI or Python SDK

Install a precompiled, SHA-256-verified `chronicle-cli` release binary on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/david-engelmann/chronicle-lob/main/install.sh | sh
```

Or install via Cargo, `cargo-binstall`, or `pip`:

```sh
# Build and install from crates.io
cargo install chronicle-lob-cli

# Or fetch prebuilt release archives via cargo-binstall
cargo binstall chronicle-lob-cli

# Or install both chronicle-cli and the Python SDK from PyPI
pip install chronicle-lob
```

Verify the installation with a deterministic synthetic benchmark run:

```sh
chronicle-cli --version
chronicle-cli bench --ops 100000 --seed 42 --json
```

## 2. Embed `chronicle-core` in Rust

Add `chronicle-core` (and optionally `chronicle-wal`) to your `Cargo.toml`:

```toml
[dependencies]
chronicle-core = "0.3"
chronicle-wal = "0.3"
```

Construct a `Book` with fixed arena capacity (`max_orders`) and integer tick grid parameters (`grid_len` must be a multiple of `64`), preallocate one `Vec<ExecutionEvent>` buffer for steady-state reuse, and submit commands:

```rust
use chronicle_core::{
    Book, BookConfig, DepthLevel, EventBuffer, ExecutionEvent, OrderId,
    OrderType, ParticipantId, Side, StpPolicy,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BookConfig {
        max_orders: 1024,
        grid_len: 4096,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    };
    let mut book = Book::new(config)?;

    // Allocate event storage and L2 depth scratch once outside the hot path.
    let mut event_storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut bid_depth = [DepthLevel {
        price: 0,
        qty: 0,
        order_count: 0,
    }; 5];

    // 1. Participant 10 rests a passive PostOnly bid for 12 lots at price 100.
    {
        let mut events = EventBuffer::new(&mut event_storage);
        book.add_order(
            OrderId(1),
            ParticipantId(10),
            Side::Bid,
            OrderType::PostOnly,
            100,
            12,
            None,
            &mut events,
        )?;
    }
    event_storage.clear();

    // 2. Participant 20 rests a Limit ask for 8 lots at price 102.
    {
        let mut events = EventBuffer::new(&mut event_storage);
        book.add_order(
            OrderId(2),
            ParticipantId(20),
            Side::Ask,
            OrderType::Limit,
            102,
            8,
            None,
            &mut events,
        )?;
    }
    event_storage.clear();

    // 3. Participant 30 sends an IOC buy for 5 lots at 102: fills 5 against OrderId(2).
    {
        let mut events = EventBuffer::new(&mut event_storage);
        let written = book.add_order(
            OrderId(3),
            ParticipantId(30),
            Side::Bid,
            OrderType::Ioc,
            102,
            5,
            None,
            &mut events,
        )?;
        assert_eq!(written, 2); // Trade, Filled
    }
    event_storage.clear();

    // Zero-allocation top-of-book and L2 depth inspection.
    assert_eq!(book.best_bid(), Some((100, 12)));
    assert_eq!(book.best_ask(), Some((102, 3)));
    assert_eq!(book.spread(), Some(2));
    let levels = book.depth(Side::Bid, &mut bid_depth);
    assert_eq!(levels, 1);
    assert_eq!(bid_depth[0].qty, 12);

    Ok(())
}
```

## 3. Persist commands and recover across crashes with `chronicle-wal`

Persist every **accepted** command as a `WalCommand` frame before acknowledging it downstream. `chronicle-wal` frames records with domain-separated Castagnoli CRC32C checksums and supports atomic snapshots and ~8 KiB streaming recovery:

```rust
use std::fs::File;
use std::path::Path;

use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
};
use chronicle_wal::{
    SyncPolicy, WalCommand, WalWriter, recover_from_files, save_snapshot_with_buffer,
};

fn run_durable_session(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let config = BookConfig {
        max_orders: 64,
        grid_len: 640,
        tick_size: 1,
        grid_start_tick: 1,
        default_stp: StpPolicy::CancelIncoming,
    };
    let wal_path = dir.join("session.wal");
    let snap_path = dir.join("session.snap");

    let mut book = Book::new(config)?;
    let mut writer = WalWriter::new(File::create(&wal_path)?, SyncPolicy::Always);
    let mut events_vec: Vec<ExecutionEvent> = Vec::with_capacity(16);
    let mut snap_buf: Vec<u8> = Vec::new();

    let cmd = WalCommand::Add {
        order_id: OrderId(1),
        participant: ParticipantId(7),
        side: Side::Bid,
        order_type: OrderType::Limit,
        stp: None,
        price: 100,
        qty: 10,
    };

    {
        let mut events = EventBuffer::new(&mut events_vec);
        cmd.apply(&mut book, &mut events)?;
    }
    events_vec.clear();
    writer.append(&cmd, 1)?;
    drop(writer);

    // Save an atomic snapshot reusing caller buffer storage.
    let state = book.capture_snapshot_state();
    save_snapshot_with_buffer(&snap_path, &state, &mut snap_buf)?;

    // Rebuild the exact book state from snapshot + WAL tail in bounded memory.
    let recovered = recover_from_files(config, Some(&snap_path), &wal_path)?;
    assert_eq!(recovered.book.best_bid(), book.best_bid());
    assert_eq!(recovered.book.live_order_count(), 1);
    Ok(())
}
```

## 4. Run the repository examples

From a checkout of [`david-engelmann/chronicle-lob`](https://github.com/david-engelmann/chronicle-lob), run all four verified examples:

```sh
# Rust core order-type, amend, cancel, and STP walkthrough
cargo run --locked -p chronicle-core --example book_session

# Rust WAL, snapshot, checkpoint archive, and seam-recovery walkthrough
cargo run --locked -p chronicle-wal --example wal_recovery

# Build chronicle-cli and run the end-to-end Python SDK and CLI drills
cargo build --locked -p chronicle-lob-cli --bin chronicle-cli
python3 examples/market_session_and_replay.py
sh examples/crash_recovery_drill.sh
```

Next, read **[CLI and Python SDK](./cli-and-python.md)** for full operator subcommand semantics, exit codes, and Python binary codec APIs.
