# Changelog

All notable changes to this project are recorded here. The format
follows Keep a Changelog, and the project uses semantic versioning.

## [Unreleased]

## [0.3.1] - 2026-11-27

Patch release adding a `[lib]` target (`chronicle_lob_cli`) to `chronicle-lob-cli`
for `docs.rs` documentation and embedded callers, pure-Python `CWAL` v1 and `CSNP` v1
binary decoders symmetric with `WalBuilder` in `chronicle_lob`, canonical GitHub URLs
in `README.md` for PyPI and crates.io rendering, and automated PyPI wheel smoke
verification in `verify-published.yml`.

### Added

- Library target `[lib]` (`chronicle_lob_cli`, `crates/chronicle-cli/src/lib.rs`)
  alongside `chronicle-cli` under `#![forbid(unsafe_code)]`, exposing `run_cli`,
  `parser`, `replay`, `verify`, `snapshot`, `checkpoint`, `bench`, `write_event`,
  `archive_path_for`, `bench_config`, `SplitMix64`, and typed command summaries
  (`BenchSummary`, `VerifySummary`, `VerifyError`, `SnapshotSummary`,
  `CheckpointSummary`) for `docs.rs` and embedded callers.
- Pure-Python `CWAL` v1 and `CSNP` v1 binary decoders in `chronicle_lob`
  (`WalFrame`, `iter_wal_frames`, `read_wal_frames`, `SnapshotHeader`, and
  `read_snapshot_header`) verifying domain-separated Castagnoli CRC32C checksums,
  header invariants, and strictly consecutive nonzero sequence numbers.
- `install-pypi` job in `.github/workflows/verify-published.yml` exercising
  `pip install chronicle-lob==0.3.1` across `bench`, `WalBuilder`,
  `read_wal_frames`, `snapshot`, and `read_snapshot_header`.

### Fixed

- Root `README.md` image sources (`docs/media/...`) and repository links now use
  canonical `raw.githubusercontent.com` and `github.com/.../blob/main/...` URLs so
  PyPI `readme_renderer` and crates.io render every diagram, GIF, and documentation
  link without stripping or 404ing relative paths.

### Changed

- Expanded the `README.md` Python quickstart to show `WalBuilder`,
  `read_wal_frames`, `snapshot`, and `read_snapshot_header` alongside `bench`,
  pinned `actions/checkout` to `v7.0.1` across `.github/workflows/*.yml`, and
  updated `.github/workflows/verify-published.yml` pins to `0.3.1`.

- docs/ROADMAP.md closes programme 29: the 0.3.2
  and 0.3.3 record and the 0.3.3 re-pin are
  landed at a7823237 (PR #112), proof dispatch
  46086960272 successful.
- docs/ROADMAP.md opens programme 29 and records
  the 0.3.2 and 0.3.3 releases, both fully
  published, and verify-published.yml is
  re-pinned to 0.3.3 (lane AT1).
- docs/ROADMAP.md closes programme 28: the 0.3.1
  release state and stranger proofs are landed
  at 64c49d9c (PR #106) and 4bc6f924 (PR #108).
- docs/ROADMAP.md records the programme 28
  stranger proofs at 0.3.1: the release binary
  and PyPI SDK match figure for figure, and the
  SDK's Python decoders decode and convict on
  their own (lane AS2).
- docs/ROADMAP.md opens programme 28 and records
  the 0.3.1 release state: tag at d61cd88b,
  crates.io and PyPI at 0.3.1, GitHub Release
  published with six assets (lane AS1).
- docs/ROADMAP.md closes programme 27: the
  post-launch links audit is landed at 6c86fc51
  (PR #104), clean.
- docs/ROADMAP.md opens programme 27 and records
  the post-launch links audit: 15 files, 12
  relative references resolve, zero broken,
  external references confined to the project's
  own domains (lane AR1).
- docs/ROADMAP.md closes programme 26: the
  performance pass is landed at e4ccc745
  (PR #102).
- docs/ROADMAP.md opens programme 26 and records
  a full performance pass in PERFORMANCE.md:
  bench dispatch 45963423977 at 8f17d9f4, add
  44.02 ns, recovery 402.74 us, no verdict
  claimed across passes (lane AQ1).
- docs/ROADMAP.md closes programme 25: the SDK
  surface-parity audit is landed at 3242a990
  (PR #100), parity holds.
- docs/ROADMAP.md opens programme 25 and records
  the SDK surface-parity audit: five subcommands
  to five functions, five flags to five fields,
  parity holds with two by-design asymmetries
  (lane AP1).
- docs/ROADMAP.md closes programme 24: the clean
  deep mutation exercise is landed at 0d9a4cd3
  (PR #98).
- docs/ROADMAP.md opens programme 24 and records
  a clean deep mutation exercise: dispatch run
  45933787610 at seed 70008, fifteen tests
  passed at 100,000 cases (lane AO1).
- docs/ROADMAP.md closes programme 23: the
  post-launch INVARIANTS traceability re-audit
  is landed at 041398b8 (PR #96), the matrix
  sound.
- docs/ROADMAP.md opens programme 23 and records
  the post-launch INVARIANTS traceability
  re-audit: 53 rows, 114 enforcement names, the
  matrix sound with three names classified
  (lane AN1).
- docs/ROADMAP.md closes programme 22: the
  failure-side proofs are landed at 1036e3f7
  (PR #94).
- docs/ROADMAP.md opens programme 22 and records
  the failure-side proofs: a flipped byte, a
  torn tail, and trailing garbage are convicted
  or discarded identically by the Rust binary
  and the Python SDK (lane AM1).
- docs/ROADMAP.md closes programme 21: the
  cross-channel interop proof is landed at
  b33fb8de (PR #92).
- docs/ROADMAP.md opens programme 21 and records
  the cross-channel interop proof: a Python
  written log verifies, replays, and snapshots
  under the Rust binary, and the SDK verifies
  against the binary-written snapshot (lane AL1).
- docs/ROADMAP.md closes programme 20: the PyPI
  stranger proof is landed at fcf8d8b9 (PR #89),
  and all three publication channels now carry
  a consumption proof.
- docs/ROADMAP.md opens programme 20 and records
  the PyPI channel's stranger proof: the SDK
  installs clean, its bench agrees with the
  release binary, and the full builder, replay,
  verify, snapshot, and checkpoint round trip
  runs (lane AK1).
- docs/ROADMAP.md closes programme 19: the
  shipped-artifact and published-crate proofs
  are landed at 892a4c0f (PR #86) and 0e3c1904
  (PR #87).
- docs/ROADMAP.md records the verify-published
  proof at 0.3.0: dispatch run 45776133389
  completed success on both jobs (lane AJ2).
- docs/ROADMAP.md opens programme 19 and records
  the shipped-artifact verification: the v0.3.0
  binary runs to contract from the release
  tarball, and publication stands complete on
  crates.io, docs.rs, and PyPI (lane AJ1).
- docs/ROADMAP.md closes programme 18: the
  v0.3.0 release-state record is landed at
  6c4b5a2a (PR #84).
- docs/ROADMAP.md opens programme 18 and records
  the verified v0.3.0 release state: GitHub
  Release published with five binary tarballs,
  tag at 0b8c1cca, registry publications still
  outstanding (lane AI1).
- docs/ROADMAP.md closes programme 17: the
  post-launch deep mutation record is landed
  at f8a734f0 (PR #80), with PR #79's changelog
  entry restored after a landing collision.
- docs/ROADMAP.md opens programme 17 and records
  the post-launch deep mutation exercise: clean
  at bf85e526 under seed 70007, fifteen tests at
  100,000 cases per surface (lane AH1).
- docs/ROADMAP.md closes programme 16: the
  post-launch performance record is landed at
  171cf5c2 (PR #77).
- docs/PERFORMANCE.md records the post-launch
  performance pass (bench dispatch run
  45707524841 at 43415ab2), and docs/ROADMAP.md
  opens programme 16 (lane AG1).
- docs/ROADMAP.md closes programme 15: the
  dependency health record is landed at
  fe819dba (PR #75).
- docs/ROADMAP.md opens programme 15 with the
  dependency health pass: 117 locked versions
  checked against the registry, none yanked
  (lane AF1).

## [0.3.0] - 2026-11-25

The third public release. Snapshot decoding and restore now enforce
every structural invariant unconditionally across build profiles,
property and allocation runners honour `SIM_SEED`, CLI verification
and replay stream in bounded `O(1)` memory with multi-cycle
checkpointing, active-price bitmaps traverse sparse grids in
`O(depth)`, and the core book gains zero-allocation inspection
helpers.

### Security

- `snapshot_config` and `decode_snapshot` now validate `BookConfig`
  in the snapshot header alongside positive `order_id` and `qty`,
  in-range neighbour links, unique order identifiers, symmetric
  acyclic level lists, exact per-level counts and quantities, single
  side per level, live-order reachability, and an uncrossed book.
- `Book::from_snapshot_state` now validates every structural
  invariant unconditionally in both debug and release builds before
  any indexing, returning `BookError::InvalidSnapshot` so untrusted
  `SnapshotState` inputs can never panic or construct a cyclic book
  that hangs `plan_sweep`.
- `encode_snapshot` saturates `last_included_seq` when `next_seq` is
  zero instead of panicking, and `mutation.rs` adds CRC-recomputing
  structural mutation harnesses and explicit regressions for zero
  `max_orders`, invalid live-order fields, and self-loop links.
- `SECURITY.md` states the supported versions, the private GitHub
  vulnerability reporting route, and the untrusted decoder scope
  (programme 6, lane W3).

### Added

- Zero-allocation order and level inspection helpers on `Book`
  (`OrderInfo`, `get_order`, `contains_order`, `spread`, `level_at`,
  `orders_at` yielding a double-ended exact-size `OrdersAt` FIFO
  iterator, `free_order_count`, and compact `Debug`), grid price
  helpers `BookConfig::min_price` and `BookConfig::max_price`,
  `EventBuffer::clear`, and `#[derive(Debug)]` on `Recovery`.
- True bounded-memory WAL streaming via `scan_reader_with` and
  `ScanReport` (~8 KiB per-chunk scratch) plus `save_snapshot_with_buffer`
  in `chronicle-wal` so repeated snapshot saves reuse one caller
  buffer without heap growth (`INV-ALLOC-3`).
- Multi-cycle `--snapshot` support on `chronicle-cli checkpoint` and
  `chronicle-cli snapshot`, supporting atomic in-place `--snapshot`
  to `--out` updates across successive log segments.
- The CLI `checkpoint` subcommand, recovering a book from a log,
  writing and verifying a snapshot, and archiving the covered log
  under its sequence number (programme 3, lane T5).
- Tag-triggered `.github/workflows/release.yml`, `.github/workflows/publish.yml`, and `.github/workflows/pypi.yml` automating five-target GitHub Releases, crates.io OIDC publishing, and `chronicle-lob` PyPI wheel publishing alongside a typed Python SDK (`pyproject.toml`, `python/chronicle_lob`) and `[package.metadata.binstall]` metadata in `chronicle-lob-cli`.
- Per-crate `README.md` files wired as doctests in `chronicle-core`
  and `chronicle-wal`, method doctests across core and WAL entry
  points, root `.gitignore`, `.github/dependabot.yml`, and pull
  request and bug report templates (programmes 5 and 7).
- Animated `wal_recovery` demo in `README.md` (`docs/media/demo.gif`)
  and end-to-end checkpoint act in the `wal_recovery` example
  (programmes 4 and 5).
- Public documentation index (`docs/README.md`), light and dark SVG
  lockups and architecture and guarantee diagrams, a `1200x630`
  social card, and a split-screen `docs/media/money-shot.gif` built
  from pinned real outputs.
- Mutation coverage of the streaming decoders across chunk sizes,
  `scan_reader`, and `recover_from_reader` against their slice forms
  (programme 4, lane U1).

### Changed

- All 18 `proptest` properties across `chronicle-core` and
  `chronicle-wal` plus `alloc_proof` now derive deterministic seeds
  from `SIM_SEED`, honour `PROPTEST_CASES`, and print the active seed
  once per test binary (`INV-DET-1`, `INV-DET-2`).
- `Bitmap::highest_below` and `Bitmap::lowest_above` now ascend and
  descend the bitmap hierarchy in `O(bitmap depth)` word reads
  instead of scanning leaf words linearly.
- `chronicle-cli verify` and `chronicle-cli replay` now stream
  through `scan_reader_with` in `O(1)` memory, `replay` writes
  directly through `BufWriter<StdoutLock>` without per-event `String`
  allocations and exits cleanly on `BrokenPipe`, and sequence
  stepping uses `checked_add(1)`.
- Fault-injection hooks `SaveStep` and `save_snapshot_with_hook` are
  marked `#[doc(hidden)]`, and `wal_recovery` cleans up all temporary
  files in a dedicated temp directory before exiting.
- Criterion harnesses in `bench_depth`, `bench_match_sweep`,
  `bench_snapshot`, `bench_recovery`, `bench_add_cancel_best`, and
  `bench_amend` now exclude book drop and buffer reallocation from
  timed iterations, benchmark `recover_from_reader` alongside
  `recover`, and wrap inputs and outputs in `std::hint::black_box`
  under Criterion 0.8.2 (programmes 8 and 9).
- CI `determinism-gate` now scans both `chronicle-core/src` and
  `chronicle-wal/src` for `SystemTime`, `Instant`, `thread_rng`,
  `RandomState`, `DefaultHasher`, `HashMap`, and `HashSet`.
- CI and dispatch workflows cache the Cargo registry and target
  directory under pinned `actions/cache` v6.1.0, `actions/checkout`
  v7, and `actions/upload-artifact` v7, and `verify-published.yml`
  runs weekly on a schedule (programmes 4 through 11).

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
