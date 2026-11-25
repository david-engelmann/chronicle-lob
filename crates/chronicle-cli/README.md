# chronicle-lob-cli

The command line tool for chronicle order books. It installs one binary, `chronicle-cli`, with five subcommands for working with write-ahead logs and snapshots.

The package is named chronicle-lob-cli because chronicle-cli was already taken on crates.io. The installed binary keeps the name `chronicle-cli`, so usage does not change.

## What it does

- `replay` applies a write-ahead log to a book in bounded memory and streams the event lines to standard output.
- `verify` reports the sequence and checksum health of a log and optional base snapshot in bounded memory.
- `snapshot` writes an atomic snapshot from a write-ahead log and optional base snapshot.
- `checkpoint` writes an atomic snapshot from a log and optional base snapshot, verifies it by loading it back, and only then archives the log the snapshot covers.
- `bench` runs a seeded synthetic load against a book.

This is the only crate in the workspace that parses text or prints JSON. Output ordering is fully determined by the input files and the seed, and every subcommand accepts `--seed`, which honours the `SIM_SEED` environment variable.

## How it is used

Install the package with Cargo or `cargo binstall` and run the binary against session files. Pass explicit book configuration flags when starting from a raw log, or pass `--snapshot` so the command reads the base book and configuration from the snapshot header.

```sh
cargo install chronicle-lob-cli
chronicle-cli snapshot --wal session.wal --out session.snap \
  --max-orders 1024 --grid-len 4096 --tick-size 1 --grid-start-tick 1
chronicle-cli verify --wal session.wal --snapshot session.snap
chronicle-cli replay --wal session.wal --snapshot session.snap
chronicle-cli checkpoint --wal session.wal --snapshot session.snap --out session.snap
chronicle-cli bench --ops 100000 --seed 42 --json
```

Worked examples with pinned output live in the `examples` directories of the chronicle-core and chronicle-wal crates in the repository, and CI runs them on every change.

## Where it fits

chronicle-lob-cli is the operator surface of the [chronicle-lob](https://github.com/david-engelmann/chronicle-lob) workspace. It drives books built with [`chronicle-core`](https://crates.io/crates/chronicle-core) and persisted with [`chronicle-wal`](https://crates.io/crates/chronicle-wal). Full design and invariant references live in [docs/ARCHITECTURE.md](https://github.com/david-engelmann/chronicle-lob/blob/main/docs/ARCHITECTURE.md) and [docs/INVARIANTS.md](https://github.com/david-engelmann/chronicle-lob/blob/main/docs/INVARIANTS.md).

## License

MIT OR Apache-2.0
