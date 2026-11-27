# How chronicle-lob Compares

`chronicle-lob` occupies a deliberate point in the design space: an embeddable, zero-dependency, deterministic single-market limit order book and crash-safe persistence layer with zero steady-state heap allocation. This page explains how it compares to other common order-book and simulation architectures, when to pick it, and what it intentionally leaves out.

## Architectural comparison at a glance

| Dimension | `chronicle-lob` | Heap-allocated `BTreeMap` + `VecDeque` LOB | Ring-buffer matching engine (Disruptor style) | Floating-point bar / L1 backtester | Full exchange appliance / SOR |
| --- | --- | --- | --- | --- | --- |
| **Steady-state heap allocation** | Zero (`INV-ALLOC-1`, counting-allocator proven over 1M ops) | Allocates tree nodes, queue blocks, and hash buckets under load | Zero on preallocated ring slots when paired with an arena book | Frequent array/DataFrame allocations | Varies by vendor |
| **Price representation** | Exact `u64` ticks on bounded grid; checked `u128` notional math | Arbitrary integer or decimal keys (`O(log P)` tree hops) | Fixed-point integers | `f64` floats (subject to IEEE-754 representation drift) | Fixed-point integers |
| **Best bid/ask & sparse depth** | `O(bitmap depth)` via 64-ary hierarchical bitmaps | `O(log P)` tree extrema; pointer-chasing across nodes | Depends on embedded book structure | N/A or top-of-book array slice | Custom price ladders or trees |
| **Cancel / amend by `OrderId`** | `O(1)` arena slot unlink + Knuth Algorithm R open-addressed lookup | `O(1)` map lookup + `O(Q)` scan unless intrusive pointers are added | `O(1)` with intrusive arena | Rarely models individual resting order queues | `O(1)` intrusive arena |
| **Durability & crash recovery** | Built-in `CWAL` v1 CRC32C log, atomic `CSNP` v1 snapshots, ~8 KiB streaming recovery | DIY serialization (often `serde` JSON/bincode without torn-tail framing) | Journaled ring buffer + external snapshotter | Stateless batch files (Parquet/CSV) | Replicated Raft/Aeron cluster |
| **Runtime dependencies (`core`)** | Zero (`std` only, `#![forbid(unsafe_code)]`) | Typically `std` or `rustc-hash` / `indexmap` | Threading, pinning, and lock-free queue crates | NumPy, Pandas, Polars, PyTorch | Full network, auth, and DB stack |

## 1. Compared to heap-allocated `BTreeMap` and `VecDeque` order books

Most open-source Rust, C++, and Python order books store each side as a `BTreeMap<Price, VecDeque<Order>>` paired with a `HashMap<OrderId, Locator>`. In fact, `chronicle-core` includes an independent `BTreeMap` and `VecDeque` implementation (`reference_book.rs`, compiled only under `cfg(test)`) as its differential property-test oracle (`INV-BOOK-2`, `INV-BOOK-4`).

**When a `BTreeMap` book is preferable:**
- You do not know the valid price range ahead of time or need an unbounded sparse price space without configuring `grid_start_tick` and `grid_len`.
- Latency jitter from allocator traffic and pointer chasing across tree nodes does not matter for your workload.

**Why `chronicle-lob` uses a SoA arena and 64-ary bitmaps instead:**
- **Zero steady-state allocation.** Preallocating struct-of-arrays columns (`max_orders`) and hierarchical bitmaps (`grid_len`) eliminates allocator contention and tail-latency spikes during market-open or mass-cancel bursts.
- **`O(1)` queue splice without unsafe pointers.** Intrusive `prev` and `next` `u32` slot indices inside the arena unlink cancelled or filled orders in `O(1)` while preserving `#![forbid(unsafe_code)]`. A plain `VecDeque` requires either tombstones or linear scans when cancelling mid-queue orders.
- **Tombstone-free deterministic lookup.** Standard `HashMap` uses process-randomized SipHash (`RandomState`) and tombstone capacity rules. `chronicle-core` uses a fixed `SplitMix64` finalizer and Knuth Algorithm R backward-shift deletion (`INV-LOOKUP-2`, `INV-LOOKUP-3`), guaranteeing <= 50 percent load, zero tombstones across arbitrary churn, and identical memory state across processes.

## 2. Compared to ring-buffer (LMAX Disruptor) matching engines

High-throughput trading engines frequently organize ingress networking, risk checks, matching, and outbound market-data publishing around lock-free SPSC/MPSC ring buffers (such as the LMAX Disruptor or Aeron Archive).

**How they relate:**
- A ring buffer is an **inter-thread transport and sequencing pipeline**, not an order book data structure. Inside the matching consumer thread, the engine still needs a deterministic state machine that maintains resting liquidity, enforces `Limit` / `Ioc` / `Fok` / `PostOnly` and self-trade prevention (`CancelIncoming`, `CancelResting`, `CancelBoth`), and emits execution reports without allocating.
- `chronicle-core` is designed to sit directly inside that single-threaded matching consumer stage: commands enter from your ring buffer, write typed `ExecutionEvent` records into a preallocated `EventBuffer` sitting on the outbound ring or scratchpad, and never call the allocator, system clock, or OS RNG (`INV-DET-1`).
- If you do not need multi-thread lock-free IPC, `chronicle-wal` gives you standalone single-writer durability, atomic snapshots, multi-cycle checkpointing, and bounded-memory crash recovery out of the box.

## 3. Compared to floating-point and DataFrame backtesters

Quantitative research frameworks written over NumPy, Pandas, or Polars simulate strategies by vectorizing fills over historical OHLCV bars or top-of-book quotes using `f64` prices.

**When a vectorized bar backtester is preferable:**
- You are screening slow cross-sectional signals across thousands of equities over daily or minute bars where queue position and sub-tick rules are irrelevant.

**Where floating-point backtesters break down, and why `chronicle-lob` exists:**
- **Queue priority and partial fills.** Market-making and execution algorithms live or die by FIFO queue position: reducing an order's size preserves its place in line (`INV-AMEND-1`), whereas repricing or increasing size sends it to the back of the queue (`INV-AMEND-2`). Bar backtesters cannot observe whether a passive limit order would have filled ahead of or behind competing liquidity.
- **Atomic order-type and STP interactions.** Fill-or-Kill (`Fok`) availability depends on exact self-trade-prevention adjustments (`INV-ORD-1`), and `PostOnly` orders reject atomically rather than taking liquidity (`INV-ORD-2`).
- **Python + Rust workflow.** Through the `chronicle_lob` Python SDK (`WalBuilder`, `read_wal_frames`, `snapshot`, `replay`), Python researchers can synthesize or convert L3 order streams into checksummed `CWAL` v1 logs, execute deterministic Rust matching and checkpointing, and assert exact event-by-event fills without floating-point drift.

## Explicit non-goals

`chronicle-lob` stays small and auditable by leaving peripheral infrastructure to specialized layers:

1. **No network sockets or exchange wire gateways.** FIX, SBE, OUCH, and ITCH framing, TLS termination, and WebSocket depth fan-out belong in an outer adapter crate or gateway process.
2. **No multi-instrument routing or cross-margin risk engine.** One `Book` instance manages one instrument on one tick grid. Multi-symbol engines instantiate one `Book` per market and keep margin/risk accounting in a dedicated pre-trade gate.
3. **No internal threads, async runtimes, or wall-clock timers.** Time-in-force expirations (such as GTD) are driven by explicit `Cancel` commands from the caller's deterministic sequencer rather than hidden background timers.
4. **No cryptographic authentication inside `CWAL` / `CSNP`.** Castagnoli CRC32C catches accidental disk corruption and torn writes (`INV-WAL-2`, `INV-WAL-3`), while unconditional structural checks reject malformed snapshots (`INV-SNAP-1`). Cryptographic signing or encryption belongs at the storage or archive layer (`SECURITY.md`).

For deeper details on internal layouts and proofs, read [ARCHITECTURE.md](ARCHITECTURE.md) and [INVARIANTS.md](INVARIANTS.md), or see [FAQ.md](FAQ.md) for operational answers.
