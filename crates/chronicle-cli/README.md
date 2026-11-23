# chronicle-lob-cli

The command line tool for chronicle order books. It installs one binary, `chronicle-cli`, with five subcommands for working with write-ahead logs and snapshots.

The package is named chronicle-lob-cli because chronicle-cli was already taken on crates.io. The installed binary keeps the name `chronicle-cli`, so usage does not change.

## What it does

- `replay` applies a write-ahead log to a book and prints the event stream.
- `verify` reports the sequence and checksum health of a log and snapshot pair.
- `snapshot` writes a snapshot from a write-ahead log.
- `checkpoint` writes a snapshot, verifies it by loading it back, and only then archives the log the snapshot covers.
- `bench` runs a seeded synthetic load against a book.

This is the only crate in the workspace that parses text or prints JSON. Output ordering is fully determined by the input files and the seed, and every subcommand accepts `--seed`, which honours the `SIM_SEED` environment variable.

## How it is used

Install the package and run the binary against session files.

```sh
cargo install chronicle-lob-cli
chronicle-cli verify --wal session.wal --snapshot session.snap
chronicle-cli replay --wal session.wal
chronicle-cli checkpoint --wal session.wal --out session.snap
```

Worked examples with pinned output live in the `examples` directories of the chronicle-core and chronicle-wal crates in the repository, and CI runs them on every change.

## Where it fits

chronicle-lob-cli is the operator surface of the chronicle-lob workspace. It drives books built with `chronicle-core` and persisted with `chronicle-wal`.

## License

MIT OR Apache-2.0
