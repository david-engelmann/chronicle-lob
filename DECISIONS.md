# Decisions

Locked by David on 2026-11-12. Any change to a decision in this file needs an explicit ruling from the project owner.

- Q1 GitHub owner and namespace is `david-engelmann` on GitHub.
- Q2 Repository visibility is public from creation.
- Q3 License is dual `MIT OR Apache-2.0`.
- Q4 Rust toolchain and edition use Rust 2024 edition with a pinned stable toolchain in `rust-toolchain.toml` and `Cargo.lock` committed.
- Q5 Zero-dependency core rule holds strictly. `chronicle-core` uses the Rust standard library only. External crates (`serde_json`, `clap`, `criterion`, and `proptest`) are restricted to the CLI, benchmarks, and dev-dependencies.
- Q6 CI seed stress volume runs 1,000 seeds per pull request and 10,000 seeds on `main` through `PROPTEST_CASES` and `SIM_SEED`.
- Q7 Formal invariant traceability matrix in `docs/INVARIANTS.md` maps every formal safety, durability, determinism, and latency invariant one-to-one to the property tests, fault oracles, or benchmarks enforcing it.
- Q8 Repository scope is isolated to `chronicle-lob` with all changes gated by `chronicle-lob` CI.
