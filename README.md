# chronicle-lob

A limit order book and matching engine written in Rust.

An order book is the data structure at the center of a market. It
holds the buy orders and sell orders that are waiting to trade, and
when a new order arrives it works out which waiting orders it trades
with. This crate does that work for one market. You feed it commands
(add an order, cancel an order, change an order) and it hands back a
stream of events that says exactly what happened, in order.

The engine is built to be predictable. It is single threaded, it does
no allocation after startup in its core paths, and it is
deterministic. The same commands in the same order always produce the
same events, byte for byte, on any machine. That makes it usable
anywhere matching logic has to be tested, replayed, or audited later.

## What it does

- Keeps buy and sell orders on a fixed price grid and matches them by
  price first and arrival time second, at the resting order's price.
- Supports four order types. A limit order rests until it fills or is
  cancelled. An immediate-or-cancel order fills what it can now and
  expires the rest. A fill-or-kill order fills in full or not at all.
  A post-only order rests only when its price would not trade at once.
- Amends resting orders. Reducing quantity keeps the order's place in
  line. Increasing quantity or changing price moves it to the back of
  its price level.
- Cancels one order, or every order belonging to one participant in a
  single command.
- Stops an order from trading with the same participant's own resting
  orders, under a self-trade prevention policy chosen per order.
- Reports every outcome as a typed event (accepted, trade, filled,
  cancelled, expired, amended) carrying a sequence number, and answers
  best-price and depth queries for display or downstream use.
- Rejects bad commands with typed errors and no side effects. A
  rejected command changes nothing, writes no events, and does not
  advance the sequence number.

The core crate has no dependencies outside the Rust standard library.

## Current state

The project is being built in four milestones, and is at milestone 3.

- Milestone 1 has landed. The core book, the price grid, limit order
  matching, cancels, and best-price and depth queries.
- Milestone 2 has landed. All four order types, amend, mass cancel
  by participant, and self-trade prevention.
- Milestone 3 is in progress now. The write-ahead log and snapshots,
  so a book can be written to disk as it runs and rebuilt exactly
  after a crash.
- Milestone 4 will add a command line tool for replaying and
  benchmarking a book, plus the final performance proof work.

Only the core crate exists today. The log, the snapshot code, and the
command line tool are not built yet, and nothing here is published to
crates.io yet.

## A short example

```rust
use chronicle_core::{
    Book, BookConfig, EventBuffer, ExecutionEvent, OrderId, OrderType,
    ParticipantId, Side, StpPolicy,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut book = Book::new(BookConfig {
        max_orders: 1024,
        grid_len: 4096,
        tick_size: 1,
        grid_start_tick: 0,
        default_stp: StpPolicy::CancelIncoming,
    })?;

    let mut storage: Vec<ExecutionEvent> = Vec::with_capacity(64);
    let mut events = EventBuffer::new(&mut storage);

    // Rest a sell order for 10 units at price 100.
    book.add_order(
        OrderId(1),
        ParticipantId(7),
        Side::Ask,
        OrderType::Limit,
        100,
        10,
        None,
        &mut events,
    )?;

    // A buy order at the same price trades with it in full.
    book.add_order(
        OrderId(2),
        ParticipantId(8),
        Side::Bid,
        OrderType::Limit,
        100,
        10,
        None,
        &mut events,
    )?;

    // storage now holds Accepted, Trade, Filled, Filled events.
    assert_eq!(book.live_order_count(), 0);
    Ok(())
}
```

Prices are plain integers in the smallest quote unit, and the price
grid in `BookConfig` says which prices are valid. Commands write
their events into a caller-owned buffer, so the caller decides how
much output room each command gets and is told up front when a buffer
is too small.

## Building and testing

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
```

The test suite includes property tests that run the engine against a
simple independent reference implementation and compare the full
event streams. Set `PROPTEST_CASES` to control how many cases they
run. Continuous integration runs 1,000 cases per pull request and
10,000 on the main branch.

## Repository layout

- `crates/chronicle-core` is the engine itself, the only crate so far.
- `docs/ARCHITECTURE.md` is the full design, written for engineers who
  want the data structures, the formats, and the reasoning.
- `docs/INVARIANTS.md` lists every correctness rule the engine
  promises, and names the exact test that enforces each one.
- `DECISIONS.md` records the locked project decisions.

## License

Licensed under either of MIT or Apache-2.0 at your option, see
`LICENSE-MIT` and `LICENSE-APACHE`.
