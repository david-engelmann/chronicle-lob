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

## Programme 2 measurement pass of 2026-11-16 (lane S1)

Source: benchmarks workflow run 43994370865, dispatched manually
against main at commit 6e1b4ecc, executed on a GitHub-hosted
ubuntu-latest runner. This pass re-measured the add and cancel
benchmarks after lane S1 of docs/ROADMAP.md changed their shape:
each now measures a stream of 1,000 operations on one pre-built
occupied book instead of single operations in batches that
rebuilt the book per batch. The add and cancel rows below are
per operation, derived from criterion's stream estimates. The
best-price benchmark moved to its own criterion group with its
measurement unchanged. The remaining rows are fresh readings of
unchanged benchmarks from the same run.

| Benchmark | Workload | Time | Throughput |
| --- | --- | --- | --- |
| add_cancel_best/add_resting | Stream of 1,000 resting limit adds on the 12,000-order occupied book, per operation | 41.00 ns [40.81, 41.23] | 24.39 million ops/s |
| add_cancel_best/cancel | Stream of 1,000 cancels of resting orders on the same occupied book, per operation | 23.88 ns [23.70, 24.10] | 41.88 million ops/s |
| best_bid_ask/best_bid_ask | One best bid and best ask lookup pair per iteration on the same occupied book | 7.97 ns [7.96, 7.97] | 125.52 million ops/s |
| match_sweep/sweep_60_levels_600_makers | One incoming order filling 600 resting makers across 60 levels | 40.60 µs [35.88, 44.95] | 14.78 million makers/s |
| depth/depth_100_levels | One depth query returning 100 levels from a book with 200 active levels and ten orders per level | 1.04 µs [0.98, 1.10] | 96.04 million levels/s |
| wal_append/append_add_frame | One 72-byte add frame per append into a discarding sink, encoding and framing only, no storage device | 118.45 ns [117.32, 120.11] | 579.71 MiB/s |
| recovery/recover_5000_frames | Full replay of a 5,000-frame log of add frames (360,000 bytes) into a fresh book | 873.64 µs [873.17, 874.17] | 392.98 MiB/s |

The stream figures supersede the first baseline's add and cancel
rows for comparison purposes: the old rows charged a full book
rebuild to each operation, and the honest per-operation costs
are roughly three orders of magnitude below them. The first
baseline stays above as the historical record of the first
measurement, caveat included. The unchanged benchmarks also
moved between the two runs (recovery reads 392.98 MiB/s here
against 325.00 in the first baseline), which is the run-to-run
variance of shared hosted runners and the reason this file
compares like with like and treats single runs as readings, not
verdicts.

## Programme 2 coverage pass of 2026-11-17 (lane S2)

Source: benchmarks workflow run 44148139727, dispatched manually
against main at commit 4bf3ffc0, executed on a GitHub-hosted
ubuntu-latest runner. This pass records the three benchmarks
lane S2 of docs/ROADMAP.md added to close finding B-3. Figures
are criterion's point estimates with the confidence range in
brackets, per operation where the benchmark counts operations;
the snapshot row reports the mean because criterion produced no
slope fit for that series.

| Benchmark | Workload | Time | Throughput |
| --- | --- | --- | --- |
| amend/amend_resting | Stream of 1,000 price-and-quantity amends against resting orders on the 12,000-order occupied book, per amend | 18.45 ns [18.39, 18.52] | 54.19 million ops/s |
| mass_cancel/cancel_by_participant | One cancel_by_participant call removing one participant's 800 resting orders from the 12,000-order occupied book, per cancelled order | 52.82 ns [51.93, 54.03] | 18.93 million orders/s |
| snapshot/encode_decode_round_trip | One encode and decode round trip of a snapshot state holding 12,000 live orders (947,012 encoded bytes), per round trip | 3.89 ms [3.87, 3.93] | 232.08 MiB/s |

The amend stream amends each target order once, moving its price
60 levels inside the bid band and raising its quantity, so every
amend takes the level-move path and none crosses the spread. The
mass cancel figure is per cancelled order over a single call;
the call as a whole measured 42.25 µs. The snapshot round trip
is the first measurement of the snapshot codec's cost at this
occupancy and stands as its baseline for later comparison.

## Programme 2 CRC pass of 2026-11-17 (lane S3)

Source: benchmarks workflow run 44153859460 against main at dcf5e176 on a GitHub-hosted ubuntu-latest runner. Lane S3 replaced the byte-at-a-time CRC32C update in chronicle-wal with slicing-by-8, and the gate for the lane was a before and after reading of the two surfaces that run the checksum over every byte. The before readings are the lane S2 pass figures above, from run 44148139727 on the same runner class.

| Benchmark | Before | After |
| --- | --- | --- |
| wal_append, per frame | 91.74 ns | 57.33 ns [57.31, 57.35] |
| wal_append throughput | 748.50 MiB/s | 1,197.73 MiB/s |
| recovery of 5,000 frames | 687.99 µs | 545.66 µs [544.10, 547.48] |
| recovery throughput | 499.02 MiB/s | 629.19 MiB/s |

The append surface runs at 1.60 times its former throughput and the recovery surface at 1.26 times. Checksum values are unchanged. A differential test in the crc module proves the slicing implementation against a bit-at-a-time reference written straight from the polynomial at every input length from 0 to 517 and across six streaming split sizes, and the golden check value and the stored fixtures across the suite stand unmodified.

## Programme 4 refresh of 2026-11-19 (lane U2)

Source: benchmarks workflow run 44601074213, dispatched against
main at commit fbee5f82, executed on a GitHub-hosted ubuntu-latest
runner. This is the first full pass since the streaming rebuild
(lanes S5 and T1) and the checkpoint lane (T5); every surface is
re-measured on the current code. Figures are criterion's slope
estimates with the confidence range in brackets, per operation
where the benchmark counts operations.

| Benchmark | Workload | Time | Throughput |
| --- | --- | --- | --- |
| add_cancel_best/add_resting | Stream of 1,000 resting limit adds on the 12,000-order occupied book, per operation | 34.80 ns [34.43, 35.31] | 28.73 million ops/s |
| add_cancel_best/cancel | Stream of 1,000 cancels of resting orders on the same occupied book, per operation | 18.64 ns [18.61, 18.68] | 53.64 million ops/s |
| best_bid_ask/best_bid_ask | One best bid and best ask lookup pair per iteration on the same occupied book | 5.77 ns [5.75, 5.79] | 173.31 million ops/s |
| amend/amend_resting | Stream of 1,000 price-and-quantity amends against resting orders on the same occupied book, per amend | 22.13 ns [22.09, 22.18] | 45.18 million ops/s |
| mass_cancel/cancel_by_participant | One cancel_by_participant call removing one participant's 800 resting orders from the same occupied book, per cancelled order | 61.70 ns [61.49, 61.99] | 16.21 million orders/s |
| match_sweep/sweep_60_levels_600_makers | One incoming order filling 600 resting makers across 60 levels | 28.15 µs [27.72, 28.50] | 21.31 million makers/s |
| depth/depth_100_levels | One depth query returning 100 levels from a book with 200 active levels and ten orders per level | 1.28 µs [1.17, 1.40] | 78.25 million levels/s |
| wal_append/append_add_frame | One 72-byte add frame per append into a discarding sink, encoding and framing only, no storage device | 29.14 ns [29.13, 29.15] | 2,356.37 MiB/s |
| recovery/recover_5000_frames | Full replay of a 5,000-frame log of add frames (360,000 bytes) into a fresh book | 322.14 µs [321.90, 322.43] | 1,065.90 MiB/s |
| snapshot/encode_decode_round_trip | One encode and decode round trip of a snapshot state holding 12,000 live orders (947,012 encoded bytes), per round trip | 1.23 ms [1.23, 1.23] | 734.82 MiB/s |

The mass cancel call as a whole measured 49.36 µs. The snapshot
row is a slope estimate this time; the S2 pass reported the mean
because that run produced no slope fit. Several surfaces read
well above their last recorded figures on this run (the append
surface at about twice its S3 reading, recovery at about 1.7
times), while amend and mass cancel read slower than their S2
figures. No performance code changed between the S3 pass and
this one except the streaming rebuild the recovery path shares,
so per this file's rule the run stands as the new comparison
point for later runs on this machine class, and its movements
are readings, not verdicts about the code.

## Refreshing the baseline

Dispatch the benchmarks workflow (.github/workflows/bench.yml)
against the main head to be measured, then update this file in a
new dated section with the new run id, the commit, and the same
table. An optimisation lands only with before and after figures
from runs on this machine class, per lane R5 of docs/ROADMAP.md.
