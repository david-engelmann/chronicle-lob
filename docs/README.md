# chronicle-lob Documentation Index

Technical specifications, benchmarks, crate guides, and operational answers for `chronicle-lob`. The rendered multi-chapter documentation book is published at <https://david-engelmann.github.io/chronicle-lob/>.

## Specifications and Benchmarks

| Document | Contents |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Tick grid validation, 64-ary hierarchical bitmaps, struct-of-arrays order arena, SplitMix64 open-addressed lookup, `CWAL` v1 and `CSNP` v1 byte layouts, and ~8 KiB streaming recovery. |
| [INVARIANTS.md](INVARIANTS.md) | Formal invariant catalog (`INV-BOOK-*`, `INV-ORD-*`, `INV-WAL-*`, `INV-SNAP-*`, `INV-ALLOC-*`, `INV-DET-1`) and 1-to-1 test traceability matrix. |
| [PERFORMANCE.md](PERFORMANCE.md) | Recorded Criterion latency and throughput baselines with runner and commit provenance (`CHECK-DOC-1`). |
| [COMPARISON.md](COMPARISON.md) | Architectural trade-offs against heap-allocated `BTreeMap` books, ring-buffer pipelines, and floating-point backtesters, plus explicit non-goals. |
| [FAQ.md](FAQ.md) | Common questions on grid sizing, queue priority, crash boundaries, checkpoints, and Python integration. |

## Crate & Agent Guides

| Document | Contents |
| --- | --- |
| [Root README](../README.md) | Quick Rust embed, memory and wire diagrams, CLI usage, and Python SDK overview. |
| [chronicle-core README](../crates/chronicle-core/README.md) | Zero-dependency matching engine API (`Book`, `BookConfig`, `EventBuffer`, `OrderType`, `StpPolicy`). |
| [chronicle-wal README](../crates/chronicle-wal/README.md) | `CWAL` v1 writer/scanner, atomic `CSNP` v1 snapshots, and `recover_from_reader`. |
| [chronicle-lob-cli README](../crates/chronicle-cli/README.md) | `chronicle-cli` subcommands (`verify`, `replay`, `snapshot`, `checkpoint`, `bench`, `completions`). |
| [examples/README.md](../examples/README.md) | Runnable Python SDK market session and POSIX CLI crash-recovery drills. |
| [AGENTS.md](../AGENTS.md) | Integration guide and repository invariants for AI coding and operations agents. |

## Project Records & Policies

| Document | Contents |
| --- | --- |
| [DECISIONS.md](../DECISIONS.md) | Locked architectural and licensing decisions established prior to implementation. |
| [SECURITY.md](../SECURITY.md) | Supported versions, CRC32C non-cryptographic scope, untrusted-input bounds, and reporting policy. |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Pinned toolchain setup, local CI gates, determinism rules, and Markdown prose checks. |
| [ROADMAP.md](ROADMAP.md) · [CHANGELOG.md](../CHANGELOG.md) | Completed engineering programmes, mutation exercises, and release history. |
