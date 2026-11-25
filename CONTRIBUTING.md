# Contributing

This is a small, deliberately strict Rust workspace, and the rules
below exist so the engine stays deterministic and provable.

## Setup

The toolchain is pinned in `rust-toolchain.toml` and dependencies
are locked in `Cargo.lock`. Every cargo command runs with
`--locked`.

## The gates

Run these before opening a pull request. Continuous integration
runs the same set.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked
cargo test --workspace --locked
cargo bench --workspace --locked -- --test
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Property tests run 1,000 cases per pull request and 10,000 on the
main branch. Set `PROPTEST_CASES` and `SIM_SEED` to change case
counts and seeds locally.

## Rules that are not negotiable

- `chronicle-core` depends on nothing outside the Rust standard
  library. New dependencies belong in the adapter crates, the
  command line tool, benchmarks, or dev-dependencies.
- No unsafe code in production targets. The one exception is the
  counting allocator inside the dedicated allocation proof test
  targets, which carries its reason in writing.
- Every invariant in `docs/INVARIANTS.md` names the exact test,
  oracle, or benchmark that enforces it. A change that adds a
  promise updates the matrix in the same commit.
- Deterministic behaviour is a feature. No wall clock time, no
  operating system randomness, and no hash map iteration order in
  the core or the recovery path.
- Work lands contract first. Tests that pin new behaviour are
  written before the implementation, and they fail on the missing
  behaviour, not on compile errors.

## Commits

Commits carry a Signed-off-by trailer with the author's identity.
Work lands through a pull request from a branch, with the hosted gate
green on the exact head, followed by a squash merge whose tree is
verified identical to the reviewed head. Every landing lane adds its
CHANGELOG.md entry in the same commit.

## Copy

Public-facing text avoids em dashes, colons, and semicolons, and
says what the software is and does in plain language.
