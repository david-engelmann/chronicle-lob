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
- A README for each crate, stating what the crate is, what it
  does, and how it is used, so the crates.io pages render a
  readme for chronicle-core, chronicle-wal, and
  chronicle-lob-cli (programme 5, lane V6).
- The wal recovery example now runs the checkpoint composition
  end to end on a copy of its session log: stream-recover,
  snapshot, verify by loading back, archive the covered log,
  start a fresh log, and recover across the seam, with the
  output pinned for CI (programme 4, lane U5).

### Added

- docs/ROADMAP.md records the programme 5 lane V5 cache
  medians (six warm runs per event type) with the
  fleet-template verdict, and closes programme 5.
- The README carries an animated demo of the wal_recovery
  worked example (docs/media/demo.gif, with a still poster
  frame), produced under the Stageshow boundary as demo
  material; every line shown is the example's real,
  CI-pinned output (programme 5, lane V3).

### Added

- docs/ROADMAP.md opens programme 11 with the
  merge-integrity sweep: every squash merge from
  programme 7 onward carries exactly its intended
  files, the repaired Y1 revert excepted (lane AB1).
- docs/ROADMAP.md closes programme 10: the reverted
  action pins are restored at 502404d8 (PR #65).
- The actions/checkout v7 and actions/upload-artifact
  v7 pins from PRs #55 and #56 are restored; lane Y1's
  stale-base tree had silently reverted them
  (programme 10, lane AA1).
- docs/ROADMAP.md closes programme 9: the criterion
  0.8 performance pass landed at a84c491c (PR #63).
- docs/PERFORMANCE.md records a full measurement pass
  under criterion 0.8.2 (programme 9, lane Z1; bench
  dispatch run 45601655307 at bcc3f1c5).
- docs/ROADMAP.md closes programme 8: the criterion
  0.8 migration landed at 5202b926 (PR #61) and the
  cache v6 re-baseline at 817b61df (PR #53), verdict
  keep v6 on the sampled evidence.
- The benchmark targets move to criterion 0.8.2, with
  black_box imported from std::hint as criterion 0.8
  requires (programme 8, lane Y1; supersedes PR #54).
- docs/ROADMAP.md closes programme 7, with X1 at
  f6e6505f, the X2 record at baacc715, X3 at e0e0af81,
  and the first dependabot queue worked (four bumps
  landed, two held with reasons stated).
- A pull request template carrying the review-readiness
  block and a bug-report issue template asking for the
  seed and command stream a reproduction needs
  (programme 7, lane X3).
- GitHub Releases now exist for v0.1.0 and v0.2.0, with
  notes drawn from their CHANGELOG sections
  (programme 7, lane X2).
- .github/dependabot.yml configures weekly version
  updates for the cargo and github-actions ecosystems,
  minor and patch grouped (programme 7, lane X1).
- docs/ROADMAP.md closes programme 6, with W1 at
  5e04b7e1, the W2 record at 9dca673c, and W3 at fa5ffbee.
- SECURITY.md states the supported versions, the private
  reporting route (GitHub private vulnerability reporting,
  now enabled on the repo), and the decoder scope
  (programme 6, lane W3).

### Changed

- The deep mutation exercise ran at the current main head
  (run 45534405839, seed 70005, all eight tests passed at
  100,000 cases per surface), recorded in docs/ROADMAP.md
  (programme 6, lane W2).
- ARCHITECTURE.md section 14 names the cache rollout to
  the dispatch workflows (programme 6, lane W1).
- docs/ROADMAP.md records the programme 5 lane V4 audit
  outcome: the INVARIANTS traceability matrix was
  cross-checked against the tree (54 rows, 120 enforcement
  names) and is sound, with no repairs required.
- ARCHITECTURE.md section 14 describes the CI cache step and
  the weekly published-crate verification, and docs/ROADMAP.md
  opens programme 5 (programme 5, lane V1).
- The three compiling dispatch workflows (bench, mutation-deep,
  publish) cache the cargo registry and the target directory
  with the same pinned cache step as CI, the bench family for
  the two release-profile workflows and the package family for
  publish (programme 5, lane V2).
- CI caches the cargo registry and the target directory in
  the ten compiling jobs, keyed by operating system, job
  family, and the lockfile and toolchain hashes, with a
  three-level restore fallback (Rust CI cache pilot).
- CONTRIBUTING.md describes the landing route that exists
  (branch, hosted gate green on the exact head, squash merge
  with the merged tree verified identical to the green head)
  and states the rule that every landing lane adds its
  changelog entry in the same commit (programme 4, lane U4).
- Performance figures refreshed. docs/PERFORMANCE.md records a
  full measurement pass on the current code, the first since
  the streaming rebuild (programme 4, lane U2).
- The published-crate verification now also runs weekly on a
  schedule, so a toolchain or registry change that breaks
  stranger consumption surfaces on its own (programme 4, lane
  U3).
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
