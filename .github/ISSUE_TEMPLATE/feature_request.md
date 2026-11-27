---
name: Feature request
about: Propose an improvement to chronicle-core, chronicle-wal, chronicle-cli, or the Python SDK
labels: enhancement
---

## Problem or use case

<!-- Describe the workflow, market rule, or integration need this feature addresses. -->

## Proposed behaviour

<!-- Sketch the API, CLI subcommand or flag, or wire behaviour you have in mind. -->

## Determinism and allocation impact

`chronicle-core` stays `std`-only with zero steady-state heap allocation, and recovery runs in bounded memory. Please note how the proposal preserves:

- Deterministic execution without wall-clock calls, ambient randomness, or hash-iteration order
- Preallocated capacity and caller-owned event buffers on the hot path
- Compatibility with `CWAL` v1 and `CSNP` v1 records when persistence is involved
