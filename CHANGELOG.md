# Changelog

All notable changes to this project are recorded here. The format
follows Keep a Changelog, and the project uses semantic versioning.

## [Unreleased]

## [0.1.0] - 2026-11-16

The first public release. The four-milestone build is complete and
gated, and all three crates are published on crates.io at 0.1.0
and verified from the registry, the docs builds, and a hosted
scratch consumer built against the published packages.

### Added

- chronicle-core, a single-threaded deterministic limit order book
  and matching engine with zero steady-state allocation, proven by
  a counting allocator over 1,000,000 operations.
- chronicle-wal, a CRC32C write-ahead log with atomic point-in-time
  snapshots and recovery that validates sequence, framing, and
  checksums before applying a single frame.
- The chronicle-cli tool (package chronicle-lob-cli) with replay,
  verify, snapshot, and bench subcommands, all seeded and byte
  deterministic across processes.
- Criterion benchmarks for the matching hot paths, WAL append, and
  recovery, compile-gated in CI.
- A post-M4 roadmap (docs/ROADMAP.md) ordering publication
  readiness, examples, documentation depth, property and seed
  coverage growth, performance baselines, and decoder hardening.
