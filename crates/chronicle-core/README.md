# chronicle-core

A single-threaded deterministic limit order book and matching engine with zero steady-state heap allocation, written in Rust with no dependencies.

## What it does

chronicle-core maintains a limit order book on a bounded tick grid and matches orders against it. Orders live in a fixed-capacity struct-of-arrays arena and are named only by `OrderId`. Active price levels are tracked by hierarchical bitmaps, so best bid, best ask, and sparse depth steps each cost a few word reads in `O(depth)` no matter how many ticks sit on the grid. An open-addressed table maps each `OrderId` to its arena slot at a load factor of at most 50 percent.

Every mutating command validates and preflights before it mutates, so every rejection is atomic and leaves the book unchanged. Execution events are written into a caller-owned buffer, and a buffer that is too small is a typed error returned before any state changes. Callers can also inspect live orders and price levels in zero allocation through `get_order`, `contains_order`, `spread`, `level_at`, `orders_at`, and `free_order_count`.

The crate is strictly std-only, forbids unsafe code, reads no clock, and uses no process-seeded randomness. The same command stream always produces the same book state and the same event stream, on every platform.

## How it is used

Add the crate to a Cargo manifest, build a book from a `BookConfig`, and drive it with commands. Results come back as typed errors or as execution events in the caller-owned `EventBuffer`.

```toml
[dependencies]
chronicle-core = "0.3"
```

```rust
use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
};

let mut book = Book::new(BookConfig {
    max_orders: 1024,
    grid_len: 4096,
    tick_size: 1,
    grid_start_tick: 1,
    default_stp: StpPolicy::CancelIncoming,
})?;
let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
let mut events = EventBuffer::new(&mut storage);

book.add_order(
    OrderId(1),
    ParticipantId(7),
    Side::Bid,
    OrderType::Limit,
    100,
    10,
    None,
    &mut events,
)?;
assert_eq!(book.get_order(OrderId(1)).map(|o| o.qty), Some(10));
events.clear();

book.cancel_order(OrderId(1), &mut events)?;
assert_eq!(book.live_order_count(), 0);
# Ok::<(), chronicle_core::BookError>(())
```

The full design contract lives in `docs/ARCHITECTURE.md` in the repository, and the invariants the engine maintains are listed in `docs/INVARIANTS.md`.

## Where it fits

chronicle-core is the deterministic core of the chronicle-lob workspace. `chronicle-wal` persists its command stream in a write-ahead log with snapshots and recovery, and `chronicle-lob-cli` drives books from the command line.

## License

MIT OR Apache-2.0
