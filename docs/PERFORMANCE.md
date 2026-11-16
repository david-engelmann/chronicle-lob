# Performance baseline

This file is the one place measured numbers live, per the rule in
docs/INVARIANTS.md (CHECK-DOC-1). The README and the invariant
matrix quote no numbers. Every figure below names its benchmark,
its workload, and the machine class it was measured on, and a
figure is only ever compared against a later run of the same
benchmark on the same machine class.

## Baseline of 2026-11-16

Source: benchmarks workflow run 43933403898, dispatched manually
against main at commit cd05902d, executed on a GitHub-hosted
ubuntu-latest runner. Criterion ran with its default settings.
Each cell gives the median estimate with criterion's confidence
range in brackets.

| Benchmark | Workload | Time | Throughput |
| --- | --- | --- | --- |
| add_cancel_best/add_resting | One resting limit add per iteration on a book holding 12,000 live orders across 240 levels | 123.45 µs [122.99, 123.89] | 8.10 thousand ops/s |
| add_cancel_best/cancel | One cancel per iteration on the same occupied book | 117.82 µs [117.15, 118.40] | 8.49 thousand ops/s |
| add_cancel_best/best_bid_ask | One best bid and best ask lookup pair per iteration on the same occupied book | 11.04 ns [11.02, 11.06] | 90.60 million ops/s |
| match_sweep/sweep_60_levels_600_makers | One incoming order filling 600 resting makers across 60 levels | 38.23 µs [36.35, 39.85] | 15.70 million makers/s |
| depth/depth_100_levels | One depth query returning 100 levels from a book with 200 active levels and ten orders per level | 1.10 µs [1.08, 1.11] | 91.24 million levels/s |
| wal_append/append_add_frame | One 72-byte add frame per append into a discarding sink, encoding and framing only, no storage device | 135.37 ns [135.31, 135.45] | 507.22 MiB/s |
| recovery/recover_5000_frames | Full replay of a 5,000-frame log of add frames (360,000 bytes) into a fresh book | 1.06 ms [1.05, 1.06] | 325.00 MiB/s |

## Reading this baseline

The add and cancel figures come from a batched harness that
rebuilds the occupied book per batch, so they carry more harness
weight than the raw operation cost. They are still the comparison
point for later runs of the same harness, which is the job of a
baseline. The best-price lookup, the sweep, the depth query, the
append, and the recovery figures measure their operations
directly.

## Refreshing the baseline

Dispatch the benchmarks workflow (.github/workflows/bench.yml)
against the main head to be measured, then update this file in a
new dated section with the new run id, the commit, and the same
table. An optimisation lands only with before and after figures
from runs on this machine class, per lane R5 of docs/ROADMAP.md.
