# Fuzz Targets for chronicle-lob

Coverage-guided `libFuzzer` harnesses exercising `chronicle-wal` and `chronicle-core` binary decoders against arbitrary hostile or corrupted byte streams (`INV-WAL-7`, `INV-SNAP-1`).

`fuzz/` is an independent Cargo workspace (`[workspace] members = ["."]`) so `libfuzzer-sys` never enters the main workspace dependency graph or `chronicle-core`'s zero-dependency check.

## Targets

| Target | Entry points exercised | Contract verified |
| --- | --- | --- |
| `wal_recover` | `chronicle_wal::recover`, `chronicle_wal::recover_from_reader`, `chronicle_wal::scan_log`, `chronicle_wal::scan_reader_with` | Never panics or hangs on arbitrary byte streams; slice and 8 KiB streaming scanners agree on frame counts, last sequence, and discarded crash-tail bytes whenever scanning succeeds. |
| `snapshot_decode` | `chronicle_wal::decode_snapshot`, `chronicle_wal::snapshot_config`, `chronicle_core::Book::from_snapshot_state` | Corrupt or crafted `CSNP` v1 byte streams return `Err` cleanly and never panic; any decoded `SnapshotState` restores into a valid `Book`. |

## Stable compile gate (CI)

Continuous integration compiles every fuzz target binary on the pinned stable toolchain with `--locked` so API changes can never silently break fuzz harnesses:

```sh
CARGO_TARGET_DIR=target cargo check --manifest-path fuzz/Cargo.toml --locked --bins
```

## Running `cargo-fuzz` locally (nightly)

Install `cargo-fuzz` and run either target seeded from `fuzz/seeds/<target>`:

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run wal_recover fuzz/seeds/wal_recover -- -max_len=16384
cargo +nightly fuzz run snapshot_decode fuzz/seeds/snapshot_decode -- -max_len=65536
```
