# chronicle-lob Architecture

Status: Milestone 0 design, locked decisions in DECISIONS.md.
This document is the design contract for Milestones 1 to 4. Where code and
this document disagree after a milestone sync, the milestone is not done.
docs/INVARIANTS.md carries the formal invariant list and the 1-to-1
traceability matrix to the tests, fault oracles, and benchmarks that
enforce each invariant.

## 1. Purpose and scope

chronicle-lob is a single-threaded, deterministic limit order book and
matching engine with a binary write-ahead log and atomic snapshots.

Design targets, in priority order:

1. Correctness that can be proved. Every safety, durability, and latency
   claim maps to a named property test, fault oracle, or benchmark in
   docs/INVARIANTS.md.
2. Zero steady-state heap allocation in the matching core. After
   construction, the hot path allocates nothing. Section 4 fixes the data
   structures that make this possible.
3. Determinism. The same command sequence at the same seed produces
   byte-identical event streams, WAL bytes, and snapshots in any process.
   No wall-clock reads, no OS randomness, and no hash-map iteration order
   anywhere in the core or the recovery path.
4. Crash recovery with a sharp boundary. A torn final WAL frame is a clean
   crash boundary. Corruption anywhere else is a typed error carrying the
   exact byte offset. Section 9 fixes the disambiguation rules.

Non-goals for this program: networking, threading or async runtimes,
multiple instruments per book instance, market data distribution, and
persistence formats other than the WAL and snapshot formats in sections 9
and 10. One book instance prices one instrument on one tick grid.

## 2. Crate and module boundaries

Workspace layout (Cargo workspace, Rust 2024 edition, toolchain pinned in
rust-toolchain.toml):

| Crate | Kind | Dependencies | Role |
| --- | --- | --- | --- |
| chronicle-core | library | none, std only | Arena, lookup table, price bitmaps, matching engine, events, errors. No I/O, no clock, no randomness. |
| chronicle-wal | library | chronicle-core, crc32fast | WAL frame writer and reader, snapshot writer and loader, recovery driver. Owns all file I/O for durable state. |
| chronicle-cli | binary | chronicle-core, chronicle-wal, clap, serde_json | Replay, verify, snapshot, and benchmark commands. The only crate that parses text or prints JSON. |

Dependency direction is one way: cli depends on wal and core, wal depends
on core, core depends on nothing. External crates are confined to the
adapter, CLI, benchmark, and dev-dependency roles listed in DECISIONS.md
Q5. chronicle-core has no non-dev dependencies at all, and CI enforces
this by parsing crates/chronicle-core/Cargo.toml in the zero-dep-core job.

### 2.1 chronicle-core modules

| Module | Contents |
| --- | --- |
| lib.rs | Crate documentation, public re-exports, forbid(unsafe_code), lint configuration. |
| config.rs | BookConfig and its validation. All capacities and the tick grid are fixed here, once, at construction. |
| types.rs | OrderId, ParticipantId, SlotIdx, Side, OrderType, StpPolicy, Price and quantity newtypes and raw conversions. |
| error.rs | BookError, every variant carries diagnostic fields (section 8). |
| arena.rs | Struct-of-arrays order arena, typed slot handles, generation checks, free list (section 4). |
| lookup.rs | OrderId to slot lookup table, SplitMix64 hashing, Knuth Algorithm R deletion (section 5). |
| bitmap.rs | Hierarchical active-price bitmaps and best bid and ask discovery (section 6). |
| level.rs | Per-price-level aggregate state (head, tail, total quantity, order count). |
| events.rs | ExecutionEvent and the caller-owned EventBuffer with its capacity contract (section 7.6). |
| book.rs | The Book state machine: add, cancel, amend, cancel_by_participant, match loop, L2 depth queries, debug_assert_invariants. |
| reference_book.rs | A deliberately simple BTreeMap based reference book, compiled only under cfg(test) and used as the property-test oracle. Never shipped in the public API. |

### 2.2 chronicle-wal modules

| Module | Contents |
| --- | --- |
| lib.rs | Crate documentation, public re-exports, forbid(unsafe_code). |
| frame.rs | Frame header and payload encode and decode for the byte format in section 9. Pure byte logic, no file I/O, unit tested against golden vectors. |
| crc.rs | CRC32C (Castagnoli) via crc32fast, with the domain-separation prefixes from sections 9.3 and 10.2 applied at this layer only. |
| writer.rs | WalWriter: appends frames to a file, reused encode buffer, configurable sync policy (section 9.4). |
| reader.rs | WalReader and the recovery scan: crash boundary versus corruption decision table (section 9.5). |
| snapshot.rs | Snapshot save and load: atomic write protocol, structural restore, CRC validation (section 10). |
| recover.rs | recover(): snapshot load plus WAL replay, sequence continuity enforcement, RecoveryReport. |

### 2.3 chronicle-cli

One binary, subcommands: replay (apply a WAL to a fresh or snapshot based
book and print the event stream), verify (scan a WAL and snapshot pair and
report sequence and CRC health), snapshot (write a snapshot from a WAL),
and bench (seeded synthetic load, plain text or JSON with --json). All
subcommands accept --seed <u64> and honour SIM_SEED. Output ordering is
fully determined by the input files and the seed.

### 2.4 Safety and lint posture

Every production library and binary target opens with
#![forbid(unsafe_code)]. The single permitted exception is the dedicated
allocation-counting integration test target in Milestone 4, and only if a
vetted tracking allocator crate proves insufficient. That target, if it
exists, carries #[allow(unsafe_code, reason = "...")] with the exact
justification written out, and no production code path depends on it.

Workspace lints deny clippy::all, clippy::pedantic, and clippy::nursery
findings, deny clippy::todo and clippy::unimplemented, deny
clippy::allow_attributes_without_reason, warn on missing_docs, and build
docs with RUSTDOCFLAGS="-D warnings". Every local allow carries a reason.

## 3. Core types and configuration

### 3.1 Identifiers and quantities

- OrderId: u64, supplied by the caller. Zero is reserved as the empty
  sentinel of the lookup table and is rejected as InvalidOrderId. Active
  OrderIds are unique; reuse after an order reaches a terminal state is
  allowed and is a new order.
- ParticipantId: u32, supplied by the caller. Zero is a valid participant.
- Price: u64 in the smallest quote unit (for example cents). Prices enter
  the tick grid only through the validation in section 3.2.
- Quantity: u64, strictly positive on entry. Zero quantity is rejected as
  InvalidQuantity.
- Sequence number (seq): u64, owned by the book. The first accepted
  command takes seq 1. Every accepted command consumes exactly one seq,
  including no-op amends. Rejected commands consume none. Seq is the join
  key between book state, WAL frames, and snapshots.

### 3.2 BookConfig and the tick grid

BookConfig fields, all validated at construction:

| Field | Type | Rule |
| --- | --- | --- |
| max_orders | u32 | Arena capacity. At least 1, at most 2^31 - 2 so SlotIdx can reserve u32::MAX as NIL. |
| grid_len | u32 | Number of price ticks on the grid. Multiple of 64, at least 64, at most 2^22. |
| tick_size | u64 | Quote units per tick, at least 1. |
| grid_start_tick | u64 | The tick value of grid index 0. Price tick t maps to index t - grid_start_tick. |
| default_stp | StpPolicy | Self-trade prevention policy applied when a command does not name one. |

Construction validates the grid arithmetic itself before anything is
allocated: grid_start_tick + grid_len, computed with checked_add after
widening grid_len to u64, must not overflow u64. A configuration whose
grid end overflows is rejected at construction as a config error, so
every later comparison of a tick against the grid end is overflow-free
by construction and needs no per-call check.

A raw price p is valid when all of these hold: p is not zero, p is an
exact multiple of tick_size, and tick = p / tick_size satisfies
grid_start_tick <= tick < grid_start_tick + grid_len. Any failure is
InvalidPriceTick { price, tick_size, grid_start_tick, grid_len } with no
state mutation. The tick index stored inside the book is
(tick - grid_start_tick) as u32, a dense index into the level arrays and
the bitmaps.

### 3.3 Order types and STP policies

OrderType: Limit, Ioc (immediate or cancel), Fok (fill or kill), PostOnly.
Time in force is folded into the type; there is no separate TIF field.

StpPolicy: CancelIncoming, CancelResting, CancelBoth. The policy that
applies to a potential self-trade is the policy carried by the incoming
order, falling back to BookConfig.default_stp. Section 7.5 gives the
per-policy behaviour.

## 4. Arena: cache-line struct-of-arrays with safe handles

### 4.1 Layout

The arena is a fixed-capacity struct-of-arrays. Each column is a Vec
allocated once at construction at exactly max_orders entries. No column
ever grows. Column set:

| Column | Type | Meaning |
| --- | --- | --- |
| order_id | Vec<u64> | OrderId per slot, zero when free. |
| participant | Vec<u32> | ParticipantId per slot. |
| price_idx | Vec<u32> | Dense tick index per slot. |
| qty | Vec<u64> | Remaining quantity per slot. |
| side | Vec<u8> | 0 bid, 1 ask. |
| state | Vec<u8> | 0 free, 1 live. |
| prev | Vec<u32> | Previous slot in the price level list, or NIL. |
| next | Vec<u32> | Next slot in the price level list, or the free list link when the slot is free. |
| generation | Vec<u32> | Generation counter per slot, section 4.3. |

Resting orders at one price form an intrusive doubly linked list through
prev and next, with the head and tail kept in the level arrays
(section 6.1). Struct-of-arrays is chosen over an array of structs for
three concrete reasons: the match loop touches qty, next, and side for
every resting order and nothing else, so those columns stream through
cache without dragging the cold fields along; the snapshot writer walks
columns sequentially; and the debug invariant checker can scan one column
at a time. A 64-byte cache line holds 8 qty entries or 16 link entries.

### 4.2 Slot handles and the free list

SlotIdx is a u32 newtype. NIL is u32::MAX and never indexes a column.
Allocation pops the head of an intrusive free list threaded through the
next column of free slots. Release pushes the slot back onto the head of
the free list. Both operations are O(1) and branch-light.

Release zeroes the slot before linking it into the free list:
order_id, participant, price_idx, qty, and side are set to zero, prev
is set to NIL, and state is set to free. The one field that then
carries a value is next, which the release itself writes with the free
list link (the previous free head, or NIL when the free list was
empty). That link is list plumbing, never order data, so no stale
order data is observable in a free slot. This is INV-ARENA-2 stated in
the same terms. Zeroing is a
correctness measure, not hygiene: it makes stale data unobservable to the
invariant checker, makes snapshot bytes deterministic for free slots,
and turns use-after-release bugs in the engine into immediate assertion
failures in debug builds instead of silent reads of plausible old values.

### 4.3 Generation checks and the ABA problem

A slot index alone is not a safe handle. A slot can be released and
reused for a different order while some structure still holds the old
index; that is the stale-handle ABA failure. The defence is a generation
counter:

- generation starts at 1 for every slot at construction.
- The lookup table stores (slot, generation) pairs, captured at insert.
- Every internal dereference of a slot through the lookup table checks
  that arena.generation[slot] equals the stored generation and that
  arena.state[slot] is live. A mismatch is an internal invariant
  failure, asserted in debug builds and impossible by construction in
  the state machine; it is never a user-facing error.
- Release increments the generation with checked arithmetic. On the
  practically unreachable overflow of u32 generations for one slot
  (4,294,967,294 releases of the same slot), the slot is retired
  permanently: it is not returned to the free list and effective
  capacity drops by one. Retiring is specified behaviour, documented
  here, so the overflow path is a tested branch and not a wrap.

Raw SlotIdx values never leave chronicle-core. Public APIs name orders
only by OrderId and participants only by ParticipantId, so external
callers can never hold a stale handle at all.

## 5. Lookup table: OrderId to SlotIdx with Knuth Algorithm R deletion

### 5.1 Structure

The lookup table is a fixed-capacity, power-of-two, open-addressed hash
table built at construction:

- capacity = the smallest power of two that is at least 2 * max_orders,
  so the load factor never exceeds 50 percent. Insert at the cap returns
  CapacityFull before any mutation.
- Columns: keys Vec<u64> (OrderId, zero means empty), slots Vec<u32>,
  gens Vec<u32>. A parallel occupied count tracks len.
- There is no resize and no rehash after construction. Capacity is a
  configuration fact, which keeps every operation allocation free and
  worst-case bounded.

### 5.2 Hashing

hash(order_id) is the SplitMix64 finalizer applied to the OrderId:

    z = order_id + 0x9E37_79B9_7F4A_7C15
    z = (z ^ (z >> 30)) * 0xBF58_476D_1CE4_E5B9
    z = (z ^ (z >> 27)) * 0x94D0_49BB_1331_11EB
    result = z ^ (z >> 31)

The home bucket is result & (capacity - 1). Probing is linear, step 1.
Identity hashing is rejected: sequential OrderIds, the common case for
exchange gateways, would cluster into one probe run under an identity or
low-bit hash, and clustering is exactly what the 50 percent load cap plus
a full-avalanche mixer removes. SplitMix64 is deterministic, std free,
and identical on every platform, which the determinism contract needs.
std RandomState is forbidden here: it is seeded per process and would
make probe behaviour differ between runs.

### 5.3 Deletion: backward shift, zero tombstones

Deletion uses Knuth Algorithm R (TAOCP volume 3, section 6.4, Algorithm
R). Tombstones are forbidden: a tombstone chain degrades probe lengths
under cancel-heavy load, which is the normal load of an order book, and
it makes probe length depend on history, which weakens the latency
story. Backward-shift deletion keeps every cluster contiguous.

Algorithm R, as implemented in lookup.rs:

1. Clear the deleted bucket i. hole = i.
2. Scan j = hole + 1, wrapping, while bucket j is occupied.
3. Let home = hash(key at j) & mask. If home lies in the cyclic interval
   (hole, j], the entry at j must stay; advance j.
4. Otherwise move the entry from j into hole, clear j, and set hole = j.
5. Stop at the first empty bucket.

After deletion, every remaining key is still reachable from its home
bucket by linear scan without crossing an empty bucket. That reachability
statement is the property the lookup proptests assert directly
(INV-LOOKUP-2 in docs/INVARIANTS.md).

## 6. Price levels and hierarchical bitmaps

### 6.1 Level state

Per tick index, fixed arrays allocated at construction:

- level_head: Vec<u32>, first slot at the level or NIL.
- level_tail: Vec<u32>, last slot at the level or NIL.
- level_qty: Vec<u64>, exact sum of remaining quantities at the level.
- level_count: Vec<u32>, number of live orders at the level.

A level is active when level_count is non-zero. Cached level_qty is
maintained incrementally with checked arithmetic on every fill, insert,
cancel, and amend, and the invariant checker recomputes it from the arena
to prove the cache honest (INV-BOOK-3).

### 6.2 Bitmap hierarchy

Active levels are tracked by a hierarchy of u64 words over the dense
tick index space:

- Level 0: one bit per tick index, grid_len / 64 words.
- Level k+1: one bit per word of level k, set exactly when that child
  word is non-zero.
- The hierarchy ends at a single root word. Depth is computed at
  construction from grid_len (depth 3 for the 131,072 tick reference
  grid: 2,048 words, then 32 words, then 1 root word).

Synchronization rule, the only rule: after any change to a word at level
k, the parent bit is rewritten to (word != 0), and propagation continues
upward only while a parent word actually changed. The rule makes the
summary invariant (INV-BITMAP-1) a local, checkable fact rather than a
global hope, and it bounds update cost at one word write per level in
the common case where neighbouring levels stay active.

Two hierarchies are kept, one for bids and one for asks, over the same
tick space. Keeping them separate makes best bid and best ask independent
scans and lets a level hold bids and asks at different times without bit
contention. A tick index can be active on both sides only across time,
never at once in a resting book that satisfies best_bid < best_ask.

### 6.3 Best bid and best ask in O(1) word scans

Best ask: from the root word, take trailing_zeros to find the lowest set
child index, descend one level, repeat, until level 0 yields the lowest
active tick index. Best bid is the mirror image with leading_zeros for
the highest active tick index. Each descent step is one word read and
one bit scan instruction; total work is O(depth) with depth at most 4
for any permitted grid, independent of how many levels are active. An
empty side yields None from a zero root word in one read.

Insertion at a level tail, removal of any slot, and level activation and
deactivation are O(1). Traversal of one level is O(orders at the level)
through the intrusive list, which is also the exact FIFO order.

## 7. The matching engine

### 7.1 Command surface

The Book exposes one method per command. Each takes a caller-owned
EventBuffer (section 7.6) and returns Result<usize, BookError>, the
number of events written.

- add_order(order_id, participant, side, order_type, price, qty, stp, events)
- cancel_order(order_id, events)
- cancel_by_participant(participant, events)
- amend_order(order_id, new_price, new_qty, events)
- best_bid(), best_ask(): Option<(price, aggregate qty)>, no events.
- depth(side, max_levels, out: &mut [DepthLevel]) -> usize: L2 snapshot
  into a caller slice, section 7.7.
- debug_assert_invariants(): full structural check, debug builds and
  tests only, section 11.

### 7.2 Processing pipeline for add_order

Every mutating command runs the same three phases. Phases 1 and 2 never
mutate book state, which is what makes every rejection atomic.

Phase 1, validation. Reject, with the typed error and zero mutation of
book, bitmap, lookup, arena, or seq, on: zero or duplicate active
OrderId, zero quantity, invalid price tick, lookup table at its load
cap.

Phase 2, preflight. Compute, without mutating, everything the command
could do:

- For PostOnly: whether the price would cross the opposite best price.
- For Fok: total executable quantity against the incoming limit price,
  computed under the STP rules of section 7.5 (own resting orders that
  STP would cancel do not count as executable).
- For every order type: the worst-case event count of the match, found
  by a read-only walk of the levels and orders the sweep would touch.
  The bound is exact for Limit: one Trade event per resting order
  touched, one Filled event per resting order the sweep would fully
  fill, and one terminal event for the incoming order. If the
  EventBuffer has fewer free slots than the bound, return
  EventBufferFull before mutating.
- Capacity, for an order that would rest a remainder: a free slot must
  exist after the sweep, counting the slots the sweep itself frees.
  An order that fills completely needs no slot and is never rejected
  for capacity. (Milestone 1 sync: this check lives in preflight, not
  phase 1, because only the sweep walk knows whether a remainder
  rests.)

Phase 3, execution. Assign the seq, append nothing yet, run the match
loop of section 7.3, then emit the lifecycle events in the fixed order
of section 7.6. Execution cannot fail: every failure mode was excluded
in phases 1 and 2, so there is no partial mutation to roll back.

### 7.3 Match loop

The incoming (taker) order sweeps the opposite side:

1. While taker quantity remains and a best opposite price exists and
   that price crosses the taker limit price (buy: best ask <= price;
   sell: best bid >= price):
2. Take the head slot of that level, the FIFO maker.
3. Apply STP if maker.participant == taker.participant (section 7.5).
4. Trade quantity is min(taker remaining, maker remaining), computed in
   u64 with the notional cross-check in u128 (section 8.2). Emit Trade
   with the maker price as the trade price.
5. Decrement both quantities and the level aggregate with checked
   arithmetic. A maker reaching zero leaves the book: unlink from the
   level list, remove from the lookup, release the slot, update the
   bitmaps if the level emptied. A partially filled maker stays at the
   head with its reduced quantity and its priority intact.
6. A taker reaching zero stops. A Limit taker with quantity left rests
   the remainder at the tail of its own level; its residual keeps the
   arrival priority of the original command, which is simply its place
   at the tail at insertion time. An Ioc taker remainder is cancelled
   and reported as Expired. A Fok taker never reaches this step with a
   remainder, by its preflight. A PostOnly taker never crosses, by its
   preflight, and always rests in full or was rejected.

Price-time priority is therefore exact: levels are visited best price
first through the bitmaps, and orders inside a level are visited head to
tail through the intrusive list.

### 7.4 Amend rules

amend_order locates the order through the lookup table. A missing or
terminal OrderId returns OrderNotFound { order_id } with no side
effects. For a live order, after validating the new price tick and a
non-zero new quantity:

- Same price and same quantity: a priority-preserving no-op. The order
  keeps its exact list position. An Amended event is emitted and a seq
  is consumed, so the WAL stays a complete command record.
- Same price, smaller quantity: quantity is reduced in place, level_qty
  decreases by the difference, list position and priority are unchanged.
- Same price, larger quantity, or any price change: the order loses its
  priority. It is unlinked, its quantity and price are updated, and it
  is reinserted at the tail of the target level. Bitmap and level
  aggregates are updated for both levels when the price changes. An
  amend never triggers a match, even if the new price would cross: the
  amended order rests at its new price and the crossed-book invariant is
  stated for post-command state after add commands and for resting state
  maintained by construction. If an amend would create a crossed book
  (new bid price at or above the best ask, or the mirror), it is
  rejected with AmendWouldCross { order_id, new_price, opposite_best }
  and zero mutation. This is a deliberate tightening beyond the minimum
  contract, recorded here so the invariant best_bid < best_ask holds
  after every command, not only after adds.

### 7.5 Self-trade prevention

When the match loop reaches a maker whose participant equals the taker
participant, the incoming order STP policy decides:

- CancelIncoming: the taker stops immediately. Its remaining quantity
  is cancelled, a SelfTradePrevented event is emitted, and the resting
  order is untouched. Orders already traded earlier in the sweep stay
  traded; STP is not retroactive.
- CancelResting: the maker is cancelled (Cancelled event with reason
  SelfTradePrevention, slot released, aggregates updated) and the sweep
  continues to the next maker without consuming taker quantity.
- CancelBoth: the maker is cancelled as above and the taker remainder
  is cancelled as under CancelIncoming.

Fok preflight uses the same rules when it sums executable quantity:
under CancelIncoming, executable quantity stops accumulating at the
first own order in the sweep path; under CancelResting and CancelBoth,
own orders contribute zero executable quantity and the sweep continues
past them; under CancelBoth the incoming remainder dies at the first own
order, so quantity beyond that point is not executable either, and the
preflight models exactly that.

### 7.6 Events and the EventBuffer contract

ExecutionEvent variants: Accepted, Trade, Filled, Cancelled,
Amended, Expired, SelfTradePrevented. Every event carries the seq of its
command and the OrderIds involved; Trade also carries price, quantity,
maker OrderId, and taker OrderId. Events contain no SlotIdx, no
timestamps, and no pointers, so event streams are byte-identical across
processes and across snapshot restore (section 10.4).

Emission order inside one command is fixed: Trade and the maker-side
Filled or Cancelled events in sweep order first, then the taker
lifecycle event (Accepted if it rests, Filled, Expired, or
SelfTradePrevented), then the command-level outcome. Tests compare full
event streams, never samples.

EventBuffer is a caller-owned sink over a pre-allocated vector:
EventBuffer::new(&mut Vec<ExecutionEvent>) in the typed API. The
caller's vector carries spare capacity reserved ahead of time; the
buffer tracks how many events were written through it, and its
remaining capacity is the vector's spare capacity. The engine writes
only through this buffer, and preflight has already proved the spare
capacity covers the worst case, so a write never grows the vector.
If preflight finds the spare capacity too small, the command returns
EventBufferFull { needed, available } before mutating anything,
including seq. The caller can retry the identical command with a
larger buffer and get the identical result, which the property tests
assert (INV-EVENT-2). (Milestone 1 sync: the buffer borrows a Vec
rather than a raw slice, because a slice of events would force the
caller to pre-fill dummy events; the capacity contract is unchanged.)

### 7.7 L2 depth queries

depth walks the side bitmap from the best price inward, writing up to
max_levels DepthLevel { price, total_qty, order_count } entries into
the caller slice and returning the count written. It performs no
allocation, mutates nothing, and costs O(levels returned times bitmap
depth). It is a snapshot in the market-data sense only; it has no
relation to the durable snapshots of section 10.

### 7.8 cancel_by_participant

Mass cancel is a deliberate linear scan. The engine walks arena slots in
ascending SlotIdx order, and for every live slot owned by the
participant it unlinks the order, updates level aggregates and bitmaps,
removes the lookup entry, releases the slot, and emits one Cancelled
event per order, in ascending SlotIdx order. No auxiliary participant
index is kept: an index would need its own allocation story, its own
invariant set, and its own snapshot section, and mass cancel is a cold
operation where an O(max_orders) scan over flat columns is a few
memory-bandwidth-bound milliseconds at the largest permitted capacity.
The scan is zero-allocation and fully deterministic, including the event
order, which the snapshot format relies on (section 10.4).

## 8. Error model and checked math

### 8.1 BookError variants

All domain errors implement std::error::Error and Display, and every
variant carries the concrete values that explain the failure:

| Variant | Fields | Raised when |
| --- | --- | --- |
| InvalidOrderId | order_id | OrderId is zero. |
| DuplicateOrderId | order_id | An active order already uses the id. |
| InvalidQuantity | qty | Quantity is zero on add or amend. |
| InvalidPriceTick | price, tick_size, grid_start_tick, grid_len | Price fails section 3.2. |
| OrderNotFound | order_id | Cancel or amend names a missing or terminal id. |
| CapacityFull | max_orders | Arena has no free slot. |
| LookupCapacityFull | capacity, len | Load cap reached. Defensive; unreachable while the arena cap is enforced first, kept as a typed guard instead of a panic. |
| PostOnlyWouldCross | price, opposite_best | PostOnly add would cross. |
| FokInsufficientLiquidity | requested, available | Fok preflight sum is short. |
| AmendWouldCross | order_id, new_price, opposite_best | Section 7.4. |
| EventBufferFull | needed, available | Section 7.6 preflight. |
| ArithmeticOverflow | what | A checked aggregate operation overflowed. Defensive guard; aggregates are sized so this is unreachable in a valid book. |

No engine path panics on user input. Panics are reserved for internal
invariant failures in debug assertions, which indicate engine bugs, not
bad commands.

### 8.2 Checked math

Per-level and per-order quantities are u64 with checked_add and
checked_sub at every mutation. Cumulative traded volume and notional
(price times quantity) are accumulated in u128, which cannot overflow
for any permitted grid and quantity: the largest product is below
2^64 times 2^64 and the u128 accumulator is the specified widening.
Debug and release builds run the same checked arithmetic; there is no
release-only wrap path.

Ownership of those cumulative counters is settled here: they are
caller-side state, not Book state. The engine retains no running
totals, because every Trade event carries its price and quantity and
any total is a fold over the event stream. Callers that want running
volume or notional (CLI reports, benchmarks) accumulate in u128 on
their side of the API. That is why the counters appear in no snapshot
field inventory (section 10.2): there is nothing to persist, and
replay reproduces them exactly by re-accumulating over the reproduced
event stream.

## 9. Write-ahead log

chronicle-wal persists commands, not events. Recovery replays commands
through the same Book pipeline, so recovered state and event streams are
identical to the original run by construction, provided the engine is
deterministic, which section 11 fixes.

### 9.1 Frame layout

All integers are little-endian. A frame is a 32-byte header, a fixed
schema payload, and a CRC carried inside the header.

Header, 32 bytes:

| Offset | Width | Field | Value |
| --- | --- | --- | --- |
| 0 | 4 | magic | Bytes 0x43 0x57 0x41 0x4C, ASCII "CWAL". |
| 4 | 2 | version | u16, value 1. |
| 6 | 2 | header_len | u16, value 32 for version 1. |
| 8 | 8 | seq | u64, the command sequence number, strictly increasing by 1 across the log. |
| 16 | 4 | payload_len | u32, exact payload width for the kind; at most MAX_PAYLOAD_LEN = 4096 as a defensive cap. |
| 20 | 1 | kind | u8, section 9.2. |
| 21 | 1 | flags | u8, reserved, must be zero in version 1. |
| 22 | 2 | reserved | u16, must be zero in version 1. |
| 24 | 4 | reserved2 | u32, must be zero in version 1. Keeps the CRC at a fixed offset and leaves room for a future header extension under a new version. |
| 28 | 4 | crc32c | u32, CRC32C over the domain-separated input of section 9.3, covering header bytes 0..28 and the full payload. |

Total frame size is 32 + payload_len. Frames are concatenated with no
padding and no separators. A reader that loses framing has no resync
strategy by design: resync after corruption is how recovery tools invent
state, and this format chooses a typed error instead.

### 9.2 Payload schemas

Payloads are fixed width per kind. All fields little-endian. Reserved
fields must be zero on write and are rejected as corruption on read,
which keeps future use of those bytes a version change, not a silent
reinterpretation.

Kind 1, Add, 40 bytes:

| Offset | Width | Field |
| --- | --- | --- |
| 0 | 8 | order_id u64 |
| 8 | 4 | participant u32 |
| 12 | 1 | side u8 (0 bid, 1 ask) |
| 13 | 1 | order_type u8 (0 Limit, 1 Ioc, 2 Fok, 3 PostOnly) |
| 14 | 1 | stp u8 (0 CancelIncoming, 1 CancelResting, 2 CancelBoth, 255 means use config default) |
| 15 | 1 | reserved u8, zero |
| 16 | 8 | price u64 |
| 24 | 8 | qty u64 |
| 32 | 8 | reserved u64, zero |

Kind 2, Cancel, 16 bytes: order_id u64 at 0, reserved u64 zero at 8.

Kind 3, CancelByParticipant, 16 bytes: participant u32 at 0, reserved
12 bytes zero at 4.

Kind 4, Amend, 32 bytes: order_id u64 at 0, new_price u64 at 8,
new_qty u64 at 16, reserved u64 zero at 24.

No other kinds exist in version 1. An unknown kind byte is corruption
(CorruptWalRecord), not a skip: skipping a command would silently change
recovered state.

The seq in the header is authoritative. Payloads carry no seq copy, so
the two can never disagree.

### 9.3 CRC32C and domain separation

The checksum is CRC32C (Castagnoli polynomial 0x1EDC6F41, reflected
0x82F63B78), computed by the crc32fast crate in chronicle-wal only. The
CRC input is, in order: the ASCII domain prefix "chronicle-wal/v1"
followed by one zero byte, then header bytes 0..28, then the payload.
Domain separation means a snapshot CRC can never validate as a WAL CRC
and a version 1 CRC can never validate under a future version that
changes the prefix. CRC32C is an error-detecting code, not a MAC; the
format claims detection of torn and corrupted writes, not resistance to
an attacker who can rewrite a file and recompute its CRC. That threat
model note is part of the format, and INVARIANTS.md states it.

### 9.4 Writer and sync policy

WalWriter owns the file and one reused Vec<u8> encode buffer sized to
the largest frame. append(command, seq) encodes, writes with write_all,
and then follows the configured SyncPolicy:

- Always: sync_data after every frame. This is the default and the only
  policy under which the durability claim holds: a frame the caller was
  told is durable survives a process or OS crash up to the storage
  stack honouring sync.
- OnSnapshot: sync_data only when a snapshot is taken and on close.
  Faster, weaker, chosen explicitly by the operator.
- Never: no syncs. Tests and benchmarks only.

Group commit, background writer threads, and io_uring are out of scope
for version 1 (section 12).

### 9.5 Reader: crash boundary versus corruption

The reader scans frames from offset 0 and classifies every stop:

| Condition at the current offset | Verdict |
| --- | --- |
| Offset equals file length exactly | Clean end of log. |
| Fewer than 32 bytes remain, and every fixed-value byte present matches the value a valid header carries at that position | Clean crash boundary. The trailing fragment is a prefix of a valid header, cut off by a torn write. Discarded byte count is reported. Fixed-value positions are: magic at bytes 0..4, version at bytes 4..6 (value 1), header_len at bytes 6..8 (value 32), flags at byte 21 (value 0), and the reserved fields at bytes 22..28 (value 0). Variable positions (seq, payload_len, kind payload semantics, CRC) constrain nothing in a fragment, except that a kind byte present at byte 20 must be a known kind value. |
| Fewer than 32 bytes remain, and any fixed-value byte present mismatches, or at least 4 bytes remain and the magic does not match | CorruptWalRecord { offset }. A torn write cannot produce this: the writer emits the header in one write_all, so a crash leaves a prefix of a correct header, never wrong fixed bytes. A mismatch in any present fixed byte means foreign or overwritten data, at any fragment length, including fragments under 4 bytes (a wrong first byte is already decisive). |
| Magic matches but fewer than 32 + payload_len bytes remain | Clean crash boundary. Torn payload. |
| version is not 1, header_len is not 32, payload_len exceeds the cap, payload_len does not match the kind fixed width, kind is unknown, or a reserved field is non-zero | CorruptWalRecord { offset } with the specific field in the error message. |
| CRC mismatch on a complete frame | CorruptWalRecord { offset, expected_crc, actual_crc }. A complete frame with a bad CRC is corruption, never a crash boundary, because the writer only reports success after the full frame is written. |
| seq is not exactly previous seq + 1 among replayed frames | WalSequenceGap { expected, found, offset }. |

The single subtle case is a torn write that lands exactly on a frame
boundary plus a partial header whose first bytes were never flushed:
that is the fewer-than-32-bytes case, and it is clean only when every
fixed-value byte that did land matches a valid header prefix, per the
second row of the table. Garbage appended by something other than the
writer fails that same prefix test at whatever length it appears,
including a fragment too short to hold the full magic. This table is
the whole disambiguation policy and the fault oracle tests exercise
every row, including short fragments with single wrong bytes
(INV-WAL-2).

### 9.6 What the WAL does not store

Rejected commands are not written: they changed nothing, and replaying
the accepted stream reproduces the book exactly. The seq continuity rule
then reads: WAL seqs are exactly the accepted command seqs, consecutive
from the first frame after any snapshot point. Event output is not
stored; it is reproduced by replay.

## 10. Snapshots

### 10.1 Purpose and shape

A snapshot is a structural dump of the book at a seq boundary, not a
re-serialisation through the command API. It stores the arena columns
for live slots, the generations of all slots, the free list head and
every free slot link, the level arrays, the configuration, and
next_seq, so that restore reproduces the internal state exactly,
including slot assignment, FIFO list links, and the exact free list,
which fixes allocation order after restore. Section 10.4 explains why
nothing weaker is acceptable.

One deliberate exclusion: the cumulative traded volume and notional
counters of section 8.2 are caller-side accumulations over the event
stream, not Book state, so they are stored nowhere in this format and
restored nowhere. Replay reproduces them exactly by re-accumulating
over the reproduced event stream.

### 10.2 File layout

All integers little-endian.

Header, 64 bytes:

| Offset | Width | Field | Value |
| --- | --- | --- | --- |
| 0 | 4 | magic | Bytes 0x43 0x53 0x4E 0x50, ASCII "CSNP". |
| 4 | 2 | version | u16, value 1. |
| 6 | 2 | header_len | u16, value 64. |
| 8 | 8 | last_included_seq | u64, seq of the last command reflected in this snapshot. |
| 16 | 8 | next_seq | u64, must equal last_included_seq + 1. Stored explicitly so a mismatch is detectable corruption, not a recomputation. |
| 24 | 4 | live_order_count | u32. |
| 28 | 4 | max_orders | u32, arena capacity the snapshot was taken at. |
| 32 | 8 | tick_size | u64. |
| 40 | 8 | grid_start_tick | u64. |
| 48 | 4 | grid_len | u32. |
| 52 | 4 | default_stp | u8 value stored in a u32 slot for alignment, plus zero padding. |
| 56 | 4 | free_list_head | u32, SlotIdx of the first free slot in allocation order, or NIL (u32::MAX) when the arena is full. |
| 60 | 4 | reserved | u32, zero. |

Body, in fixed order:

1. Generations: max_orders u32 values, slot order, covering free and
   live slots alike. Free slot generations must persist, or a handle
   captured before the snapshot could validate after restore.
2. Free list links: max_orders u32 values, slot order. For a free
   slot, the value is the SlotIdx of the next free slot in allocation
   order, or NIL for the last free slot. For a live slot, the value
   must be NIL; anything else is CorruptSnapshot. Together with
   free_list_head in the header, this section persists the exact free
   list, so allocation order after restore is identical to allocation
   order at snapshot time. The live free list is LIFO (most recently
   released slot is allocated first, section 4.2); rebuilding it in
   any canonical order, such as ascending slot order, would change
   which slot the next add takes and is therefore not a valid restore.
3. Level arrays, in tick index order, each entry 20 bytes: head u32,
   tail u32, level_qty u64, level_count u32.
4. Live slot records, ascending SlotIdx, each 56 bytes: slot u32,
   generation u32, order_id u64, participant u32, price_idx u32,
   qty u64, side u8, state byte (must be live), 6 bytes zero padding,
   prev u32, next u32, reserved u64 zero. Width check:
   4 + 4 + 8 + 4 + 4 + 8 + 1 + 1 + 6 + 4 + 4 + 8 = 56. The record width
   is therefore fixed at 56 bytes; the field offsets inside the record
   are slot 0, generation 4, order_id 8, participant 16, price_idx 20,
   qty 24, side 32, state 33, padding 34..40, prev 40, next 44,
   reserved 48..56.

Trailer: crc32c u32 over the domain prefix "chronicle-snapshot/v1" plus
one zero byte, then every preceding byte of the file, header and body.

Total size is exactly 64 + 4 * max_orders (generations) +
4 * max_orders (free list links) + 20 * grid_len +
56 * live_order_count + 4, which is 68 + 8 * max_orders +
20 * grid_len + 56 * live_order_count. A size mismatch against the
header values is CorruptSnapshot before any state is built.

### 10.3 Atomic write protocol

Saving a snapshot performs, in order:

1. Encode into the writer reused buffer.
2. Write the bytes to a temporary file in the same directory as the
   target, named by appending ".tmp" to the target file name.
3. sync_all on the temporary file.
4. rename the temporary file over the target. Rename within one
   directory is atomic on the supported platforms.
5. Open the parent directory and sync_all on it, so the rename itself
   is durable.

A crash at any step leaves either the old snapshot fully intact or the
new one fully intact. A leftover ".tmp" file is ignored by the loader
and may be overwritten by the next save. Concurrent snapshot writers to
the same target are out of scope: the system has one writer, matching
the single-threaded engine.

### 10.4 Load, restore, and continuity

Loading validates in order: file size against the header arithmetic,
magic, version, header_len, next_seq equals last_included_seq + 1,
configuration compatibility with the BookConfig the caller supplied
(a snapshot restores into a book built with the identical config; any
field mismatch is SnapshotConfigMismatch naming both values), and
finally the trailing CRC. CRC failure is CorruptSnapshot { expected_crc,
actual_crc }. No partial state escapes a failed load: restore builds a
fresh Book and returns it only on full success.

Restore then: copies generations for all slots, restores the free
list verbatim (free_list_head from the header, each link from the free
list body section) and validates it before any allocation can use it:
the chain from the head must visit exactly the slots with no live
record, each exactly once, with no cycles, and every live slot link
field must be NIL, otherwise the load fails as CorruptSnapshot and no
state escapes. It then writes live records into the arena columns
including their prev and next links, restores level arrays verbatim,
rebuilds the lookup table by inserting live orders in ascending
SlotIdx order, recomputes the bitmaps from level_count, and asserts
the recomputed bitmaps equal the bitmaps derived from the level data,
then sets next_seq from the header.

Exactness matters for two concrete reasons, both about SlotIdx order.
First, cancel_by_participant emits events in ascending SlotIdx order
(section 7.8). A restore that renumbered slots or rebuilt FIFO order
from a traversal would produce a different event order for the first
mass cancel after recovery, and the snapshot plus WAL replay
equivalence property (INV-SNAP-3) compares full event streams byte for
byte. Second, allocation order is SlotIdx order over time. The live
free list is LIFO, so the next slot an add takes depends on release
history. A restore that rebuilt the free list in ascending slot order
would hand out different slots than the no-snapshot run from the first
post-restore add onward, every later SlotIdx would diverge, and a mass
cancel after those adds could emit a different event order even though
every order field matched. Persisting free_list_head and every free
slot link (section 10.2) removes that divergence. Structural dump plus
verbatim restore, free list included, is the design that makes
INV-SNAP-3 true by construction.

WAL continuity after a snapshot: replay skips frames with
seq <= last_included_seq, requires the first replayed frame to have
seq exactly last_included_seq + 1 (otherwise WalSequenceGap), and
requires strict +1 steps thereafter. After recovery,
next_seq = max_recovered_seq + 1, where max_recovered_seq is
last_included_seq when the WAL adds nothing.

## 11. Determinism rules

Binding on chronicle-core and on the recovery path of chronicle-wal:

- No SystemTime, Instant, or any clock read.
- No thread_rng, no OS entropy, no RandomState. All hashing uses the
  fixed SplitMix64 function of section 5.2.
- No iteration over HashMap or HashSet. The design needs neither; where
  an associative structure is required it is the lookup table of
  section 5 or a BTreeMap in the test-only reference book.
- Floating point does not appear in the core at all. Prices, quantities,
  notionals, and aggregates are integers.
- Simulation and property runners take --seed <u64> or SIM_SEED and
  print the active seed on every run. The same seed must reproduce
  byte-identical event traces across processes; CI checks this by
  running the CLI replay twice and comparing hashes in Milestone 4.

debug_assert_invariants() is the structural self-check used by every
property test after every command: crossed book check, per-level qty and
count recomputation from the arena, bitmap summary recomputation,
lookup reachability for every live order, generation agreement between
lookup and arena, free list contains exactly the free slots with no
cycles, and level list links are symmetric (next of prev is self).

## 12. Rejected alternatives

Each rejection records the concrete reason, as the milestone audit
requires. These are decisions, not open questions.

1. std HashMap for the OrderId index. Rejected on three grounds:
   RandomState makes probe behaviour process-dependent, breaking the
   determinism contract; iteration order is unspecified, and any future
   feature that iterates it inherits nondeterminism; and its growth
   policy allocates on the hot path, breaking zero steady-state
   allocation. The fixed open-addressed table of section 5 has none of
   these and a hard load cap.
2. BTreeMap or a sorted Vec of price levels for best price discovery.
   Rejected on complexity and cache grounds: best price becomes
   O(log levels) with pointer chasing through tree nodes scattered in
   memory, or O(levels) for a scan. The bitmap hierarchy answers in
   O(depth <= 4) word reads over a few cache lines and updates in O(1).
   A BTreeMap also allocates per inserted level.
3. Tombstone deletion in the lookup table. Rejected because probe
   lengths then depend on delete history, so cancel-heavy load, the
   normal case, degrades lookup latency without bound and makes latency
   a function of the past. Algorithm R keeps clusters contiguous and
   probe length a function of load factor only.
4. Box per order, or an Rc linked list per level. Rejected because it
   allocates on every insert and frees on every fill, which is the
   steady-state allocation the program exists to avoid, and because
   pointer chasing across heap objects destroys the cache behaviour
   section 4 is built for. The arena holds every order in flat columns
   allocated once.
5. Text or JSON WAL and snapshot formats. Rejected on integrity and
   cost grounds: variable width fields make torn-write disambiguation
   ambiguous (a truncated JSON line and a corrupt one are hard to tell
   apart at the byte level), parsing allocates, and numbers lose exact
   integer width guarantees. The fixed binary formats make every offset
   computable and every failure class decidable from the header alone.
6. Snapshot by serialising live orders through the command API
   (replay-to-rebuild on load). Rejected because it renumbers slots and
   can reorder the arena, which changes cancel_by_participant event
   order after recovery and breaks byte-identical replay equivalence.
   The structural dump of section 10 restores the exact state.
7. An internal staging Vec for events inside the Book (grow as needed,
   drain to the caller). Rejected because growth is allocation on the
   hot path and because it hides backpressure. The caller-owned buffer
   plus the preflight worst-case bound makes capacity an explicit,
   testable contract (EventBufferFull) instead of an OOM surprise.
8. A per-participant order index for cancel_by_participant. Rejected
   for now: a second associative structure doubles the invariant and
   snapshot surface for a cold operation. The linear arena scan is
   zero-allocation, deterministic in event order, and fast in absolute
   terms at the permitted capacities. Revisiting this needs a measured
   mass-cancel latency problem, not a preference.

## 13. Verification strategy

docs/INVARIANTS.md is the normative mapping. Summary of the machinery:

- Property tests (proptest, dev-dependency of chronicle-core) drive the
  engine and the cfg(test) reference book with one generated command
  stream and compare best bid and ask, full L2 depth, and the complete
  event stream after every command. PROPTEST_CASES scales volume:
  1,000 on PRs, 10,000 on main (DECISIONS.md Q6).
- debug_assert_invariants() runs after every generated command in every
  property test, so structural invariants are checked millions of times,
  not sampled.
- Fault oracles for the WAL: a harness that takes a valid log and
  truncates it at every byte offset, flips bytes at chosen offsets, and
  splices sequence gaps, asserting the section 9.5 verdict table exactly.
- Tamper tests for snapshots: flip each header field and sample body and
  trailer bytes, asserting CorruptSnapshot or SnapshotConfigMismatch,
  never a panic and never a successful load of altered state.
- Allocation proof: a thread-local counting GlobalAlloc in a dedicated
  integration test target runs 1,000,000 mixed operations at full
  capacity, including multi-level sweeps, mass cancels, and every
  rejection path, and asserts exactly zero allocations in the steady
  state window. Thread-local counting is what makes the measurement
  immune to parallel test threads.
- Benchmarks (criterion, benches only): add and cancel latency,
  match sweep latency, depth query cost, WAL append and recovery
  throughput, each reporting p50, p95, p99, and p99.9 where criterion
  supports it, with Throughput::Elements or Throughput::Bytes set per
  benchmark and inputs and outputs wrapped in black_box. CI runs
  cargo bench --workspace -- --test on every PR so benchmark code can
  never rot uncompiled.
- Golden vectors: frame.rs encode and decode are pinned against
  hand-computed byte strings in unit tests, so a format change is a
  visible test change, never an accident.

## 14. CI gates

.github/workflows/ci.yml implements: rustfmt check, clippy over the
whole workspace with the lint posture of section 2.4, cargo test at PR
volume, the 10,000 case seed sweep on main, rustdoc with
-D warnings, the bench compile check, and the zero-dependency check on
chronicle-core. At Milestone 0 the manifest does not exist yet and each
cargo job reports that plainly and passes; from Milestone 1 the guards
are inert and the gates run at full strength. The merge verdict for this
program is hosted CI green on the exact head plus the milestone gate
sign-off from Main Muse.

## 15. Milestone map

| Milestone | Delivers | Architecture sections |
| --- | --- | --- |
| M0 | This document, INVARIANTS.md, DECISIONS.md, toolchain pin, CI seed. No Rust sources. | All |
| M1 | Arena, lookup table, bitmaps, Limit matching, L2 depth, debug_assert_invariants. | 4, 5, 6, 7 (Limit path), 11 |
| M2 | Ioc, Fok, PostOnly, amend rules, cancel_by_participant, STP, EventBufferFull preflight, reference-book proptests. | 7, 8 |
| M3 | WAL writer and reader, crash versus corruption oracle suite, snapshots, recovery equivalence. | 9, 10 |
| M4 | Allocation proof at 1,000,000 operations, criterion benchmarks, chronicle-cli, README and CONTRIBUTING. | 13, 2.3 |
