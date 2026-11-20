# Changelog

All notable changes to this project are recorded here. The format
follows Keep a Changelog, and the project uses semantic versioning.

## [Unreleased]

### Added

- The CLI checkpoint subcommand. It recovers a book from a log,
  writes a snapshot, verifies the snapshot by loading it back,
  and only then archives the log under the sequence the snapshot
  covers, bounding a long-running log's growth. The loop and its
  safety order are recorded in ARCHITECTURE.md section 10.6
  (programme 3, lane T5).
- Mutation coverage of the streaming decoders. Every seeded
  mutation also runs through the streaming scanner at several
  chunk sizes, through scan_reader, and through streaming
  recovery, and the streaming verdicts and outcomes must match
  the slice forms exactly (programme 4, lane U1).

### Changed

- Performance figures refreshed. docs/PERFORMANCE.md records a
  full measurement pass on the current code, the first since
  the streaming rebuild (programme 4, lane U2).
- Documentation currency. ARCHITECTURE.md section 14 now
  describes the CI that exists, all fifteen jobs and the four
  dispatch workflows, with the macOS test leg's cancellation
  remedy recorded. The README states the published state, the
  streaming reads and recovery, and the platform coverage, and
  no longer says the crates are unpublished (programme 3, lane
  T4).

## [0.2.0] - 2026-11-17

The second public release. All three crates are published on
crates.io at 0.2.0 and verified from the registry and the sparse
index, under direction call DC-4 in docs/ROADMAP.md.

### Added

- Streaming reads in chronicle-wal. LogScanner applies the frame
  decision table incrementally to chunks of bytes, scan_reader
  drives it from any byte reader in bounded memory, and the CLI
  verify and replay commands read through it (programme 2, lane
  S5).
- Streaming recovery. recover_from_reader applies frames as the
  scanner produces them, recover_from_files and the CLI snapshot
  command read through it, and the recovered result is proven
  identical to the slice form (programme 3, lane T1).
- Benchmarks for amend, mass cancel by participant, and the
  snapshot encode and decode round trip, each with its own
  invariant row (programme 2, lane S2).
- macOS and Windows CI legs for the test and examples gates at
  pull request volume, so the supported platforms are exercised
  on every change (programme 2, lane S4).
- Multi-point mutations in the decoder harness and a
  dispatch-only deep mutation workflow that runs one hundred
  thousand cases per surface (programme 2, lane S6).

### Changed

- CI exploration seeds now rotate. The two main seed sweeps
  derive their SIM_SEED from the run number and the mutation
  harness takes its seed from CHRONICLE_MUTATION_SEED, which the
  deep workflow derives per dispatch or accepts as an input. The
  seed in force is printed in every run, so any failure still
  reproduces exactly (programme 3, lane T3).
- The CRC32C checksum now consumes eight bytes per step through
  slicing-by-8 tables built at compile time. WAL append runs at
  1.60 times its former throughput and recovery at 1.26 times,
  with checksum values proven unchanged against a bit-at-a-time
  reference (programme 2, lane S3).
- The add and cancel benchmarks now measure streams of one
  thousand operations per iteration on a pre-built occupied book,
  replacing figures that measured the harness itself (programme
  2, lane S1).
- Snapshot saving on non-Unix platforms is a defined no-op for
  the directory sync step, with the durability semantics stated
  in the architecture document (programme 2, lane S4).

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
