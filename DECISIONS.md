# DECISIONS.md (LOCKED by David 2026-11-12 via APPROVE DEFAULTS)

- Q1 GitHub owner / namespace: david-engelmann (David's primary personal GitHub account).
- Q2 Repository visibility: public from creation.
- Q3 License: MIT OR Apache-2.0 (dual licensed).
- Q4 Rust toolchain and edition: Rust 2024 edition, latest stable toolchain pinned via rust-toolchain.toml, Cargo.lock committed.
- Q5 Zero-dependency core rule: YES. chronicle-core is strictly std-only. External crates (crc32fast, sha2, serde_json, clap, criterion, proptest) are restricted to I/O adapters, CLIs, benchmarks, and dev-dependencies.
- Q6 CI seed stress volume: 1,000 seeds per PR, 10,000 seeds on main (PROPTEST_CASES and SIM_SEED).
- Q7 Formal invariant traceability matrix: YES. docs/INVARIANTS.md maps every formal safety, durability, and latency invariant 1-to-1 to the exact property tests, fault oracles, or benchmarks enforcing it.
- Q8 Sub-chat execution cadence: parallel. Your sibling chats (simraft-kv, verirun) run concurrently; you own chronicle-lob only.
