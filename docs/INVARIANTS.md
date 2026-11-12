# chronicle-lob Invariants and Traceability Matrix

Status: Milestone 0, normative. DECISIONS.md Q7 requires this matrix:
every formal safety, durability, and latency invariant maps 1-to-1 to the
exact property test, fault oracle, or benchmark that enforces it.

Rules for this document:

- An invariant without a named enforcement does not exist. If a claim
  cannot be enforced by one of the named checks, the claim is removed
  from the documentation, not left as prose.
- Test and benchmark names below are the names the code will carry from
  the milestone that introduces them. Renaming a test means updating
  this file in the same commit.
- "Enforced at" is the first milestone whose CI runs the check. Once
  enforced, a check never leaves CI.
- Architecture references point at docs/ARCHITECTURE.md sections, which
  carry the design reasoning. This file carries the statements and the
  proofs.

Conventions: property tests are proptest tests in the named crate test
targets, scaled by PROPTEST_CASES (1,000 per PR, 10,000 on main,
DECISIONS.md Q6) and seeded by SIM_SEED, printed on every run.
debug_assert_invariants() is the structural checker defined in
ARCHITECTURE.md section 11; property tests call it after every generated
command unless a row says otherwise.

## 1. Allocation invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-ALLOC-1 | After construction, add_order, cancel_order, cancel_by_participant, amend_order, the match loop, and depth perform exactly zero heap allocations in steady state, over 1,000,000 mixed operations that include full-capacity books, multi-level sweeps, mass cancels, and every rejection path. | chronicle-core integration test alloc_zero_steady_state_1m_ops, using a thread-local counting GlobalAlloc in the dedicated test target (ARCHITECTURE.md section 2.4). Thread-local counting isolates the measurement from parallel test threads. The warm-up phase fills the book once; the measured phase asserts a zero allocation delta. | M4 |
| INV-ALLOC-2 | Construction allocates each arena column, level array, bitmap level, and lookup column exactly once, at the configured capacity, and no structure grows afterwards. | Unit test construction_allocates_fixed_capacity_once plus the capacity assertions inside alloc_zero_steady_state_1m_ops at full capacity. | M1, proof completed at M4 |
| INV-ALLOC-3 | WAL append reuses one encode buffer and allocates nothing per frame beyond the file write itself; snapshot save reuses its encode buffer the same way. | chronicle-wal test wal_append_reuses_buffer, asserting buffer capacity is unchanged across 10,000 appends, plus the same thread-local counter run around the writer in alloc_wal_append_zero_steady_state. | M3, counter proof at M4 |

## 2. Book, priority, and aggregate invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-BOOK-1 | After every accepted or rejected command, if both sides are non-empty then best_bid < best_ask. The book is never crossed and never locked. | debug_assert_invariants check crossed_book, called after every command in prop_book_matches_reference and in prop_add_cancel_amend_storm. Targeted unit test amend_that_would_cross_is_rejected covers the amend path of ARCHITECTURE.md section 7.4. | M1 for adds, M2 for amends |
| INV-BOOK-2 | Within every price level, orders execute in strict FIFO arrival order. A partially filled maker keeps its head position. The residual quantity of a partially filled incoming Limit order rests at the tail of its level with the priority of its arrival. | prop_book_matches_reference compares the full event stream against the reference book, which orders fills by arrival; any priority violation changes the stream. Targeted test fifo_partial_fill_keeps_head_and_residual_rests_at_tail pins the two named cases without randomness. | M1 |
| INV-BOOK-3 | For every level, cached level_qty equals the exact sum of remaining quantities of live orders at that level, and cached level_count equals their number. | debug_assert_invariants recomputes both from the arena after every command in every property test. Unit test level_aggregates_track_fill_cancel_amend walks one level through each mutation kind. | M1 |
| INV-BOOK-4 | A Trade always executes at the maker price, for min(taker remaining, maker remaining), and total traded quantity is conserved: the sum of fill quantities equals the quantity removed from the book plus the quantity consumed from the taker, with no creation or loss. | prop_book_matches_reference event stream comparison (price and quantity are event fields) plus prop_trade_quantity_conservation, which sums events independently of engine internals. | M1 |

## 3. Bitmap invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-BITMAP-1 | For every bitmap word at every level above 0, a parent summary bit is 1 if and only if its child word is non-zero. For level 0, a bit is 1 if and only if the level has at least one live order on that side. | debug_assert_invariants recomputes the whole hierarchy from level_count after every command in every property test. Unit test bitmap_summary_sync_on_last_order_removal covers the deactivation edge where propagation must reach the root. | M1 |
| INV-BITMAP-2 | best_bid returns the highest active bid tick and best_ask the lowest active ask tick, and each returns None exactly when its side is empty. | prop_best_price_matches_brute_force scans all levels naively as its oracle over generated books, including empty and single-level books. | M1 |

## 4. Arena and handle invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-ARENA-1 | The free list contains exactly the set of free slots, each exactly once, with no cycles. Every live slot is reachable from exactly one level list, and level list links are symmetric: for live slots a and b, next(a) = b implies prev(b) = a, and head and tail agree with the list ends. | debug_assert_invariants free list and link checks after every command in every property test. Unit test arena_free_list_exact_after_churn drives interleaved insert and release to full capacity and back to empty. | M1 |
| INV-ARENA-2 | Releasing a slot zeroes its order_id, participant, price_idx, qty, and side fields and sets prev and next to NIL or the free list link, so no stale order data is observable in a free slot. | Unit test arena_release_zeroes_slot inspects the columns directly after release, in the crate internal test module. | M1 |
| INV-ARENA-3 | A lookup entry validates against the arena only when the stored generation equals the slot current generation and the slot is live. A released and reused slot never validates under its previous generation. | Unit test arena_generation_rejects_stale_handle captures a (slot, generation) pair, releases and reallocates the slot, and asserts validation fails for the old pair and succeeds for the new one. Property coverage via prop_lookup_reachability_under_churn. | M1 |
| INV-ARENA-4 | Generation overflow on one slot retires that slot permanently instead of wrapping. Effective capacity drops by one and no handle is ever issued for the retired slot again. | Unit test arena_generation_overflow_retires_slot sets a slot generation to u32::MAX - 1 in the internal test module, releases twice, and asserts the slot never returns from allocation. This is a branch test of specified behaviour (ARCHITECTURE.md section 4.3), not a load test. | M1 |

## 5. Lookup table invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-LOOKUP-1 | The table never exceeds 50 percent load. Insert that would cross the cap returns LookupCapacityFull or is pre-empted by the arena CapacityFull check, before any mutation. Capacity is the smallest power of two at least 2 * max_orders. | Unit test lookup_load_cap_enforced fills the table to the cap through the public book API and directly, and asserts the error and the unchanged len. | M1 |
| INV-LOOKUP-2 | After any sequence of inserts and deletes, including deletes from the middle of probe clusters, every remaining key is findable by linear scan from its home bucket without crossing an empty bucket. Deletion uses Knuth Algorithm R backward shift and leaves zero tombstones, which this reachability property states operationally. | prop_lookup_backward_shift_reachability generates insert and delete sequences biased to clustered hashes, deletes, then asserts findability and exact contents against a BTreeMap oracle after every operation. | M1 |
| INV-LOOKUP-3 | Lookup behaviour is identical across processes for the same operation sequence: the hash function is the fixed SplitMix64 finalizer of ARCHITECTURE.md section 5.2, with no process seed. | Unit test lookup_hash_is_splitmix64_golden pins hash outputs for a fixed input set against golden values. Probe-order determinism is covered end to end by INV-DET-1. | M1 |

## 6. Order type and atomicity invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-ORD-1 | A Fok order either fills its full quantity or mutates nothing: no resting order, aggregate, bitmap, lookup entry, or seq changes when it returns FokInsufficientLiquidity { requested, available }, and available is the exact STP-adjusted executable quantity of ARCHITECTURE.md section 7.5. | prop_fok_is_atomic snapshots full book state (via depth, best prices, and the invariant checker state hash used in tests) before a failing Fok and asserts equality after. Unit test fok_available_accounts_for_stp pins the STP interaction cases. | M2 |
| INV-ORD-2 | A PostOnly order whose price would cross the opposite best is rejected with PostOnlyWouldCross and mutates nothing, including seq. A PostOnly order that does not cross rests in full. | prop_post_only_never_crosses plus unit test post_only_rejection_is_atomic with full state equality, mirroring INV-ORD-1. | M2 |
| INV-ORD-3 | An Ioc order never rests. Its unfilled remainder is reported Expired in the same command and no order with its OrderId is live afterwards. | prop_ioc_never_rests asserts lookup absence and event shape after every generated Ioc command. | M2 |
| INV-ORD-4 | Every rejected command of any kind leaves book state and seq bit-identical to before the call. Rejection is decided in validation or preflight, never mid-execution (ARCHITECTURE.md section 7.2). | prop_rejections_are_atomic generates commands biased to invalid inputs (zero qty, bad tick, duplicate id, full book, failing Fok, crossing PostOnly, undersized event buffer) and asserts full state equality and unchanged seq after each rejection. This is the umbrella atomicity property; INV-ORD-1, INV-ORD-2, and INV-EVENT-1 are its named special cases. | M2 |

## 7. Amend, cancel, and mass-cancel invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-AMEND-1 | Amending to a smaller quantity at the same price preserves the order exact FIFO position. Amending to the identical price and quantity is a no-op for position, quantity, and aggregates, and still consumes one seq and emits one Amended event. | Unit tests amend_reduce_preserves_priority and amend_identical_is_noop inspect level list order directly and through fill order. Property coverage in prop_book_matches_reference, whose oracle implements the same rules independently. | M2 |
| INV-AMEND-2 | Amending to a larger quantity at the same price, or to any different price, moves the order to the tail of the target level, behind every order already resting there. | Unit test amend_increase_or_reprice_goes_to_tail, verified by the fill order of a subsequent sweep, not only by list inspection. | M2 |
| INV-AMEND-3 | Cancel or amend of a missing, filled, or already cancelled OrderId returns OrderNotFound { order_id } with zero side effects. | prop_cancel_amend_missing_id_is_noop over generated terminal and never-seen ids, with full state equality. | M2 |
| INV-CANCEL-1 | cancel_by_participant cancels exactly the live orders of that participant, no others, emits one Cancelled event per order in ascending SlotIdx order, and leaves levels, bitmaps, and aggregates consistent for the survivors. | prop_cancel_by_participant_exact_set compares the cancelled id set and the surviving book against the reference book; unit test cancel_by_participant_event_order_is_slot_order pins the event order the snapshot format depends on (ARCHITECTURE.md section 10.4). | M2 |

## 8. Self-trade prevention invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-STP-1 | Under CancelIncoming, a taker that reaches its own resting order stops, its remainder is cancelled with a SelfTradePrevented event, the resting order is untouched, and fills already made earlier in the sweep stand. | Unit test stp_cancel_incoming_stops_at_own_order builds the three-maker case (other, own, other) and asserts the third maker is never reached. | M2 |
| INV-STP-2 | Under CancelResting, each own resting order in the sweep path is cancelled with reason SelfTradePrevention and the sweep continues past it without consuming taker quantity. | Unit test stp_cancel_resting_skips_own_orders, same three-maker layout. | M2 |
| INV-STP-3 | Under CancelBoth, the first own resting order in the path is cancelled and the taker remainder is cancelled in the same command. | Unit test stp_cancel_both_cancels_each_side, same layout. Property coverage for all three policies in prop_stp_policies_match_oracle, where the reference book implements the policies independently. | M2 |

## 9. Event buffer invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-EVENT-1 | If the caller EventBuffer has fewer free slots than the preflight worst-case event bound, the command returns EventBufferFull { needed, available } before mutating any book state, including seq. | Unit test event_buffer_full_is_atomic fills a book whose sweep needs a known number of events and offers buffers of size bound - 1, asserting the error values and full state equality. | M2 |
| INV-EVENT-2 | Retrying the identical command with a sufficient buffer after an EventBufferFull returns the identical event stream and final state as a first attempt with a sufficient buffer. | prop_event_buffer_retry_is_identical runs generated commands twice, once starved then fed, once fed, and compares streams and state. | M2 |
| INV-EVENT-3 | Event emission order inside a command is fixed (ARCHITECTURE.md section 7.6) and events contain only seq, OrderIds, price, and quantity: no SlotIdx, no timestamps. Two runs of the same command sequence produce byte-identical event streams. | prop_book_matches_reference compares complete streams, and INV-DET-1 compares streams across processes. The event type field set is pinned by the golden event encoding in unit test event_shape_golden. | M2 |

## 10. WAL invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-WAL-1 | Every frame on disk matches the byte format of ARCHITECTURE.md section 9.1 and 9.2 exactly: magic "CWAL", version 1, header_len 32, fixed payload width per kind, zero reserved fields, little-endian throughout. | Unit test wal_frame_golden_vectors encodes one frame of each kind and compares against hand-computed byte strings; decode of the same bytes returns the original command and seq. | M3 |
| INV-WAL-2 | Truncation is a clean crash boundary and corruption is an error, per the decision table of ARCHITECTURE.md section 9.5, with no third outcome and no panic at any offset. | Fault oracle fault_wal_truncate_at_every_offset takes a valid multi-frame log and truncates at every byte offset, asserting recovery succeeds with exactly the complete prefix frames. Fault oracle fault_wal_corrupt_bytes flips bytes in magic, version, header fields, payload, and CRC of complete frames and asserts CorruptWalRecord { offset } naming the frame start. | M3 |
| INV-WAL-3 | A complete frame whose CRC does not match is corruption, never a crash boundary, and is reported as CorruptWalRecord { offset, expected_crc, actual_crc }. CRC input is domain separated by the "chronicle-wal/v1" prefix, so a WAL frame never validates under a snapshot CRC or a future version prefix. | fault_wal_corrupt_bytes includes CRC field flips and payload flips; unit test wal_crc_domain_separation asserts a frame CRC recomputed under the snapshot domain does not validate. | M3 |
| INV-WAL-4 | Replayed seqs must be consecutive. The first replayed frame after a snapshot point must equal last_included_seq + 1 and each next frame must step by exactly 1, otherwise WalSequenceGap { expected, found, offset }. Frames at or below last_included_seq are skipped, never applied twice. | Fault oracle fault_wal_sequence_gap splices duplicated, dropped, and reordered frames into a valid log and asserts the gap error values. Unit test wal_replay_skips_snapshot_covered_frames covers the skip rule. | M3 |
| INV-WAL-5 | Replay of a WAL through the Book pipeline reproduces the original book state and event stream exactly: same best prices, same full depth, same per-command events. | Property test prop_wal_replay_reproduces_book generates command streams, logs them, recovers into a fresh book, and compares state and streams. This is the WAL half of the recovery equivalence. | M3 |
| INV-WAL-6 | Under SyncPolicy::Always, every append the writer reports as successful has been written with write_all and sync_data before the call returns. Weaker policies are explicit operator choices and the code names them at the call site. | Unit test wal_writer_sync_policy_always_calls_sync uses a counting wrapper around the writer sync hook in tests to assert one sync per append under Always and zero under Never. The durability claim itself is scoped by the storage-stack caveat in ARCHITECTURE.md section 9.4; no test claims to prove hardware behaviour. | M3 |

## 11. Snapshot invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-SNAP-1 | A snapshot file is exactly 64 + 4 * max_orders + 20 * grid_len + 56 * live_order_count + 4 bytes, with header fields per ARCHITECTURE.md section 10.2, next_seq equal to last_included_seq + 1, and a trailing CRC32C over the "chronicle-snapshot/v1" domain prefix and all preceding bytes. Any deviation is CorruptSnapshot or SnapshotConfigMismatch, never a partial load. | Unit test snapshot_layout_golden pins header bytes and total size for a small book. Tamper oracle tamper_snapshot_every_header_field mutates each header field and asserts the typed error; fault test tamper_snapshot_body_and_trailer flips sampled body and trailer bytes and asserts CorruptSnapshot with the CRC values. | M3 |
| INV-SNAP-2 | Snapshot save is atomic under crash at any step: a reader sees the complete old snapshot or the complete new one, never a mixture, and a leftover ".tmp" file is never loaded. | Fault oracle fault_snapshot_crash_at_each_step interrupts the save protocol of ARCHITECTURE.md section 10.3 between steps (the writer exposes step hooks under cfg(test)) and asserts load returns the old or the new snapshot in full. Unit test snapshot_loader_ignores_tmp_file plants a garbage ".tmp" beside a valid snapshot. | M3 |
| INV-SNAP-3 | Snapshot plus trailing WAL replay is equivalent to full WAL replay: recovering from a snapshot at seq s plus frames after s yields the identical book state and the identical subsequent event stream as recovering from the empty snapshot plus all frames, including the event order of the first cancel_by_participant after recovery. | Property test prop_snapshot_plus_wal_equals_full_replay generates a command stream, snapshots at a random midpoint, and compares both recovery paths state by state and then command by command, including a forced mass cancel immediately after recovery. | M3 |
| INV-SNAP-4 | Snapshot restore preserves generations for all slots, free and live, so a handle or lookup entry from before the snapshot cannot validate against a reused slot after restore; and restore sets next_seq to last_included_seq + 1 before any WAL replay, ending at max_recovered_seq + 1 after replay. | Unit test snapshot_restore_preserves_generations replays the INV-ARENA-3 stale handle scenario across a snapshot boundary. Sequence assertions live in prop_snapshot_plus_wal_equals_full_replay and in unit test recover_next_seq_is_max_plus_one for the empty trailing WAL case. | M3 |
| INV-SNAP-5 | A snapshot restores only into a book with the identical BookConfig. Any differing field is SnapshotConfigMismatch naming the field and both values. | Unit test snapshot_config_mismatch_names_field varies each config field in turn. | M3 |

## 12. Checked math and input guard invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-MATH-1 | No engine path panics or wraps on user input. Per-level and per-order quantity math uses checked_add and checked_sub; cumulative volume and notional use u128 widening. Overflow of a guarded aggregate is the typed ArithmeticOverflow error, unreachable in a valid book. | Property tests run with overflow checks enabled in the test profile (overflow-checks = true for the workspace test profile in Cargo.toml, Milestone 1) so any wrap panics loudly in tests instead of passing silently. Unit test math_checked_paths_at_u64_edges drives quantities at u64::MAX boundaries through the validation layer and asserts typed rejections, never a wrap. | M1, edges at M2 |
| INV-MATH-2 | Zero quantity, zero OrderId, duplicate active OrderId, and any price failing the tick rules of ARCHITECTURE.md section 3.2 are rejected with their typed errors and zero mutation. | Unit tests guard_zero_qty, guard_zero_order_id, guard_duplicate_order_id, guard_invalid_price_tick_each_rule (one case per tick rule: zero price, unaligned price, tick below grid, tick above grid), each with full state equality. Umbrella coverage in prop_rejections_are_atomic. | M1 for add guards, M2 for the umbrella |

## 13. Determinism invariants

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-DET-1 | The same command sequence at the same SIM_SEED produces byte-identical event traces across separate OS processes. No clock, OS randomness, RandomState, or HashMap or HashSet iteration order influences any core or recovery behaviour. | CLI level test det_cli_replay_twice_byte_identical (chronicle-cli) runs replay twice in separate processes at a fixed seed and compares output hashes. Core level coverage: prop_book_matches_reference uses seeded generation only. A CI grep gate (in the test job from M1) forbids the tokens SystemTime, Instant::now, thread_rng, RandomState, HashMap, and HashSet in chronicle-core sources outside the cfg(test) reference book, which uses BTreeMap only. | M1 for the grep gate, M4 for the CLI proof |
| INV-DET-2 | Every simulation and property runner prints its active seed, and a failing case reproduces from its printed seed and PROPTEST_CASES settings alone. | CONTRIBUTING.md documents the reproduction workflow (M4). Unit test seed_banner_printed asserts the CLI bench and replay commands emit the seed line at startup. | M4 |

## 14. Latency and throughput invariants

Latency claims are benchmark obligations, not prose. Each benchmark runs
in CI in compile-check mode (cargo bench --workspace -- --test) on every
PR from M4, and full runs produce the numbers quoted in README.md
section 3. A benchmark that stops compiling or loses its throughput
configuration fails CI.

| ID | Invariant | Enforcement | Enforced at |
| --- | --- | --- | --- |
| INV-LAT-1 | add_order (resting, no cross), cancel_order, and best_bid and best_ask are measured with p50, p95, p99, and p99.9 reported, on a book at realistic occupancy (at least 10,000 live orders across at least 200 active levels), with inputs and outputs wrapped in black_box. | criterion benchmark bench_add_cancel_best in chronicle-core benches, BenchmarkGroup throughput set to Throughput::Elements(1) per operation batch. | M4 |
| INV-LAT-2 | Match sweep cost is measured per filled maker order, including a multi-level sweep of at least 50 levels, so the per-level bitmap descent cost is visible in the results. | criterion benchmark bench_match_sweep, Throughput::Elements set to the number of maker orders filled. | M4 |
| INV-LAT-3 | depth cost is measured against the number of levels returned. | criterion benchmark bench_depth, Throughput::Elements set to levels returned. | M4 |
| INV-LAT-4 | WAL append throughput and full recovery throughput are measured in bytes per second and frames per second. | criterion benchmarks bench_wal_append and bench_recovery in chronicle-wal benches, Throughput::Bytes set to frame bytes written or read. | M4 |
| INV-LAT-5 | No latency number appears in README.md or release notes unless it names the benchmark, the book configuration, and the machine class it was measured on. | Documentation rule, checked at the M4 milestone review against bench output saved with the release. This is a review gate, not a test, and it is recorded here so the obligation is visible in the matrix. | M4 |

## 15. Adversarial edge case catalogue

The cases below are the standing adversarial list. Each is owned by the
invariant and check named in the last column; a case with no owner is a
defect in this document.

| Case | Expected behaviour | Owner |
| --- | --- | --- |
| Book filled to max_orders, then one more add | CapacityFull, zero mutation | INV-ORD-4, INV-MATH-2 |
| Sweep that empties the entire opposite side | Taker remainder rests (Limit), expires (Ioc), or was pre-rejected (Fok); bitmaps fully clear on the emptied side | INV-BITMAP-1, INV-ORD-3 |
| Cancel the only order at the best price while deeper levels remain | Best price moves exactly one active level inward, summary bits propagate to the root | INV-BITMAP-1, INV-BITMAP-2 |
| Amend the head order of a level to a larger quantity | Order moves behind orders that arrived later than it at that level | INV-AMEND-2 |
| Fok order that would fill only if its own resting orders counted | Fails with the STP-adjusted available quantity | INV-ORD-1 |
| PostOnly order priced exactly at the opposite best | Crosses by equality (buy price >= best ask), rejected | INV-ORD-2 |
| EventBuffer with exactly the worst-case bound, and with bound - 1 | Bound succeeds fully; bound - 1 returns EventBufferFull atomically | INV-EVENT-1 |
| WAL cut at every possible byte offset of a multi-frame log | Recovery returns exactly the complete prefix, reports discarded trailing bytes | INV-WAL-2 |
| WAL with one payload byte flipped in the middle frame | CorruptWalRecord at that frame offset; frames after it are never applied | INV-WAL-2, INV-WAL-3 |
| WAL with a frame duplicated after a snapshot | Skipped if at or below last_included_seq; gap error only if it breaks the +1 rule for replayed frames | INV-WAL-4 |
| Snapshot saved, crash simulated between rename and directory sync | Old or new snapshot loads in full; a third outcome fails the oracle | INV-SNAP-2 |
| Snapshot taken of a full book, restored, then mass cancel | Event order and contents match the no-snapshot run exactly | INV-SNAP-3, INV-CANCEL-1 |
| OrderId reused immediately after its order filled | Accepted as a new order; the old lookup entry and generation never validate | INV-ARENA-3 |
| Quantity and price at u64 extremes that still pass tick validation | Checked math rejects or handles without wrap or panic; notional accumulates in u128 | INV-MATH-1 |
| cancel_by_participant for a participant with no orders | Succeeds, zero events, zero mutation beyond seq consumption of an accepted command | INV-CANCEL-1, prop suite |
| Amend an order to the price it already sits at with a smaller quantity while it is mid-level | Priority preserved, aggregates drop by the difference only | INV-AMEND-1, INV-BOOK-3 |
| Two runs of the full CLI replay at one seed in separate processes | Byte-identical output | INV-DET-1 |

## 16. Threat model note

CRC32C detects torn writes, partial flushes, and incidental corruption.
It does not authenticate. An attacker who can rewrite a WAL or snapshot
file can recompute its CRC and the format will accept the result.
Protecting against that adversary requires a MAC or signatures, which
are out of scope for version 1 of both formats and are a versioned
format change if ever added. Every durability statement in this
repository is made under this note.
