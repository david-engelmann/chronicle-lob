# chronicle-lob

<p>
  <img class="theme-light-only" src="docs/media/chronicle-lockup-light.svg" alt="chronicle-lob" height="50">
  <img class="theme-dark-only" src="docs/media/chronicle-lockup-dark.svg" alt="chronicle-lob" height="50">
</p>

`chronicle-lob` is a single-threaded, deterministic limit order book and matching engine written in safe Rust (`#![forbid(unsafe_code)]`) with zero steady-state heap allocation, paired with a Castagnoli CRC32C write-ahead log (`chronicle-wal`), an operator CLI (`chronicle-cli`), and a pure-Python codec and SDK (`chronicle_lob`).

<p>
  <img src="docs/media/money-shot.gif" alt="chronicle-lob live order book state, atomic CSNP checkpointing, CWAL crash recovery, and counting-allocator proof" width="860">
</p>

## Inside `chronicle-core::Book`

<p>
  <img class="theme-light-only" src="docs/media/book-memory-layout-light.svg" alt="Internal memory layout of chronicle-core::Book showing the 64-ary active-tick bitmap hierarchy, open-addressed SplitMix64 lookup table, and preallocated struct-of-arrays order arena" width="880">
  <img class="theme-dark-only" src="docs/media/book-memory-layout-dark.svg" alt="Internal memory layout of chronicle-core::Book showing the 64-ary active-tick bitmap hierarchy, open-addressed SplitMix64 lookup table, and preallocated struct-of-arrays order arena" width="880">
</p>

- **O(depth) 64-ary active-tick bitmaps.** Separate bid and ask word hierarchies (`depth <= 4` up to `2^22` grid ticks) locate best ask (`trailing_zeros`), best bid (`ilog2`), and L2 neighbours without tree pointer chasing (`INV-BITMAP-1`).
- **Tombstone-free open-addressed lookup.** `SplitMix64` hashing at `<= 50%` load with Knuth Algorithm R backward-shift deletion (`INV-LOOKUP-1..3`) and generation-checked slot handles (`INV-ARENA-3`).
- **Cache-line struct-of-arrays arena.** Hot match-sweep fields (`qty`, `next`, `side`) stream contiguously in 64-byte cache lines while released slots are zeroed before re-entering the LIFO free list (`INV-ARENA-2`).
- **Atomic preflight.** `Limit`, `Ioc`, `Fok`, and `PostOnly` commands validate tick bounds, STP-adjusted `Fok` liquidity, spread crossing, and `EventBuffer` capacity before mutating state (`INV-ORD-1..4`, `INV-EVENT-1`).

## Wire Layout & Streaming Recovery

<p>
  <img class="theme-light-only" src="docs/media/wal-recovery-layout-light.svg" alt="CWAL v1 binary frame byte offsets, Castagnoli CRC32C domain separation, and bounded 8 KiB CSNP snapshot plus WAL crash-recovery seam" width="880">
  <img class="theme-dark-only" src="docs/media/wal-recovery-layout-dark.svg" alt="CWAL v1 binary frame byte offsets, Castagnoli CRC32C domain separation, and bounded 8 KiB CSNP snapshot plus WAL crash-recovery seam" width="880">
</p>

`chronicle-wal` frames accepted commands in 32-byte `CWAL` v1 headers plus fixed-width payloads under domain `"chronicle-wal/v1\0"` and writes structural `CSNP` v1 snapshots under `"chronicle-snapshot/v1\0"`, streaming verification and crash recovery in ~8 KiB of memory (`INV-WAL-2`, `INV-SNAP-1..3`).

## Chapters in This Book

- **[Quickstart](./quickstart.md):** Install `chronicle-cli` or `chronicle-lob`, embed `Book` in Rust, and run end-to-end session persistence.
- **[CLI and Python SDK](./cli-and-python.md):** Subcommands, exit-code contracts, multi-cycle `checkpoint`, and pure-Python `CWAL` / `CSNP` codecs.
- **[Architecture](docs/ARCHITECTURE.md):** Normative specification of every arena column, bitmap rule, wire byte offset, and restore check.
- **[Invariants and Traceability](docs/INVARIANTS.md):** Formal `INV-*` catalog mapped 1-to-1 to property sweeps, fault matrices, and allocator proofs.
- **[Performance Baselines](docs/PERFORMANCE.md):** Recorded Criterion microbenchmark baselines with commit and hardware provenance (`CHECK-DOC-1`).
- **[Comparison](docs/COMPARISON.md) & [FAQ](docs/FAQ.md):** Trade-offs against `BTreeMap` books, Disruptor rings, and DataFrame backtesters, plus operational answers.
