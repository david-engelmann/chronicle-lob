# Frequently Asked Questions

Common questions about `chronicle-lob`'s data structures, binary persistence formats, determinism rules, and operational tooling.

## Why are prices `u64` integers on a bounded tick grid instead of floating-point decimals?

IEEE-754 binary floating-point numbers cannot represent common decimal tick sizes such as `0.01` or `0.0005` exactly. Over long sessions, rounding residue causes crossed-book edge cases, inconsistent notional checks, and platform-dependent replay divergence.

`chronicle-core` represents prices and quantities as exact `u64` integers in the market's smallest quote and lot units, using checked `u128` intermediates whenever price and quantity multiply (`INV-MATH-1`). Placing valid ticks on a bounded integer grid (`grid_start_tick`, `tick_size`, `grid_len`) also lets active price levels map directly onto a 64-ary hierarchical bitmap (`INV-BITMAP-1`, `INV-BITMAP-2`), making best-bid, best-ask, and sparse neighbour lookups cost a small constant number of 64-bit word reads (`O(bitmap depth)`) regardless of how sparse the book becomes.

## How does `chronicle-lob` achieve zero steady-state heap allocation?

General-purpose order books built on `BTreeMap<Price, VecDeque<Order>>` allocate tree nodes, grow ring buffers, and rehash lookup tables during bursty order flow. `chronicle-core` fixes every memory structure at `Book::new` time (`INV-ALLOC-1`, `INV-ALLOC-2`):

- **Struct-of-arrays (SoA) order arena** preallocates parallel columns up to `max_orders`, addressed internally by 32-bit slot indices and validated by per-slot generation counters (`INV-ARENA-3`).
- **Intrusive doubly linked level queues** thread resting orders through `prev` and `next` `u32` slot columns inside the arena, giving `O(1)` queue removal on cancel or fill without touching the heap.
- **Open-addressed `OrderId` lookup** preallocates a power-of-two table capped at 50 percent load (`INV-LOOKUP-1`) using a fixed `SplitMix64` finalizer (`INV-LOOKUP-3`) and Knuth Algorithm R backward-shift deletion (`INV-LOOKUP-2`), leaving zero tombstones so probe lengths never degrade over long sessions.
- **Caller-owned `EventBuffer`** writes `ExecutionEvent` values into a preallocated slice supplied by the caller (`INV-EVENT-1`).
- **Reused persistence buffers** in `chronicle-wal` reuse encode storage across `WalWriter::append` and `save_snapshot_with_buffer` calls (`INV-ALLOC-3`).

Dedicated integration tests (`alloc_zero_steady_state_1m_ops` in `chronicle-core` and `alloc_proof` in `chronicle-wal`) wrap a thread-local counting `GlobalAlloc` over one million mixed operations and assert a zero allocation delta.

## Why does the write-ahead log store accepted commands instead of execution events?

Persisting accepted `WalCommand` frames (`Add`, `Cancel`, `CancelByParticipant`, `Amend`) rather than derived `ExecutionEvent` streams keeps every frame compact and fixed-width (`48` to `72` bytes including the 32-byte `CWAL` v1 header). Because `chronicle-core` is strictly deterministic (`INV-DET-1`), replaying the accepted command log from either a fresh `BookConfig` or an earlier `CSNP` v1 snapshot reconstructs both the exact resting book state and the exact execution event stream (`INV-WAL-5`).

Rejected commands mutate nothing and consume no sequence number (`INV-ORD-4`), so they are never appended to the write-ahead log.

## How does crash recovery tell a torn write apart from file corruption?

`chronicle-wal` applies the deterministic decision table in [ARCHITECTURE.md](ARCHITECTURE.md) section 9.5 (`INV-WAL-2`):

- **Clean crash boundary:** If an unexpected end-of-file occurs either inside a trailing header prefix whose present fixed bytes (`CWAL` magic, version `1`, `header_len = 32`, zero reserved bytes) are all valid, or inside the payload bytes of the final frame before all declared bytes arrived, the scanner treats those trailing bytes as an incomplete write interrupted by power loss or process death. Recovery succeeds up to the last complete frame and reports the exact `discarded_trailing_bytes` count.
- **Hard corruption:** Any bad magic, unsupported version, nonzero reserved byte, invalid command tag, sequence gap (`INV-WAL-4`), or Castagnoli CRC32C mismatch (`INV-WAL-3`) on a complete frame, or any malformed header byte anywhere in the file, returns a typed `WalError` naming the exact byte offset. `chronicle-cli verify` exits with status `1`.

## Can `chronicle-wal` verify and replay logs larger than RAM?

Yes. `LogScanner`, `scan_reader_with`, and `recover_from_reader` stream input in fixed 8 KiB chunks. Only the undecided bytes of at most one trailing frame sit in the scanner's scratch buffer at any instant, so verifying or replaying a multi-gigabyte session log uses roughly 8 KiB of streaming buffer memory on top of the configured `Book` arena.

## When does amending an order preserve FIFO time priority?

`Book::amend_order` follows standard exchange price-time rules (`INV-AMEND-1`, `INV-AMEND-2`):

- **Quantity reduction at the same price** (`new_price == old_price` and `0 < new_qty < old_qty`) updates the order in place and preserves its exact FIFO queue position.
- **Identical price and quantity** (`new_price == old_price` and `new_qty == old_qty`) leaves queue position and book aggregates unchanged while consuming one sequence number and emitting one `Amended` event.
- **Quantity increase or price change** moves the order to the tail of the target price level behind existing resting liquidity.
- **Crossing amend** (a bid amended at or above `best_ask`, or an ask amended at or below `best_bid`) is rejected atomically with `BookError::PriceCrossesSpread` (`INV-BOOK-1`, `INV-ORD-4`).

## Why is the Cargo package named `chronicle-lob-cli` while the binary is `chronicle-cli`?

The package name `chronicle-cli` was already registered on crates.io. Publishing the crate as `chronicle-lob-cli` while keeping the binary target named `chronicle-cli` means every installation channel (`install.sh`, `cargo install chronicle-lob-cli`, `cargo binstall chronicle-lob-cli`, and `pip install chronicle-lob`) puts the same `chronicle-cli` executable on your `PATH`.

## Is CRC32C sufficient against untrusted or adversarial snapshot and WAL files?

Castagnoli CRC32C detects accidental bit flips, torn writes, and storage corruption, and uses separate domain prefixes (`chronicle-wal/v1\0` and `chronicle-snapshot/v1\0`) so WAL frames and snapshots cannot be confused (`INV-WAL-3`, `INV-SNAP-1`). CRC32C is **not** a cryptographic MAC; an attacker with write access to disk can recompute valid checksums. Use filesystem permissions or external cryptographic signatures to authenticate untrusted archives.

Even when an adversary recomputes valid CRC32C trailers over crafted bytes, both `decode_snapshot` and `Book::from_snapshot_state` validate every structural invariant (header arithmetic, slot generations, free-list exactness, doubly linked level symmetry, level aggregate sums, single-side levels, and uncrossed spread) unconditionally in both debug and release builds (`INV-SNAP-1`, `INV-WAL-7`), returning a typed error rather than panicking, looping, or exposing corrupt state.

## Can I run multiple instruments or multiple threads?

One `Book` models one instrument on one tick grid on a single deterministic thread. Production systems with multiple instruments shard horizontally by symbol: run one independent `(Book, WalWriter)` pair per instrument on its pinned worker thread or single-threaded event loop, with zero cross-book lock contention.

## Where are the latency and throughput benchmarks?

Per `CHECK-DOC-1` in [INVARIANTS.md](INVARIANTS.md), measured Criterion baselines live exclusively in [PERFORMANCE.md](PERFORMANCE.md), paired with the exact benchmark target name, book occupancy, git commit hash, and runner hardware class. To run the deterministic synthetic workload locally on your own hardware, use `cargo bench --workspace --locked` for Criterion microbenchmarks or `chronicle-cli bench --ops 100000 --seed 42 --json` for a quick smoke summary.
