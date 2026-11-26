# Post-M4 roadmap

Status: the four-milestone build (M0 through M4) is complete and fully
gated at main head 387200b1. Hosted CI is green on all eight jobs
including the 10,000-case seed sweep, 112 tests pass, and the M4 gate
re-audit closed the INVARIANTS enforcement census at 65 of 65.
Programme 1 below (lanes R1 through R6) is complete: every lane
landed and verified on main at 38733778. DC-1, the publish act, was
decided by David on 2026-11-16 and exercised the same day: all
three crates are live on crates.io at 0.1.0 and verified firsthand
from the registry, the docs builds, and a hosted stranger
consumption run. Programme 2 follows the direction calls and is
the current working plan.

This roadmap is the working plan for everything after M4. Each lane is
scoped, ordered, and carries the gate it must pass. Lanes land through
the same route the milestones used: contract and tests first where
behaviour changes, a branch and pull request for hosted verification,
and a single-commit landing on main whose tree is identical to the
green branch head. Behaviour changes keep docs/INVARIANTS.md in sync
in the same commit, and any new invariant row lands with the test,
oracle, or benchmark that enforces it.

## Audit findings this roadmap answers

The findings below were verified firsthand against main at 387200b1
and the crates.io index on 2026-11-16.

- A-1. chronicle-wal cannot publish as it stands. Its manifest carries
  no description, license, keywords, categories, or repository, and
  crates.io requires a license and a description.
- A-2. chronicle-wal and the CLI depend on workspace crates by path
  only. Cargo requires a version alongside a path for publication, so
  neither crate can publish until the dependency entries carry one.
- A-3. The name chronicle-cli is already taken on crates.io by an
  unrelated crate. The names chronicle-core, chronicle-wal,
  chronicle-lob, and chronicle-lob-cli are unclaimed.
- A-4. The repo has no CHANGELOG.md and no git tags. There is no
  release record and nothing for a release to point at.
- A-5. The repo has no examples anywhere. A new user can read the API
  documentation and the README walkthrough, but no runnable example
  exists in any crate.
- A-6. The five criterion benchmarks are compile-gated only. No
  baseline numbers are recorded anywhere, by design under CHECK-DOC-1,
  which means performance regressions are currently invisible.
- A-7. The WAL and snapshot decoders parse untrusted bytes and are
  covered by crafted corruption cases, but no systematic mutation
  coverage exists. A panic on hostile input would be a defect in the
  product's core promise.
- A-8. Property coverage is strong in the core and present in the WAL
  recovery tests, but there is no byte-level snapshot round-trip
  property and no seed discipline beyond the two CI volumes. Proptest
  regression files are not checked in because none have been produced
  and no lane is charged with producing them.
- A-9. Found by lane R2 on 2026-11-16. save_snapshot fails with an
  Io error when the snapshot path is a bare file name with no
  parent directory, because the atomic write stages its temp file
  in the parent and syncs it. The same call with an explicit
  parent (./name) succeeds. Whether the library should normalise
  an empty parent itself or the requirement should be documented
  on save_snapshot is a decision for lane R6, whose robustness
  scope covers the wal entry points. Decided in lane R6 on
  2026-11-16: the library normalises an empty parent to the
  current directory inside save_snapshot, so a bare file name
  saves and loads like any other path; the unit test
  snapshot_save_and_load_accept_bare_file_name pins it, and the
  wal example drops its ./ workaround.

## Lane R1, publication readiness (starts first)

Goal: every crate in the workspace is mechanically ready to publish
to crates.io, proven by CI, with the publish act itself the only step
left.

Scope:

- Complete the chronicle-wal manifest (description, license,
  keywords, categories, repository) to the standard chronicle-core
  already meets.
- Add version 0.1.0 alongside the path in every intra-workspace
  dependency entry (chronicle-wal on chronicle-core, the CLI on
  both).
- Rename the CLI package from chronicle-cli to chronicle-lob-cli,
  because the old name is taken (A-3). The binary keeps the name
  chronicle-cli, so installed usage does not change. Recorded as
  decided in-lane in the direction calls below.
- Add CHANGELOG.md with an Unreleased section recording the gated
  M0 through M4 build as the 0.1.0 candidate.
- Add a package job to CI. It runs a full cargo package verification
  build for chronicle-core, and a cargo package --list contents audit
  for chronicle-wal and chronicle-lob-cli. The full verification
  build for the dependent crates resolves their dependencies from
  the registry, so it can only pass once chronicle-core is live; the
  job extends to them in the publish sequence recorded below.

Gate: hosted CI green on the lane head including the new package job,
with the package contents of all three crates read and confirmed
against the intended file sets. Cargo.lock stays consistent under
--locked throughout.

Out of scope: the publish act itself, git tags, and GitHub releases.
Those are the publish sequence, gated on the direction call below.

Publish sequence once the call is made: publish chronicle-core
0.1.0, extend the package job to a full verification build and a
publish dry run for chronicle-wal and publish it, then the same for
chronicle-lob-cli, then tag v0.1.0 at the published main head and cut
the CHANGELOG section.

## Lane R2, examples and worked usage

Goal: a new user can run the engine, the log, and the tool within
minutes, from material inside the repo.

Scope:

- A chronicle-core example that builds a book, runs a short session
  across every order type and self-trade policy, and prints the event
  stream.
- A chronicle-wal example that writes a log during a session, takes
  a snapshot mid-session, and recovers from the pair, printing the
  recovered state next to the live one.
- A worked CLI example in the README that uses the same session the
  CLI tests drive, so the behaviour tests already pin its output.

Gate: the examples build under the workspace clippy and test gates,
and a CI step runs the core and wal examples and compares their
output against pinned expectations checked into the lane. The README
example output matches what the CLI tests assert for the same
session.

## Lane R3, documentation depth

Goal: the public API teaches itself, and the format stability rules
are written down before any 1.x promise makes them load-bearing.

Scope:

- Doctests on the public entry points a user touches first (Book
  construction and order entry, WalWriter append, recover, snapshot
  save and load), each one small and runnable.
- Raise missing_docs from warn to deny in the workspace lints, so
  undocumented public items fail CI instead of accumulating.
- A format stability note in docs/ARCHITECTURE.md covering the WAL
  and snapshot version fields, what a version bump means, and the
  rule that readers reject versions they do not know.

Gate: the rustdoc job passes with -D warnings under the raised lint,
and the doctests run and pass inside the existing test job at both
volumes.

## Lane R4, property and seed coverage growth

Goal: the byte formats get the same property rigour the book already
has, and seed handling becomes a recorded discipline.

Scope:

- A byte-level snapshot round-trip property in chronicle-wal. Random
  live sessions are captured, encoded, decoded, and the decoded state
  must equal the captured state exactly.
- A byte-level recovery equivalence property. A random valid log
  recovers to the same book state the live session reached, compared
  through captured snapshot state.
- Seed discipline. Any proptest regression file a failing run
  produces is checked in with the fix that answers it, and the seed
  sweep gains a second seed so main volume runs two seeds at 10,000
  cases each.
- docs/INVARIANTS.md enforcement cells updated in the same commits,
  naming the new properties where they strengthen an existing row.

Gate: the new properties run inside the existing test and seed sweep
jobs and pass at both volumes, and the INVARIANTS matrix names them,
so the enforcement census stays complete.

## Lane R5, performance baselines and measurement

Goal: performance becomes a measured quantity with a recorded
baseline, so later optimisation is argued from numbers.

Scope:

- A manually dispatched CI workflow that runs the full criterion
  suite and uploads the results as an artifact.
- The first baseline recorded in a docs/PERFORMANCE.md that names
  each benchmark, the book configuration it uses, and the machine
  class (the GitHub-hosted runner), satisfying CHECK-DOC-1.
- Optimisation work only where the baseline indicts a hot path, each
  change carrying before and after criterion output from the same
  workflow. Candidate hypotheses are recorded here so they are not
  lost: snapshot encoding buffer reuse in chronicle-wal, and
  streaming reads in the CLI replay path. Neither is a commitment.

Gate: the baseline document lands with artifact links from a real
workflow run, and any optimisation lane carries its before and after
numbers in its pull request.

## Lane R6, decoder hardening

Goal: hostile bytes can only ever produce typed errors, proven by
systematic mutation rather than by crafted cases alone.

Scope:

- A deterministic mutation harness in chronicle-wal tests. Valid WAL
  and snapshot byte strings are mutated at seeded positions (bit
  flips, truncations, extensions, and field overwrites) and fed to
  scan_log, decode_snapshot, and load paths. Every outcome must be a
  typed WalError or a valid decode, never a panic.
- The harness runs at PR volume in the test job, with positions and
  mutation kinds drawn from the same SplitMix64 discipline the rest
  of the repo uses, so any failure reproduces from its seed.
- A new INVARIANTS row under the WAL section naming the harness,
  landed in the same commit as the harness itself.

Gate: the mutation tests pass at both volumes. Any panic the harness
finds is a blocker fixed inside the lane before it lands, with the
minimised input kept as a regression case.

## Order and dependencies

R1 runs first and alone, because publication metadata touches every
manifest and the lock. R2 and R4 follow in either order. R3 follows
R2, because the raised documentation lint should meet the examples
already written. R5 can start once R1 lands, since its workflow is
independent of the code lanes. R6 runs last among the code lanes,
because its INVARIANTS row should be written against the final shape
of the wal test suite after R4.

## Direction calls

- DC-1, the publish act. Publishing 0.1.0 to crates.io is permanent
  and public, so it waits on David's word. Lane R1 makes everything
  ready and stops there. Nothing else in this roadmap depends on it.
  The crates.io token, when the time comes, is David's credential to
  supply through the secure flow.
  Decided by David on 2026-11-16 and exercised the same day.
  chronicle-core, chronicle-wal, and chronicle-lob-cli are published
  at 0.1.0 in that order, each packaged by the hosted publish
  workflow with its full verification build, and the publication is
  verified from the registry API and index, the docs.rs builds for
  the two library crates, and the verify-published workflow's
  scratch consumer and cargo install runs. The publish and
  verification workflows landed ahead of the act at dedb93b3.
- DC-2, the CLI package name. Decided in-lane under the standing
  delegation, because the old name is taken and the fix is
  mechanical: the package becomes chronicle-lob-cli and the binary
  stays chronicle-cli. The decision is reversible at any point
  before DC-1 is exercised.
- DC-3, none open besides DC-1. Any future call the lanes surface
  gets recorded here in the same commit that raises it, rather than
  stopping the lane that found it.

## Programme 2

Written on 2026-11-16 from a fresh audit of the finished build at
main 38733778, seeded by the two instruments programme 1 built for
exactly this purpose: the performance baseline in
docs/PERFORMANCE.md and the mutation harness of lane R6. Publication
was out of scope for this programme when it was written; DC-1 was
decided and exercised while lane S1 ran, and the programme continues
unchanged behind the publication. Every lane runs under the standing
delegation, through the same staged route as before.

### Audit findings for programme 2

Verified firsthand against main at 38733778 on 2026-11-16.

- B-1. The baseline's add and cancel figures measure the harness as
  much as the operation. Both benches use iter_batched with
  SmallInput, rebuilding the 12,000-order occupied book for every
  batch, so the recorded 123.45 and 117.82 microseconds carry setup
  weight the operation itself does not. The baseline document flags
  this honestly; nothing yet corrects the measurement.
- B-2. The CRC32C implementation is byte-at-a-time: one 256-entry
  table, one byte per step in Crc32c::update. The two throughput
  surfaces the baseline measures, WAL append at 507.22 MiB/s into a
  discarding sink and recovery at 325.00 MiB/s, both run this
  checksum over every byte and do little else per byte.
- B-3. Three hot paths have no benchmark at all. Amend is a core
  operation with real work in it, cancel_by_participant drives the
  mass-cancel path the invariants care about, and snapshot encode
  and decode sit on the durability path. The baseline's seven
  benchmarks cover none of them.
- B-4. Every file read in the stack is whole-file. The CLI calls
  fs::read at five sites, the library's scan and recover APIs are
  slice-based, and read_log reads a whole file too. Log size is
  therefore bounded by memory for the tools and for every library
  consumer.
- B-5. The snapshot save protocol opens the parent directory and
  syncs it. That is a Unix operation: the engine and both formats
  are otherwise platform-neutral, CI runs only Ubuntu, and the
  save path as written would fail on Windows. The portability
  claim is untested anywhere but Linux.
- B-6. The R6 mutation harness mutates one point at a time and
  runs 7,500 mutations per CI run. Multi-point corruption and
  deep volumes are unexplored, and no dispatched deep run exists
  to explore them with.

### Lane S1: honest latency measurement

Rework the add_resting and cancel benches in bench_add_cancel_best
to measure operation streams on one pre-built occupied book: a
fixed run of 1,000 distinct operations per iteration, throughput
counted per operation, batch size large. The workload itself does
not change (12,000 live orders across 240 levels, the same prices
and quantities). best_bid_ask keeps its per-call shape.

Gate: a fresh dispatch of the benchmarks workflow records the new
figures in a dated docs/PERFORMANCE.md section that explains the
harness change and why the new figures supersede the add and
cancel rows of the first baseline for comparison purposes. The
first baseline stays in the file as the historical record.

### Lane S2: benchmark coverage completion

Add the three missing benchmarks: bench_amend (amends against the
occupied book), bench_mass_cancel (cancel_by_participant over the
occupied book), and bench_snapshot (one encode and decode round
trip of a snapshot state at a stated live order count). Each
follows the existing bench conventions, black_box throughout and
Throughput set per operation or per byte as fits.

Gate: the bench compile check passes, and a dispatch records all
three in docs/PERFORMANCE.md alongside the existing table.

### Lane S3: CRC slicing

Replace the byte-at-a-time CRC32C update with slicing-by-8: eight
tables built at compile time beside the existing one, eight bytes
per step, with a byte-at-a-time tail. The checksum values must not
change by a single bit: the golden check value, the domain
separation tests, and the frame and snapshot golden vectors all
pin them already.

Gate: the full suite passes at both volumes with every golden
vector unchanged, and before and after figures for wal_append and
recovery from two dispatches are recorded in docs/PERFORMANCE.md.
If the after figures do not improve on the same machine class,
the lane reports that and reverts rather than landing dead weight.

### Lane S4: platform reach

Add macOS and Windows legs to CI running the test and examples
jobs at PR volume. Fix what surfaces. The known first suspect is
the snapshot directory sync of finding B-5: keep the full
protocol on platforms that support it, give the save path a
defined behaviour where directory sync does not exist, and state
the per-platform durability semantics honestly in
ARCHITECTURE.md sections 10.3 and 10.5.

Gate: all three platforms green on main, and the architecture
text matches the behaviour the code now has.

### Lane S5: streaming reads

Give chronicle-wal a streaming scan: frames yielded from a Read
source through bounded buffers, with the same framing decisions,
offsets, and typed errors as scan_log, and a streaming recovery
path if it composes cleanly with the same error shape. Move the
CLI's verify and replay onto the streaming path so log size stops
being bounded by memory anywhere in the stack. The slice-based
APIs stay exactly as they are.

Gate: the CLI test suite passes byte-identical on every pinned
stream, a new test replays a generated 1,000,000-frame log
through the CLI, and the library suite passes at both volumes.

### Lane S6: mutation depth

Extend the R6 harness with multi-point mutations (two and three
simultaneous mutations drawn from the same seeded stream) at
standard volume in the test job, and add a manually dispatched
deep-mutation workflow on the bench workflow's pattern, running
millions of mutations per surface with the outcome counts in the
run log.

Gate: a deep dispatch completes with zero panics, and the
INV-WAL-7 enforcement cell names the multi-point coverage in the
same commit as the harness change.

### Order and dependencies for programme 2

S1 runs first, because every later performance claim compares
against the numbers it corrects. S2 follows, completing the
coverage before any optimisation is argued from it. S3 is
evidence-led and reads its before figures from the existing
baseline, since the append and recovery benches were directly
measured. S4 and S5 are independent of the performance lanes and
of each other. S6 runs last, extending the harness once the
surfaces it tests have settled.

### Direction calls for programme 2

None open. DC-1 was decided by David on 2026-11-16 and exercised
the same day, as recorded in the programme 1 direction calls, and
no lane in this programme depends on it. The Windows durability
semantics in lane S4 are decided inside the lane and recorded in
ARCHITECTURE.md, per the DC-3 rule.

## Programme 3: the C set (opened 2026-11-17)

A fresh end-to-end audit of the repo as it stood at the close of
programme 2 (main 72f655a1) produced seven findings, C-1 to C-7.
The project review approved the list and the lane order on
2026-11-17, with T6 folded into T4. The findings in brief:

- C-1: recovery is still whole-file. The scan streams since S5,
  but recover() consumes a byte slice, recover_from_files reads
  the whole log, and the CLI snapshot command does the same.
- C-2: the changelog Unreleased section stood empty and the
  version still read 0.1.0 after two programmes of landed work.
- C-3: exploration seeds never rotate. The sweeps run SIM_SEED
  10000 and 10001 on every main push and the mutation harness
  seed is a compile-time constant, so CI re-proves the same
  cases forever.
- C-4: ARCHITECTURE.md section 14 no longer describes the CI
  that exists.
- C-5: the macOS test leg is cancelled runner-side under fleet
  load, repeatedly (five instances on 2026-11-17 alone), and the
  remedy is a manual re-dispatch today.
- C-6: no checkpoint workflow ties snapshots to the WAL, so a
  long-running deployment's log grows without a supported bound.
- C-7: the README says nothing about platform support or
  streaming reads, the two headline programme 2 capabilities.

### Lane T1: streaming recovery (C-1)

recover_from_reader applies frames as the scanner produces
them; recover_from_files and the CLI snapshot command read
through it. Landed at afce63d4 (PR #25).

### Lane T2: release record and 0.2.0 readiness (C-2)

Backfill the changelog for programmes 2 and 3, bump every
manifest and the lock file to 0.2.0, and raise DC-4. Standing
rule from this lane on: every landing lane adds its changelog
entry in the same commit. Landed at f0525b63 (PR #26).

### Lane T3: seed rotation (C-3)

The sweeps and the mutation harness take a seed that rotates on
a stated cadence and is printed in every run; the deep workflow
gains a seed input. Landed at b37c9188 (PR #29). The first
rotated main run's sweeps ran at SIM_SEED 10097 and 20097, and
the deep dispatch derived seed 70002.

### Lane T4: documentation currency, with T6 folded in (C-4, C-7, C-5)

Section 14 rewritten to the real gate map including all four
workflows; the README gains the platform and streaming
statements; the macOS leg's standing remedy is recorded, or a
workflow-level retry replaces it. Landed at 5b7481ca (PR #30),
the remedy recorded rather than automated.

### Lane T5: checkpoint story (C-6)

Design first: a short decision record on the snapshot and
fresh-log loop, then the minimal library and CLI support the
record argues for. Landed at 8a658624 (PR #31). The record is
ARCHITECTURE.md section 10.6; the support is the CLI checkpoint
subcommand, and the library gained no function, as the record
argues.

Programme 3 is complete: findings C-1 to C-7 are closed, and
DC-4 stands decided and exercised in the direction calls below.

### Direction calls for programme 3

- DC-4, the 0.2.0 publish. DECIDED by David on 2026-11-17, in
  the main chat relay, "release 0.2.0", and exercised the same
  day. All three crates were packaged by the hosted publish
  workflow on main 403cbebd (runs 44192321984, 44192851709,
  44193284451), uploaded in dependency order with zero warnings,
  and confirmed live on the registry API and the sparse index
  with the dependency lines resolved at 0.2.0. One repair rode
  along: the publish workflow's artifact path named the 0.1.0
  file literally and failed the first 0.2.0 packaging run at the
  upload step; PR #27 made the path a version glob. The
  verify-published workflow's pins move to 0.2.0 in the release
  record commit, and tag v0.2.0 marks the record head, as v0.1.0
  did.

## Programme 4: the D set (opened 2026-11-19)

A fresh end-to-end audit at fbee5f82 found the streaming rebuild
complete and the surfaces around it uneven. Findings D-1 to D-5,
lanes U1 to U5 in order, approved by the orchestrator on
2026-11-19. No direction call arose: nothing published, no format
changed, no public API moved.

- D-1: the mutation harness still targeted the slice decoders
  while production read paths are the streaming surfaces (U1).
- D-2: the performance record predated the streaming rebuild
  (U2).
- D-3: the published-crate verification ran only on dispatch
  (U3).
- D-4: CONTRIBUTING.md described the retired landing route (U4).
- D-5: the checkpoint loop had no worked example (U5).

### Lane U1: mutation coverage of the streaming decoders (D-1)

Every seeded mutation also runs through the streaming scanner
at chunk sizes 1, 7, and 64, through scan_reader, and through
streaming recovery, with verdicts and outcomes asserted equal
to the slice forms. Landed at 8109436a (PR #33). The harness
convicted one real divergence on its first run: for a header
corrupt in both its payload length and its reserved field, the
slice form names the length and the streaming form names the
reserved field, because the two paths judge the bytes in
different orders by construction. Verdict, variant, and
convicted offset agree in every case, and section 9.5 promises
verdicts rather than field order, so the pair runner compares
error identity (variant plus convicted offset) and the finding
is recorded in the harness and in INV-WAL-7.

### Lane U2: performance record refresh (D-2)

A full criterion pass on the current code, recorded in
docs/PERFORMANCE.md with the machine class named. Landed at
d09ad7a1 (PR #34), figures from bench dispatch 44601074213 on
fbee5f82. Append reads about twice its S3 figure and recovery
about 1.7 times; amend and mass cancel read slower than S2. No
performance code changed in the lane.

### Lane U3: weekly verify-published schedule (D-3)

A weekly cron (Mondays 06:17 UTC) joins the dispatch trigger,
so a toolchain or registry change that breaks stranger
consumption surfaces on its own. Landed at b4dcfb8c (PR #35).

### Lane U4: CONTRIBUTING landing route (D-4)

The Commits section states the route that exists and the
changelog-per-lane rule. Landed at 37b332bf (PR #36).

### Lane U5: checkpoint worked example (D-5)

The wal example gains a second act running the checkpoint
composition end to end on a copy of its session log, with the
output pinned for CI. Landed at e53de9ba (PR #37).

Programme 4 is complete: findings D-1 to D-5 are closed.

## Programme 5: the E set (opened 2026-11-22)

A fresh audit at 6e9d1615, the cache pilot's landing, found the
engine and its gates in good order and the surfaces around
them uneven again. Findings E-1 to E-6, lanes V1 to V6. One
direction call, DC-5, is raised below for David; no lane waits
on it.

- E-1: ARCHITECTURE.md section 14 never mentions the cache
  step the pilot added to ten of the fifteen CI jobs, and it
  describes verify-published as dispatch-only when lane U3
  gave it a weekly schedule (V1).
- E-2: the three compiling dispatch workflows (bench,
  mutation-deep, publish) still build from cold, the pilot
  note's flagged rollout follow-on (V2).
- E-3: the selling layer carries no media at all; the README
  has no image, GIF, or video (V3).
- E-4: the INVARIANTS traceability (53 rows plus CHECK-DOC-1)
  has never been re-verified end to end since the streaming
  refactors moved and renamed tests (V4).
- E-5: the cache pilot's median follow-up is owed, three
  warm runs per event type before the fleet-template verdict
  hardens (V5).
- E-6: no crate carries a README, so the crates.io pages
  render no readme for any of the three published crates
  (V6).

### Lane V1: section 14 currency (E-1)

Section 14 describes the cache step and the weekly
verify-published run, and this programme section lands in the
same commit, so the design record lives in the repo.

### Lane V2: cache rollout to the dispatch workflows (E-2)

bench.yml, mutation-deep.yml, and publish.yml gain the cache
step in the pilot's terms, with cold and warm dispatch
measurements recorded.

### Lane V6: crate READMEs (E-6)

Each crate gains a README stating what the crate is, what it
does, and how it is used, and the package lists are
re-verified. The lane runs early so any future release ships
them.

### Lane V4: INVARIANTS traceability audit (E-4)

Every row's enforcement cell is cross-checked against the
tree, that the named tests and benchmarks exist and run, and
repairs land in the same lane.

Outcome (audited 2026-11-24 at 70b0cc01): the matrix is
sound, and no repairs were required. All 54 rows (53
invariants plus CHECK-DOC-1) were read against the tree, and
120 enforcement names were extracted from the cells and
judged one by one. Every named property test, unit test,
integration test, fault oracle, benchmark, and file resolves
to the tree as it stands; all 17 property tests and all 8
benchmark targets in the tree are cited by a row. Five names
needed a verdict by eye and each referent exists: the crossed
book check inside debug_assert_invariants (annotated with its
row id in the source), the level count column the bitmap
checker recomputes from, the frame header length field in the
decoder, the determinism gate's forbidden token list in
ci.yml, and the best_bid_ask benchmark function inside its
bench target. The one identifier with no row, INV-LAT-5, is
the documented removal recorded at the head of the
documentation checklist: it carries no enforcement, so under
this file's own rule it is CHECK-DOC-1, not an invariant.

### Lane V3: selling-layer media (E-3)

A scripted terminal demo of the worked example, with verify,
replay, and checkpoint run against the session files,
recorded as a high-quality GIF and placed in the README.
Demo material only, under the Stageshow boundary.

Outcome: the demo as landed is the wal_recovery example's
own story, recovery and then checkpoint, told in one
terminal session. The verify, replay, and checkpoint beats
of the original sketch are the CLI's surface, and the
example already performs the checkpoint composition itself,
so the demo follows the example and stays inside one real
program's output. Every line shown is that output, pinned
byte for byte in wal_recovery.expected.txt and diffed in CI
on every change. The animation (docs/media/demo.gif, 880 by
452, about thirteen seconds, with demo-poster.png as the
still frame) types the command, plays the two acts, and
holds on the second match line.

### Lane V5: cache medians record (E-5)

The warm PR and main runs that normal traffic accrues are
collected, and the medians are recorded with the
fleet-template verdict stated from them. The lane closes
last, when three warm runs exist per event type.

Outcome: six warm runs per event type had accrued by the
V3 landing (the pilot's warm re-run and PRs 40 to 44; the
six main runs of the same landings). Medians below are per
job, started to completed, with the pre-pilot baseline
beside them (the single PR run 44751716892 and main run
44752806343 from the pilot's design note; wall medians are
over each era's successful runs).

Pull requests, wall median 198s against a 208s baseline
median. Test on macOS 176s against 245s. Bench compile 34s
against 54s. Test on Ubuntu 128s against 131s. Test on
Windows 186s against 124s, the one leg worse at the median,
with values spread 150s to 194s across the six runs.
Examples on macOS 46s against 57s, on Windows 52s against
47s, on Ubuntu 21s against 27s. Clippy 20s against 26s.
Package 20s against 17s and rustdoc 20s against 16s, both
flat.

Main runs, wall median 723s against a 735s baseline
median, inside the baseline range. First seed sweep 720s
against 538s and second seed sweep 532s against 420s.
Test on macOS 185s against 178s, on Ubuntu 133s against
176s, on Windows 198s against 181s. Bench compile 38s
against 45s. Examples on macOS 38s against 49s, on Windows
55s against 104s, on Ubuntu 18s against 26s. Clippy 25s
against 26s, package 17s against 19s, rustdoc 25s against
26s.

The hit evidence is tied to the same era's logs. The
newest PR run's macOS test and bench compile jobs and the
newest main run's first sweep all log a primary-key cache
hit on their family keys. The sweep is the telling case:
it restores its cache and still runs 726s in that run,
because its time is 10,000-case execution, which no cache
can shorten. That is the design note's prediction,
confirmed rather than assumed.

Verdict for the fleet template: keep the step. It earns
its place on the compile-heavy PR legs (the macOS test leg
and the bench compile check are the clear gains), it is
neutral on the small jobs, and it cannot move execution-
bound work, which is what sets the main wall clock. One
confound stands from the pilot's first comparison: the
toolchain action's bundled cache was already active in
the baseline era, so these deltas are the explicit step's
marginal effect on top of it, entangled with runner
variance; the clean claims are the hit lines and the
sweep case above.

### Direction calls for programme 5

- DC-5, publish 0.3.0. The Unreleased changelog holds eight
  entries spanning programmes 3 and 4 and the cache pilot.
  Raised for David; nothing in this programme waits on it,
  and lane V6 runs early so a release, if called, ships the
  crate READMEs.

Programme 5 is complete: V1 landed at 0a3ec1f1 (PR #40),
V2 at d9d1b90d (PR #41), V6 at 70b0cc01 (PR #42), the V4
audit at 1a2cb443 (PR #43), V3 at a881ccbf (PR #44), and
the V5 record in this section. Every E finding is closed.
DC-5 remains open as David's call, and the Unreleased
changelog now holds the programme 3, 4, and 5 entries and
the cache pilot's.

## Programme 6: the F set (opened 2026-11-24)

A fresh audit at f0506095, the programme 5 closure, found
the record and the exercise schedule uneven in three
places. Findings F-1 to F-3, lanes W1 to W3 in order,
then the closure record.

- F-1: ARCHITECTURE.md section 14 describes the cache
  step for CI's ten compiling jobs, but its paragraph on
  the four dispatch workflows never mentions the V2
  rollout that added the same step to three of them.
- F-2: the deep mutation exercise has never run at a
  main head containing the streaming pair tests U1 added.
  The last main-head deep run predates U1 (run
  44196756726, 2026-11-17, at b37c9188); the 2026-11-23
  deep dispatches ran on the V2 branch head.
- F-3: the repo carries no SECURITY.md, and GitHub
  private vulnerability reporting is disabled, so the
  decoder's hardening story has no stated reporting
  route beside it.

Checked and cleared in the same audit: PERFORMANCE.md
currency (no hot-path code has changed since the U2
pass), the INVARIANTS matrix (sound at the V4 audit,
nothing structural since), the verify-published weekly
schedule (fired 2026-11-23, run 45233341399, success),
the TODO and FIXME scan (clean), and the repo metadata
(description and topics set).

### Lane W1: section 14 currency (F-1)

Section 14's dispatch-workflow paragraph names the V2
cache rollout and the verify-published exception. This
section opens in the same commit.

### Lane W2: deep mutation at current main (F-2)

A mutation-deep dispatch at the current main head under
a fresh rotating seed, with the result recorded here. A
repair lane follows only if the exercise convicts
something.

Outcome: dispatch run 45534405839 at 5e04b7e1 completed
success, seed in force 70005. All eight mutation tests
passed at the deep volume of 100,000 cases per surface,
the four streaming pair tests among them. No panic and
no conviction, so no repair lane follows.

### Lane W3: the security policy (F-3)

SECURITY.md states the supported versions and the
reporting route, and private vulnerability reporting is
enabled on the repo so the route exists.

Outcome: private vulnerability reporting was enabled on
the repo first (the API confirms it enabled), and
SECURITY.md then names that route. The policy states
that fixes apply to the newest release and to main, and
it scopes the decoder surface honestly: a panic or an
accepted malformed record in either decoder counts as a
security issue, and a pure performance problem does not,
unless untrusted input can use it to halt a host.

Programme 6 is complete: W1 landed at 5e04b7e1 (PR #46),
the W2 exercise ran clean at that head (run 45534405839)
and its record landed at 9dca673c (PR #47), and W3 landed
at fa5ffbee (PR #48). Every F finding is closed. DC-5
remains open as David's call.

## Programme 7: the G set (opened 2026-11-24)

A fresh audit at 92e19f9b, the programme 6 closure,
covered the maintenance and community surfaces no
earlier set had examined end to end. Findings G-1 to
G-3, lanes X1 to X3 in order, then the closure record.

- G-1: no dependabot configuration exists. Security
  updates are enabled and both alert surfaces are empty,
  but routine version updates for the cargo dependencies
  and the pinned actions have no automation.
- G-2: no GitHub Release objects exist for v0.1.0 or
  v0.2.0. The tags stand alone, though the CHANGELOG
  holds a full section for each release.
- G-3: no issue or pull request templates exist, though
  CONTRIBUTING.md requires a review-readiness block in
  every PR body and the engine's determinism makes a bug
  exactly reproducible from a seed and a command stream.

Checked and cleared in the same audit: secret scanning
and push protection (both enabled), private
vulnerability reporting (enabled at W3), and both alert
surfaces (empty).

### Lane X1: dependabot configuration (G-1)

.github/dependabot.yml covers the cargo and
github-actions ecosystems, weekly, with minor and patch
updates grouped and major updates left individual for
deliberate review. This section opens in the same
commit.

### Lane X2: releases for the two tags (G-2)

GitHub Releases are created for v0.1.0 and v0.2.0 with
notes drawn from their CHANGELOG sections, and the
outcome is recorded here.

Outcome: both releases were created on 2026-11-24
through the API, each carrying its CHANGELOG section's
opening and highlights with a link back to the full
section. The releases page now shows 0.2.0 and 0.1.0
against their tags. A side effect of lane X1 worth
recording here: within minutes of the dependabot
configuration landing, dependabot opened its first six
pull requests, the grouped cargo minor-and-patch PR and
five individual major bumps, each of which goes through
the same hosted gate as any other change.

### Lane X3: issue and PR templates (G-3)

A pull request template carries the review-readiness
block CONTRIBUTING.md requires, and a bug-report issue
template asks for the seed and the command stream a
reproduction needs.

Outcome: .github/pull_request_template.md carries the
summary and review-readiness block (spec reference,
verified commands and head SHA, known deviations), and
.github/ISSUE_TEMPLATE/bug_report.md asks what happened,
what was expected, and for the seed, command stream or
WAL file, crate and version, and full error output a
deterministic reproduction needs, plus the environment.

Programme 7 is complete: X1 landed at f6e6505f
(PR #50), the X2 releases were created on 2026-11-24
and their record landed at baacc715 (PR #57), and X3
landed at e0e0af81 (PR #58). The dependabot queue the
X1 configuration produced was worked on its own diffs
and CI: PR #51 (clap 4.6.7 to 4.6.9, grouped) landed at
71aa101e, PR #52 (setup-rust-toolchain 1 to 2) at
75340db6, PR #56 (checkout 4 to 7) at f04e24cd, and
PR #55 (upload-artifact 4 to 7) at 6d9df39b. Two PRs
stand open with their reasons stated on them: PR #53
(actions/cache 4.3.0 to 6.1.0) is held until a
deliberate cache re-baseline re-measures under the new
backend, and PR #54 (criterion 0.5.1 to 0.8.2) is held
because its own CI fails on the bench compile, which
makes it a migration lane rather than a bump. Every G
finding is closed. DC-5 remains open as David's call.

## Programme 8: the H set (opened 2026-11-25)

A fresh audit at 9fe071c3 worked the two held
dependabot PRs down to their root causes and re-checked
the surfaces the programme 7 merges could have touched.
Findings H-1 and H-2, lanes Y1 and Y2 in order, then
the closure record.

- H-1: the criterion 0.8.2 bump (PR #54) fails for one
  reason, proven from its bench-compile job log:
  criterion 0.8 deprecates criterion::black_box in
  favour of std::hint::black_box, every bench target
  imports the deprecated path, and the workspace denies
  warnings. Nothing else breaks; the bump's test legs
  pass on the 0.8.2 lock.
- H-2: the actions/cache v6 bump (PR #53) still owes
  its deliberate re-baseline: warm medians measured the
  programme 5 way on the current v4 line, then the same
  measurement on v6, with the verdict recorded.

Checked and cleared in the same audit: ARCHITECTURE
section 14 names no action versions, so the programme 7
merges left it accurate; the toolchain pin stands
current (rustc 1.100.0); both alert surfaces remain
empty; PERFORMANCE.md and INVARIANTS are untouched by
the lock-only and workflow-only merges, correctly.

### Lane Y1: the criterion 0.8 migration (H-1)

The migration is built on PR #54's own head, so its
verified Cargo.lock carries the 0.8.2 bump, and ports
the seven bench targets that use black_box from
criterion::black_box to std::hint::black_box.
bench_amend uses no black_box and is untouched. PR #54
closes as superseded when this lane lands. This
section opens in the same commit.

### Lane Y2: the cache v6 re-baseline (H-2)

Warm medians are measured on the current v4 line and
again on the v6 line the same way, the comparison and
the keep-or-revert verdict are recorded, and PR #53 is
dispositioned on that evidence.

Y1 outcome: the migration landed at 5202b926
(PR #61, on the fresh branch y1-criterion-0-8-fresh;
PR #60, stacked on PR #54's history, computed phantom
conflicts in these record files from its stale merge
base and closed with a pointer once the identical tree
was re-committed onto main). The seven bench targets
that use black_box import it from std::hint, placed
after the criterion import where this repo's rustfmt
ordering puts it; PR #54 is closed as superseded. The
merged Cargo.lock carries criterion 0.8.2 and clap
4.6.9 together, proven by a package-block comparison
of the two locks before landing.

Y2 outcome: the re-baseline ran on real traffic, not
synthetic dispatches. The v4 sample, from the night's
runs before the bump: PR wall median 230s (n=3, spread
176 to 237), main wall median 712s (n=4, spread 638 to
733), bench compile legs 29 to 62s, and macOS test legs
bimodal between about 50s (full restore hits) and
about 220s. PR #53 then landed at 817b61df. The v6
sample: PR walls 316s and 508s (n=2; the 508s run was
the first on the new backend's empty cache), main wall
740s (n=1), bench compile legs 27 to 43s, macOS test
legs 42s, 52s, and 181s. Every v6 figure sits inside
the v4 spread once the cache warmed, and the warm-hit
signatures match v4's. Verdict: keep actions/cache v6,
on a small sample with runner variance stated plainly;
the fleet template's per-family expectations carry
over unchanged.

Programme 8 is complete: Y1 landed at 5202b926
(PR #61) and Y2 at 817b61df (PR #53), with this
closure record as Y3. Every H finding is closed.
DC-5 remains open as David's call.

## Programme 9: the I set (opened 2026-11-25)

A fresh audit at bcc3f1c5 found the surfaces
programmes 3 to 8 closed still closed: ARCHITECTURE
section 14's cache wording names no action version
and stays accurate under v6, both alert surfaces are
empty, the demo still matches the example output it
pins, and the deep mutation exercise stands current
from W2. One item stood out: every figure in
docs/PERFORMANCE.md was measured under criterion
0.5, and programme 8 moved the harness to 0.8, so
the performance record was one harness generation
stale. Lane Z1 refreshes it, then the closure
record.

### Lane Z1: performance pass under criterion 0.8

A full bench dispatch at the current main, with the
new pass recorded in docs/PERFORMANCE.md as a dated
section naming the harness change and claiming no
performance change, since no measured code changed.

Outcome: bench dispatch run 45601655307 at main
bcc3f1c5 completed success, and the pass is recorded
in docs/PERFORMANCE.md (the Programme 9 pass of
2026-11-25). Add 40.58 ns, cancel 24.11 ns,
best_bid_ask 11.73 ns, amend 24.53 ns, mass cancel
73.69 ns per order, sweep 27.33 microseconds, depth
915.02 ns, wal_append 41.34 ns per frame at
1,661.25 MiB/s, recovery 422.01 microseconds at
813.54 MiB/s, snapshot round trip 1.22 ms at
741.78 MiB/s. Movement against the U2 pass runs both
ways, as between every pair of passes on shared
runners; no performance change is claimed.

Programme 9 is complete: Z1 landed at a84c491c
(PR #63), with the criterion 0.8 performance pass
recorded in docs/PERFORMANCE.md from bench dispatch
run 45601655307, and this closure record is Z2.
The I set held one item and it is closed. DC-5
remains open as David's call.

## Programme 10: the J set (opened 2026-11-25)

The fresh audit at 1ebc617a found one material item,
and it is a correction to this record. PR #56
(checkout v7) and PR #55 (upload-artifact v7) landed
in programme 7 at f04e24cd and 6d9df39b with the
correct patches, verified in their merge commits.
Lane Y1 of programme 8 then silently reverted both:
its tree was built on PR #54's branch, cut from main
at 71aa101e before those merges, and the squash at
5202b926 carried the older workflow files back over
the newer ones. The seat's verification compared the
merged tree to the branch tree, which proves a lane
landed what it built, but never diffed the merge
against its parent for files the lane did not intend
to touch. That parent-diff check is now part of this
repo's landing routine for any branch not cut from
current main. The Y2 cache verdict is unaffected:
both samples ran with checkout v4 in force, so the
cache step was the only workflow difference between
them. Everything else in the audit came back clean:
no TODO or FIXME markers in the crate sources, crate
metadata complete on all three packages, and the
surfaces closed in programmes 3 to 9 still closed.

### Lane AA1: restore the reverted action pins

Restore exactly the #55 and #56 changes on current
main: actions/checkout v7 in ci.yml, bench.yml,
mutation-deep.yml, and publish.yml, and
actions/upload-artifact v7 in bench.yml and
publish.yml. Nothing else changes.

AA1 outcome: the pins are restored at 502404d8
(PR #65), verified live on main after the merge:
checkout v7 in ci.yml, bench.yml, mutation-deep.yml,
and publish.yml; upload-artifact v7 in bench.yml and
publish.yml; the cache v6 SHA pin and the toolchain
v2 pin untouched. The merge's parent diff was read
file by file before the landing was called done, the
check whose absence let the revert through in Y1.

Programme 10 is complete: AA1 landed at 502404d8
(PR #65), and this closure record is AA2. The J set
held one item, a correction, and it is closed.
DC-5 remains open as David's call.

## Programme 11: the K set (opened 2026-11-25)

Programme 10's lesson set this audit's first pass:
if one stale-base tree slipped a revert past
tree-identity verification, the same check was owed
to every other merge of the era. The parent diff of
every squash merge from programme 7's X1 through
programme 10's AA2 was read, file list against the
lane's stated intent. Every merge carries exactly
its intended files, with the one exception already
repaired: Y1's seventeen files are its intended
thirteen plus the four workflow files, the clobber
surface and nothing more, so AA1's restore covers
the incident's full extent. The merges of
programmes 1 to 6 were built on branches cut from
the then-current main, and their compare diffs were
verified at landing per their records; the
stale-base condition was unique to Y1's stack on
PR #54's branch.

Standing practice, recorded here so it outlives the
seat: a landing is verified by three checks, not
one. The merged tree must equal the green branch
tree; the merge's parent diff must name only the
files the lane intended to touch; and any content
the landing claims to carry must be read back live
on main after the merge.

### Lane AB1: record the merge-integrity sweep

This section is the lane: the sweep, its outcome,
and the standing practice above. The changelog
entry rides along. AB2 closes the programme.

AB1 outcome: the sweep record and the standing
practice landed at 644f3d55 (PR #67), its merge
verified by the three checks the record itself
names: tree identity, a parent diff of exactly the
two record files, and the landed text read back on
main.

Programme 11 is complete: AB1 landed at 644f3d55
(PR #67), and this closure record is AB2. The K set
held one lane, the sweep record, and it is closed.
DC-5 remains open as David's call.

## Programme 12: the L set (opened 2026-11-25)

The first pass audited documentation links and GitHub repository
settings. Merge settings were tightened to squash-only merges (`PR #69`,
`aa545dc8`) with the unused wiki disabled and `delete_branch_on_merge`
left off. Closure recorded in `ee9bf24f` (`PR #70`).

## Programme 13: the M set (opened 2026-11-25)

Dispatch run `45650240040` at `ee9bf24f` exercised all eight mutation
tests at `100,000` cases under seed `70006` (`PR #71`, `1381b5c6`), and
`80a80e47` (`PR #72`) closed the programme while noting that `DC-5`
(`0.3.0` publication) resolves the missing per-crate READMEs on
crates.io.

## Programme 14: production launch perfection and 0.3.0 preparation (2026-11-25)

Closes `DC-5` preparation and hardens every crate, CLI command,
benchmark, and workflow for the `0.3.0` production release. Unconditional
header and structural snapshot validation now runs in both debug and
release builds alongside CRC-recomputing structural mutation suites,
`SIM_SEED` drives per-stream ChaCha property runners across all eighteen
property suites, `scan_reader_with` streams `verify` and buffered
`replay` in bounded memory, `checkpoint` and `snapshot` accept
`--snapshot` bases across multi-cycle checkpoints, `Bitmap` neighbour
queries run in `O(bitmap depth)`, zero-allocation `Book` query helpers
and `OrdersAt` L3 iterators join `chronicle-core`, Criterion teardown
windows and `save_snapshot_with_buffer` are aligned, doctested crate
READMEs and `.gitignore` are in place, and `.github/workflows/release.yml`
builds precompiled five-target `chronicle-cli` archives with `SHA256SUMS`.

## Programme 15: the N set (opened 2026-11-25)

This seat's dependency health pass, renumbered on
arrival: the launch programme landed as programme
14 (PR #73) while the pass was in flight, and this
seat's first record of it (PR #74) was closed
unmerged against the moved main, correctly. The
pass itself was re-verified against the new lock
before recording: 120 packages, only the three
workspace entries changed by the launch bump.

### Lane AF1: record the dependency health pass

The full Cargo.lock read from the remote tree,
every external locked version checked against the
crates.io API for yanked status, and the lock
scanned for duplicate versions of one crate. The
outcome: 117 external versions checked, zero
yanked, zero failed lookups. One duplicate stands:
syn at 2.0.119 and 3.0.10, which is the dependency
graph's own shape (different consumers requiring
different majors of a compile-time library), not a
defect this repo can or should act on; it is
recorded as observed and explained. This section
is the lane; AF2 closes the programme.

AF1 outcome: the dependency health record landed
at fe819dba (PR #75). The pass was re-verified
against the post-launch lock before recording:
120 packages, only the three workspace entries
changed by the 0.3.0 bump, 117 external versions
checked against the registry, none yanked, one
duplicate (syn) recorded as the graph's own shape.

Programme 15 is complete: AF1 landed at fe819dba
(PR #75), and this closure record is AF2. The N
set held one lane and it is closed.

## Programme 16: the O set (opened 2026-11-25)

The post-launch audit. The 0.3.0 launch (PR #73)
changed hot paths and several standing surfaces,
so this programme re-verifies what it touched.
First-pass checks, all against the current tree:
the README demo still matches the wal_recovery
example's pinned output byte for byte after the
launch's example edits; the INVARIANTS spot pass
resolves, the only absent names being the
prohibited constructs the determinism rows forbid;
and no v0.3.0 tag or GitHub Release exists yet,
which belongs to the publication step and is
recorded here as an observation only.

### Lane AG1: record the post-launch performance pass

Bench dispatch run 45707524841 at 43415ab2
completed success. The figures stand in
docs/PERFORMANCE.md as the programme 16 pass,
recorded as measured under the post-launch
harness with the comparability caveat stated
there: the launch reworked several measurement
windows, so no performance verdict is claimed
against the programme 9 pass. The pass also
records the launch's new recovery-from-reader
benchmark beside the slice form. AG2 closes the
programme.

AG1 outcome: the post-launch performance record
landed at 171cf5c2 (PR #77) from bench dispatch
45707524841, figures recorded as measured under
the post-launch harness with the comparability
caveat stated in PERFORMANCE.md.

Programme 16 is complete: AG1 landed at 171cf5c2
(PR #77), and this closure record is AG2. The O
set's checks stand as recorded at the programme's
head: the demo verified byte-accurate against the
example, the INVARIANTS spot pass consistent, and
the v0.3.0 tag and release absence noted for the
launch ledger, where it belongs to the
publication step.

## Programme 17: the P set (opened 2026-11-25)

The deep mutation exercise against the post-launch
decoders. The launch hardened snapshot decoding
and extended the mutation suites, so the surface
the exercise stresses changed materially after
the programme 13 run; this programme puts the new
decoders under the deep volume.

### Lane AH1: exercise and record the deep mutation pass

Dispatch run 45713933476 at bf85e526 completed
success. Verified from the job log: the seed in
force was 70007, the volume 100,000 cases per
surface, and the mutation suite now runs fifteen
tests, the launch's CRC-recomputing structural
suites among them (the valid-CRC snapshot and WAL
forms beside the single-point, multi-point, and
streaming pair forms). All fifteen passed. No
panic and no conviction, so no repair lane
follows. AH2 closes the programme.

AH1 outcome: the exercise record landed at
f8a734f0 (PR #80). One repair rides with this
closure, stated plainly: PR #79 (the launch
README and media polish) landed at 70e8ab2b while
AH1 was in flight, and AH1's tree, built on the
earlier main, carried the older CHANGELOG region
over PR #79's added entry in the 0.3.0 section
when the squash applied. The parent-diff check
caught it in the same pass (the merge showed
CHANGELOG +4/-4 where the lane intended +4/-0).
Nothing else from PR #79 was touched: its files
stand intact, and the lost four lines are
restored verbatim by this closure in their
original position. The added discipline: when
main has moved since a lane's tree was built, the
parent diff is read against the new parent before
the landing is reported, not after.

Programme 17 is complete: AH1 landed at f8a734f0
(PR #80), and this closure record is AH2. The P
set held one lane and it is closed.

## Programme 18: the Q set (opened 2026-11-26)

The release-state audit: the launch's release
apparatus verified from the outside, read-only,
while the launch drive works it.

### Lane AI1: record the v0.3.0 release state

Verified firsthand on 2026-11-26: the GitHub
Release v0.3.0 is published (not a draft, not a
prerelease) and carries five binary tarballs
(macOS and Linux on arm64 and x86_64, Windows on
x86_64) plus SHA256SUMS. The annotated tag v0.3.0
points at 0b8c1cca. The release workflow's run at
that head completed success, after an earlier
failure at 2b31951c that PR #83 addressed. Two
registry gaps stand as observations for the
launch ledger, not actioned from this seat:
crates.io still lists only 0.1.0 and 0.2.0 for
the three crates, and PyPI carries no package
under the chronicle-lob names yet, though PR #82
landed the Python SDK and the OIDC release
automation. AI2 closes the programme.

AI1 outcome: the release-state record landed at
6c4b5a2a (PR #84), its parent exactly the base it
was built on and its diff additions only. The
verified state stands as recorded there: GitHub
Release v0.3.0 published with five binary
tarballs and SHA256SUMS, tag at 0b8c1cca, release
run green at that head, and the crates.io and
PyPI publications outstanding as launch-ledger
observations.

Programme 18 is complete: AI1 landed at 6c4b5a2a
(PR #84), and this closure record is AI2. The Q
set held one lane and it is closed.

## Programme 19: the R set (opened 2026-11-26)

The shipped-artifact verification: the binary
channel the launch opened gets the stranger
consumption check the crates channel has always
had, and the publication state is re-verified
across every channel.

### Lane AJ1: verify the shipped binary and the publication state

The v0.3.0 x86_64 Linux tarball was downloaded
from the GitHub Release at exactly its listed
size (512,615 bytes) and the shipped chronicle-cli
binary was run in a clean sandbox: bench at
10,000 operations under seed 1 printed the eight
contract lines in order (applied 7216, rejected
2784, trades 3818, live_orders 41, best_bid 302,
best_ask 322) and the matching JSON object, exit
0 in both modes. The publication state is now
complete on every channel, verified firsthand:
crates.io lists 0.3.0 for all three crates,
docs.rs serves both library crates' 0.3.0 pages,
and PyPI carries chronicle-lob at 0.3.0. The
registry observations left outstanding in
programme 18 are discharged by the launch drive
itself. AJ2 moves the verify-published pins to
0.3.0 now that the registry carries it; AJ3
closes the programme.

### Lane AJ2: prove the published 0.3.0 crates

The verify-published pins already read 0.3.0: the
launch drive re-pinned the workflow with the
release, so this lane is the proof rather than
the pin move. Dispatch run 45776133389 at
892a4c0f completed success on both jobs: a fresh
scratch project adds chronicle-core and
chronicle-wal at 0.3.0 from the real registry,
builds, and runs the round trip, and cargo
install of chronicle-lob-cli at 0.3.0 succeeds
with the installed binary's bench running. The
crates channel's stranger-consumption proof
stands at 0.3.0 beside AJ1's binary-channel
proof. AJ3 closes the programme.

AJ2 outcome: the verify-published proof record
landed at 0e3c1904 (PR #87), from dispatch
45776133389 completed success on both jobs.

Programme 19 is complete: AJ1 landed at 892a4c0f
(PR #86), AJ2 at 0e3c1904 (PR #87), and this
closure record is AJ3. The R set's two proofs
stand: the shipped binary runs to contract from
the release tarball, and the published 0.3.0
crates work from a stranger's scratch project and
a clean cargo install. Publication stands
complete on every channel.
