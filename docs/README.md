# chronicle-lob documentation

Reference documentation for the `chronicle-lob` matching engine, write-ahead log, and operator command line tool.

## Start here

| If you want to... | Read |
| --- | --- |
| Try the library or CLI in five minutes | [Root README](../README.md) |
| Use the zero-dependency matching engine crate | [chronicle-core README](../crates/chronicle-core/README.md) |
| Persist sessions with the write-ahead log and snapshots | [chronicle-wal README](../crates/chronicle-wal/README.md) |
| Verify, replay, checkpoint, or benchmark logs from a shell | [chronicle-lob-cli README](../crates/chronicle-cli/README.md) |

## Design and verification

| Document | What it covers |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Price grid math, 64-ary hierarchical bitmaps, SoA order arena, open-addressed ID table, binary WAL and snapshot layouts, and streaming recovery. |
| [INVARIANTS.md](INVARIANTS.md) | Every formal invariant (`INV-BOOK-*`, `INV-ORD-*`, `INV-WAL-*`, `INV-SNAP-*`, `INV-ALLOC-*`, `INV-DET-1`), adversarial edge cases, and the exact test or benchmark enforcing each row. |
| [PERFORMANCE.md](PERFORMANCE.md) | Recorded Criterion baselines across matching, sweeps, depth walks, WAL framing, snapshot codecs, and crash recovery, with commit and runner provenance (`CHECK-DOC-1`). |
| [DECISIONS.md](../DECISIONS.md) | Locked architectural and licensing choices established before implementation. |

## Operations and project history

| Document | What it covers |
| --- | --- |
| [SECURITY.md](../SECURITY.md) | Supported versions, CRC32C non-cryptographic scope, untrusted-input validation guarantees, and private vulnerability reporting. |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Pinned toolchain setup, local CI gates, determinism rules, and public prose guidelines. |
| [ROADMAP.md](ROADMAP.md) | Completed post-Milestone 4 engineering programmes, deep mutation exercises, and closure records. |
| [CHANGELOG.md](../CHANGELOG.md) | Release-by-release record of additions, fixes, and behavioral changes. |
