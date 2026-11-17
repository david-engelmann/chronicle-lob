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
