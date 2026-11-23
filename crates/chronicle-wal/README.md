# chronicle-wal

The write-ahead log, snapshots, and recovery for chronicle-core order books, written in Rust.

## What it does

chronicle-wal persists accepted commands, never events, as CRC32C framed records with a 32 byte header and fixed width payloads. The reader implements a fixed decision table, so a torn tail left by a crash is a reported boundary and anything worse is a typed corruption error at an exact offset.

Snapshots are a structural dump of the book, saved by an atomic rename protocol and validated field by field on load. Recovery rebuilds a book from a snapshot plus the log tail that follows it, and the recovered book is proven equal in state to the live run it came from.

Reads and recovery also run in streaming form. A scanner applies the frame decision table incrementally to chunks of bytes from any reader in bounded memory, so a long log never has to sit in memory at once.

The crate forbids unsafe code. Its recovery path reads no clock, uses no randomness, and iterates no hash maps, so recovery is as deterministic as the book it rebuilds.

## How it is used

Add the crate alongside chronicle-core, append accepted commands through a `WalWriter`, and rebuild state with the recovery entry points.

```toml
[dependencies]
chronicle-core = "0.2"
chronicle-wal = "0.2"
```

```rust
use chronicle_wal::{recover, scan_log};

// Scan a log to check its framing and checksums, or recover a
// book from a snapshot plus the log tail after it.
```

The log and snapshot formats are specified in `docs/ARCHITECTURE.md` sections 9 and 10 in the repository.

## Where it fits

chronicle-wal is the persistence layer of the chronicle-lob workspace. It persists books built with `chronicle-core`, and `chronicle-lob-cli` exposes its verify, replay, snapshot, and checkpoint operations from the command line.

## License

MIT OR Apache-2.0
