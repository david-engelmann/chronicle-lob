# Agent Guide for chronicle-lob

This guide is for AI coding agents and automation harnesses that embed `chronicle-core` and `chronicle-wal`, drive `chronicle-cli`, generate or inspect binary logs with the `chronicle_lob` Python SDK, or modify this repository.

## Start here

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/INVARIANTS.md](docs/INVARIANTS.md) first. Those two files define every data structure, binary wire offset, preflight rule, and named invariant (`INV-*`).

Skip `docs/ROADMAP.md` unless you are specifically auditing historical post-Milestone-4 engineering programme closure records. It is maintainer history rather than a live API specification.

| Resource | What it covers |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Tick grid math, 64-ary bitmaps, SoA order arena, open-addressed lookup, `CWAL` v1 and `CSNP` v1 binary layouts, and streaming recovery. |
| [docs/INVARIANTS.md](docs/INVARIANTS.md) | Formal invariants (`INV-ALLOC-*`, `INV-BOOK-*`, `INV-ORD-*`, `INV-WAL-*`, `INV-SNAP-*`, `INV-DET-1`) and the exact tests enforcing each row. |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | Recorded Criterion latency and throughput baselines with runner and commit provenance (`CHECK-DOC-1`). |
| [docs/COMPARISON.md](docs/COMPARISON.md) | Architectural trade-offs against heap-allocated books, ring-buffer engines, and floating-point backtesters, plus explicit non-goals. |
| [docs/FAQ.md](docs/FAQ.md) | Common operational, wire-format, and integration questions. |
| [examples/README.md](examples/README.md) | Copy-paste runnable Python SDK and CLI crash-recovery drills, plus crate-level Rust examples. |
| [Published book](https://david-engelmann.github.io/chronicle-lob/) | Rendered mdBook documentation site with Mermaid diagrams. |

## Embedding `chronicle-core` and `chronicle-wal` (Rust)

- **Integer grid configuration.** Construct a `Book` once with `Book::new(BookConfig { max_orders, grid_len, tick_size, grid_start_tick, default_stp })`. `grid_len` must be a multiple of `64` in `64..=4_194_304` (`2^22`), `tick_size >= 1`, and `max_orders >= 1`.
- **Identifiers and quantities.** `OrderId(0)` is reserved and rejected with `BookError::InvalidOrderId`. `ParticipantId(0)` is valid. Quantities and prices are `u64` quote/lot integers; notional checks use checked `u128` arithmetic (`INV-MATH-1`).
- **Zero-allocation command loop.** Allocate `Vec<ExecutionEvent>` once at startup, wrap it in `EventBuffer::new(&mut storage)` per command, read `storage[..written]`, and clear `storage` before the next call. Every rejected command fails during preflight before mutating book state or `seq` (`INV-ORD-4`, `INV-EVENT-1`).
- **Persistence and bounded-memory recovery.** Persist accepted `WalCommand` records (never derived `ExecutionEvent` records) via `chronicle_wal::WalWriter`. Use `scan_reader_with` and `recover_from_reader` to verify or rebuild multi-gigabyte logs in ~8 KiB of streaming buffer space. Reuse snapshot encode buffers with `save_snapshot_with_buffer` (`INV-ALLOC-3`).

## Driving `chronicle-cli` from scripts and tool calls

`chronicle-cli` (installed from crates.io package `chronicle-lob-cli`, PyPI `chronicle-lob`, or `install.sh`) exposes six deterministic subcommands: `verify`, `replay`, `snapshot`, `checkpoint`, `bench`, and `completions`.

### Configuration vs snapshot flags

For `verify`, `replay`, `snapshot`, and `checkpoint`, pass **either**:
- `--snapshot <PATH>` (reads `BookConfig` and base book state from the verified `CSNP` v1 header), **or**
- `--max-orders <U32> --grid-len <U32> --tick-size <U64> --grid-start-tick <U64>` (optional `--default-stp <cancel-incoming|cancel-resting|cancel-both>`).

Passing both `--snapshot` and explicit grid flags is a usage error (`exit 2`) by design so configuration drift cannot go unnoticed.

### Exit codes and machine-readable output

| Exit code | Meaning | Typical trigger |
| --- | --- | --- |
| `0` | Success | Healthy `verify` (`verdict: ok`, including clean crash-tail truncation reported in `discarded=<N>`), clean `replay`, atomic `snapshot` / `checkpoint`, or `bench`. |
| `1` | Corrupt durable state | `verify` detected bad magic/version, nonzero reserved bytes, CRC32C mismatch, sequence gap, or malformed snapshot state (`verdict: corrupt ...` on `stdout`). |
| `2` | Usage, I/O, or replay rejection | Invalid CLI flags, unreadable file path, or a command frame rejected by the book during `replay` (`error: ...` on `stderr`). |

- **JSON benchmark output.** Run `chronicle-cli bench --ops 100000 --seed 42 --json` for single-line deterministic JSON (`seed`, `ops`, `applied`, `rejected`, `trades`, `live_orders`, `best_bid`, `best_ask`).
- **Deterministic seeding.** Every subcommand accepts `--seed <U64>` and honours the `SIM_SEED` environment variable (`INV-DET-2`). `replay` prints `seed=<N>` on its first line followed by one `<seq> <event_kind> <k=v ...>` record per line.

## Using the `chronicle_lob` Python SDK

`python/chronicle_lob/__init__.py` provides zero-dependency pure-Python `CWAL` v1 and `CSNP` v1 binary codecs alongside typed `chronicle-cli` wrappers:

- **Pure-Python binary codecs (no subprocess required):** `WalBuilder`, `read_wal_frames`, `iter_wal_frames`, `read_snapshot_header`, `crc32c`, `BookSpec`.
- **CLI-backed operations (locates `chronicle-cli` via `CHRONICLE_CLI_BIN`, Python environment `bin/`/`Scripts/`, `target/{release,debug}/`, or `PATH`):** `verify`, `replay`, `snapshot`, `checkpoint`, `bench`, `completions`, `version`.

See [examples/market_session_and_replay.py](examples/market_session_and_replay.py) for a complete runnable workflow.

## Non-negotiable repository invariants (when editing code or docs)

1. **Std-only core (`zero-dep-core`).** `crates/chronicle-core/Cargo.toml` must have zero non-dev dependencies.
2. **Determinism gate (`INV-DET-1`).** Source files under `crates/chronicle-core/src` (excluding `reference_book.rs`) and `crates/chronicle-wal/src` must **never** contain the tokens `SystemTime`, `Instant`, `thread_rng`, `RandomState`, `DefaultHasher`, `HashMap`, or `HashSet`, even inside comments. CI scans source text directly and fails on any match.
3. **Forbid unsafe.** Every production target keeps `#![forbid(unsafe_code)]`.
4. **Prose and README gates (`CHECK-DOC-1`).**
   - Never put measured latency or throughput numbers (`ns`, `ops/sec`, `MiB/s`, etc.) in `README.md`; recorded numbers belong only in `docs/PERFORMANCE.md`.
   - Keep all links in `README.md` as canonical `https://...` URLs so PyPI renders the package page without broken relative links.
   - Never use em dashes or en dashes in Markdown files; use standard ASCII hyphens (`-`), commas, or periods.
5. **Local verification gate.** Run `make ci` (and `make book` plus `make fuzz-check` when touching docs or decoders) with `--locked` before concluding any change.
