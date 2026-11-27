# Runnable Examples

Self-contained walkthroughs demonstrating `chronicle-lob` across Rust, the `chronicle_lob` Python SDK, and the `chronicle-cli` operator binary. Continuous integration runs every example on every pull request and push to `main`.

## Top-level SDK and operator drills (`examples/`)

Build the `chronicle-cli` binary once first so the Python SDK and shell drill can invoke it from `target/debug/chronicle-cli`:

```sh
cargo build --locked -p chronicle-lob-cli --bin chronicle-cli
```

| Script | Surface | What it demonstrates |
| --- | --- | --- |
| [`market_session_and_replay.py`](market_session_and_replay.py) | Python SDK (`chronicle_lob`) + `chronicle-cli` | Encodes a multi-participant session (`Limit`, `PostOnly`, `Ioc`, `Amend`, `Cancel`, and `CancelByParticipant`) with `WalBuilder`, verifies `CWAL` v1 frames via `read_wal_frames`, writes an atomic `CSNP` v1 snapshot, validates `read_snapshot_header`, replays deterministic execution events across a snapshot seam, and asserts exact book invariants. |
| [`crash_recovery_drill.sh`](crash_recovery_drill.sh) | `chronicle-cli` (`sh` + `mktemp -d`) | Simulates an interrupted `CWAL` v1 trailing frame write, verifies clean crash-boundary truncation (`discarded=19`, exit `0`) versus mid-log corruption detection (exit `1`), executes an atomic `checkpoint` with log archiving, replays continuation frames across the checkpoint seam, and runs `bench --json`. |

Run both top-level drills directly:

```sh
python3 examples/market_session_and_replay.py
sh examples/crash_recovery_drill.sh
```

## Crate-level Rust examples (`crates/*/examples/`)

The Rust workspace crates also include standalone examples whose `stdout` is diff-checked against pinned `.expected.txt` files in CI:

```sh
# Order types (Limit, IOC, FOK, PostOnly), priority-aware amend, cancel,
# atomic rejections, and all three self-trade prevention policies:
cargo run --locked -p chronicle-core --example book_session

# Live book + WAL append, mid-session snapshot, streaming recovery,
# verified checkpoint archive, and recovery across the checkpoint seam:
cargo run --locked -p chronicle-wal --example wal_recovery
```
