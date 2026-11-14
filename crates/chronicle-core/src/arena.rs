//! Fixed-capacity struct-of-arrays order arena with generation-checked
//! slot handles (ARCHITECTURE.md section 4).
//!
//! Columns are allocated once at construction and never grow. A slot
//! index alone is never trusted: the lookup table stores the generation
//! it captured at insert, and [`Arena::validate_handle`] requires the
//! generation and the live state to still agree, which is what makes a
//! released and reused slot reject its previous handle (the ABA case).

/// Slot link sentinel: no slot.
pub const NIL: u32 = u32::MAX;

/// Arena column value for a free slot.
pub const STATE_FREE: u8 = 0;
/// Arena column value for a live slot.
pub const STATE_LIVE: u8 = 1;
/// Arena column value for a slot retired by generation overflow. A
/// retired slot never returns to the free list (section 4.3).
pub const STATE_RETIRED: u8 = 2;

/// The order arena. All fields are crate-visible so the invariant
/// checker and the module tests can inspect columns directly; nothing
/// here is part of the public API.
pub struct Arena {
    /// Order id per slot, zero when free.
    pub order_id: Vec<u64>,
    /// Participant id per slot.
    pub participant: Vec<u32>,
    /// Dense tick index per slot.
    pub price_idx: Vec<u32>,
    /// Remaining quantity per slot.
    pub qty: Vec<u64>,
    /// Side encoding per slot (see `Side::as_u8`).
    pub side: Vec<u8>,
    /// State per slot: free, live, or retired.
    pub state: Vec<u8>,
    /// Previous slot in the price level list, or NIL.
    pub prev: Vec<u32>,
    /// Next slot in the price level list when live; the free list link
    /// when free.
    pub next: Vec<u32>,
    /// Generation counter per slot, starting at 1.
    pub generation: Vec<u32>,
    /// Head of the intrusive free list, or NIL when the arena is full.
    free_head: u32,
    /// Number of slots currently on the free list.
    free_len: u32,
    /// Number of live slots.
    live: u32,
    /// Total slots, fixed at construction.
    capacity: u32,
}

impl Arena {
    /// Allocate every column at exactly `capacity` entries and thread
    /// the free list through all slots in ascending order, so the first
    /// allocations take slots 0, 1, 2, and so on.
    pub fn new(capacity: u32) -> Self {
        let n = capacity as usize;
        let mut next = vec![NIL; n];
        for (i, link) in next.iter_mut().enumerate() {
            if i + 1 < n {
                *link = u32::try_from(i + 1).unwrap_or(NIL);
            }
        }
        Self {
            order_id: vec![0; n],
            participant: vec![0; n],
            price_idx: vec![0; n],
            qty: vec![0; n],
            side: vec![0; n],
            state: vec![STATE_FREE; n],
            prev: vec![NIL; n],
            next,
            generation: vec![1; n],
            free_head: if capacity == 0 { NIL } else { 0 },
            free_len: capacity,
            live: 0,
            capacity,
        }
    }

    /// Total slots fixed at construction.
    pub const fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Slots currently live.
    pub const fn live_count(&self) -> u32 {
        self.live
    }

    /// Slots currently on the free list.
    pub const fn free_count(&self) -> u32 {
        self.free_len
    }

    /// Head of the free list, or NIL. Exposed for the snapshot writer
    /// (Milestone 3) and the invariant checker.
    pub const fn free_head(&self) -> u32 {
        self.free_head
    }

    /// Overwrite the free list head and the live and free counters in
    /// one step, during snapshot restore (ARCHITECTURE.md section
    /// 10.4). The caller has already written the columns and the per
    /// slot states and links to match the persisted snapshot exactly;
    /// this setter exists because the three counters are private, and
    /// it keeps them consistent with each other by construction.
    pub const fn restore_bookkeeping(&mut self, free_head: u32, free_len: u32, live: u32) {
        self.free_head = free_head;
        self.free_len = free_len;
        self.live = live;
    }

    /// True when `slot` currently holds a live order.
    pub fn is_live(&self, slot: u32) -> bool {
        self.state[slot as usize] == STATE_LIVE
    }

    /// True when `(slot, generation)` names the live order the handle
    /// was captured for: the slot is live and its generation has not
    /// moved since.
    pub fn validate_handle(&self, slot: u32, generation: u32) -> bool {
        self.is_live(slot) && self.generation[slot as usize] == generation
    }

    /// Take the head slot off the free list and mark it live.
    /// Returns the slot and its current generation, or `None` when the
    /// arena is full.
    pub fn alloc(&mut self) -> Option<(u32, u32)> {
        if self.free_head == NIL {
            return None;
        }
        let slot = self.free_head;
        let s = slot as usize;
        self.free_head = self.next[s];
        self.next[s] = NIL;
        self.prev[s] = NIL;
        self.state[s] = STATE_LIVE;
        self.free_len -= 1;
        self.live += 1;
        Some((slot, self.generation[s]))
    }

    /// Release a live slot: zero its order data, bump its generation
    /// (retiring the slot instead on generation overflow), and push it
    /// onto the head of the free list.
    pub fn release(&mut self, slot: u32) {
        let s = slot as usize;
        debug_assert_eq!(self.state[s], STATE_LIVE, "release of a non-live slot");
        // Zero the order data first (INV-ARENA-2). The only field that
        // carries a value afterwards is `next`, written below with the
        // free list link, which is list plumbing, never order data.
        self.order_id[s] = 0;
        self.participant[s] = 0;
        self.price_idx[s] = 0;
        self.qty[s] = 0;
        self.side[s] = 0;
        self.prev[s] = NIL;
        self.live -= 1;
        if self.generation[s] == u32::MAX {
            // Generation overflow: retire the slot permanently instead
            // of wrapping a handle back to a used generation.
            self.state[s] = STATE_RETIRED;
            self.next[s] = NIL;
        } else {
            self.generation[s] += 1;
            self.state[s] = STATE_FREE;
            self.next[s] = self.free_head;
            self.free_head = slot;
            self.free_len += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk_free_list(arena: &Arena) -> Vec<u32> {
        let mut seen = Vec::new();
        let mut cur = arena.free_head();
        while cur != NIL {
            assert!(!seen.contains(&cur), "free list cycle at slot {cur}");
            seen.push(cur);
            cur = arena.next[cur as usize];
        }
        seen
    }

    #[test]
    fn construction_allocates_fixed_capacity_once() {
        let arena = Arena::new(64);
        assert_eq!(arena.order_id.len(), 64);
        assert_eq!(arena.participant.len(), 64);
        assert_eq!(arena.price_idx.len(), 64);
        assert_eq!(arena.qty.len(), 64);
        assert_eq!(arena.side.len(), 64);
        assert_eq!(arena.state.len(), 64);
        assert_eq!(arena.prev.len(), 64);
        assert_eq!(arena.next.len(), 64);
        assert_eq!(arena.generation.len(), 64);
        assert_eq!(arena.capacity(), 64);
        assert_eq!(arena.live_count(), 0);
        assert_eq!(arena.free_count(), 64);
        let free = walk_free_list(&arena);
        assert_eq!(free.len(), 64);
        assert_eq!(free[0], 0);
        assert_eq!(free[63], 63);
    }

    #[test]
    fn arena_free_list_exact_after_churn() {
        let mut arena = Arena::new(8);
        for _ in 0..8 {
            arena.alloc().expect("slot");
        }
        assert_eq!(arena.free_count(), 0);
        assert!(arena.alloc().is_none());
        // Release in a scrambled order; the free list must contain
        // exactly the released slots, most recent first (LIFO).
        for slot in [3u32, 0, 7, 1] {
            arena.release(slot);
        }
        assert_eq!(walk_free_list(&arena), vec![1, 7, 0, 3]);
        assert_eq!(arena.free_count(), 4);
        assert_eq!(arena.live_count(), 4);
        for slot in [2u32, 4, 5, 6] {
            arena.release(slot);
        }
        assert_eq!(arena.free_count(), 8);
        assert_eq!(arena.live_count(), 0);
        assert_eq!(walk_free_list(&arena).len(), 8);
    }

    #[test]
    fn arena_release_zeroes_slot() {
        let mut arena = Arena::new(4);
        let (slot, _) = arena.alloc().expect("slot");
        let s = slot as usize;
        arena.order_id[s] = 99;
        arena.participant[s] = 7;
        arena.price_idx[s] = 12;
        arena.qty[s] = 50;
        arena.side[s] = 1;
        arena.prev[s] = 2;
        arena.release(slot);
        assert_eq!(arena.order_id[s], 0);
        assert_eq!(arena.participant[s], 0);
        assert_eq!(arena.price_idx[s], 0);
        assert_eq!(arena.qty[s], 0);
        assert_eq!(arena.side[s], 0);
        assert_eq!(arena.prev[s], NIL);
        assert_eq!(arena.state[s], STATE_FREE);
        // next carries only the free list link (here NIL: the rest of
        // the list was already allocated away is false, so check the
        // link points at the previous free head, slot 1).
        assert_eq!(arena.next[s], 1);
    }

    #[test]
    fn arena_generation_rejects_stale_handle() {
        let mut arena = Arena::new(2);
        let (slot, generation) = arena.alloc().expect("slot");
        assert!(arena.validate_handle(slot, generation));
        arena.release(slot);
        assert!(
            !arena.validate_handle(slot, generation),
            "released slot must not validate"
        );
        let (slot2, gen2) = arena.alloc().expect("slot");
        assert_eq!(slot2, slot, "LIFO free list returns the same slot");
        assert_ne!(gen2, generation);
        assert!(
            !arena.validate_handle(slot, generation),
            "stale generation must not validate"
        );
        assert!(arena.validate_handle(slot2, gen2));
    }

    #[test]
    fn arena_generation_overflow_retires_slot() {
        let mut arena = Arena::new(1);
        let (slot, _) = arena.alloc().expect("slot");
        arena.generation[slot as usize] = u32::MAX - 1;
        arena.release(slot); // generation becomes u32::MAX, slot returns
        let (slot_again, generation) = arena.alloc().expect("slot returns once");
        assert_eq!(slot_again, slot);
        assert_eq!(generation, u32::MAX);
        arena.release(slot); // overflow: retire instead of wrapping
        assert_eq!(arena.state[slot as usize], STATE_RETIRED);
        assert_eq!(arena.free_count(), 0);
        assert!(
            arena.alloc().is_none(),
            "retired slot is never handed out again"
        );
    }
}
