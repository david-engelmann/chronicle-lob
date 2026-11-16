# Post-M4 roadmap

Status: the four-milestone build (M0 through M4) is complete and fully
gated at main head 387200b1. Hosted CI is green on all eight jobs
including the 10,000-case seed sweep, 112 tests pass, and the M4 gate
re-audit closed the INVARIANTS enforcement census at 65 of 65.

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
  scope covers the wal entry points.

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
- DC-2, the CLI package name. Decided in-lane under the standing
  delegation, because the old name is taken and the fix is
  mechanical: the package becomes chronicle-lob-cli and the binary
  stays chronicle-cli. The decision is reversible at any point
  before DC-1 is exercised.
- DC-3, none open besides DC-1. Any future call the lanes surface
  gets recorded here in the same commit that raises it, rather than
  stopping the lane that found it.
