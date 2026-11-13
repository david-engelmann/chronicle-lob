//! Hierarchical active-price bitmaps (ARCHITECTURE.md section 6.2).
//!
//! Level 0 holds one bit per tick index. Each higher level holds one
//! bit per word of the level below, set exactly when that child word is
//! non-zero. Updates rewrite the parent bit to `(child word != 0)` and
//! propagate only while a parent word actually changed, so the summary
//! invariant (INV-BITMAP-1) is a local fact, and best price discovery
//! descends the hierarchy in O(depth) word reads.

/// One side's active-level bitmap over the dense tick index space.
pub struct Bitmap {
    /// `levels[0]` is the leaf level; the last level has exactly one
    /// word. All levels are allocated once at construction.
    levels: Vec<Vec<u64>>,
    /// Number of tick indices covered (the book grid length).
    grid_len: u32,
}

impl Bitmap {
    /// Build the hierarchy for `grid_len` tick indices. `grid_len` is a
    /// validated multiple of 64.
    pub fn new(grid_len: u32) -> Self {
        let mut levels = Vec::new();
        let mut words = (grid_len / 64) as usize;
        loop {
            levels.push(vec![0u64; words]);
            if words == 1 {
                break;
            }
            words = words.div_ceil(64);
        }
        Self { levels, grid_len }
    }

    /// Number of hierarchy levels, root included.
    pub const fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Read one word, for the invariant checker and tests.
    pub fn word(&self, level: usize, word_idx: usize) -> u64 {
        self.levels[level][word_idx]
    }

    /// Number of words at one level, for the invariant checker.
    pub fn words_at(&self, level: usize) -> usize {
        self.levels[level].len()
    }

    /// True when no tick index is active.
    pub fn is_empty(&self) -> bool {
        self.levels[self.levels.len() - 1][0] == 0
    }

    /// True when the tick index is active at the leaf level.
    pub fn contains(&self, idx: u32) -> bool {
        let word = self.levels[0][(idx / 64) as usize];
        (word >> (idx % 64)) & 1 == 1
    }

    /// Mark a tick index active.
    pub fn set(&mut self, idx: u32) {
        self.update(idx, true);
    }

    /// Mark a tick index inactive.
    pub fn clear(&mut self, idx: u32) {
        self.update(idx, false);
    }

    /// Shared update path: rewrite the leaf bit, then rewrite each
    /// parent bit to `(child word != 0)`, propagating only while the
    /// parent word actually changed (the section 6.2 sync rule). The
    /// parent bit is never blindly set or cleared: a word that still
    /// has other bits keeps its parent bit set.
    fn update(&mut self, idx: u32, active: bool) {
        let mut word_idx = (idx / 64) as usize;
        let bit = idx % 64;
        let before = self.levels[0][word_idx];
        let after = if active {
            before | (1u64 << bit)
        } else {
            before & !(1u64 << bit)
        };
        if after == before {
            return; // leaf unchanged: every ancestor already agrees
        }
        self.levels[0][word_idx] = after;
        let mut child_nonzero = after != 0;
        for level in 1..self.levels.len() {
            let parent_bit = u32::try_from(word_idx % 64).unwrap_or(0);
            word_idx /= 64;
            let before = self.levels[level][word_idx];
            let after = if child_nonzero {
                before | (1u64 << parent_bit)
            } else {
                before & !(1u64 << parent_bit)
            };
            if after == before {
                return; // ancestors already reflect this word
            }
            self.levels[level][word_idx] = after;
            child_nonzero = after != 0;
        }
    }

    /// The highest active tick index, or `None` when empty.
    pub fn highest(&self) -> Option<u32> {
        self.descend(true)
    }

    /// The lowest active tick index, or `None` when empty.
    pub fn lowest(&self) -> Option<u32> {
        self.descend(false)
    }

    /// Descend the hierarchy from the root, choosing the highest set
    /// bit (for best bid) or the lowest set bit (for best ask) at each
    /// level.
    fn descend(&self, want_highest: bool) -> Option<u32> {
        let mut word_idx = 0usize;
        for level in (0..self.levels.len()).rev() {
            let word = self.levels[level][word_idx];
            if word == 0 {
                return None;
            }
            let bit = if want_highest {
                word.ilog2()
            } else {
                word.trailing_zeros()
            };
            if level == 0 {
                let idx = u32::try_from(word_idx * 64 + bit as usize).unwrap_or(u32::MAX);
                return if idx < self.grid_len { Some(idx) } else { None };
            }
            word_idx = word_idx * 64 + bit as usize;
        }
        None
    }

    /// The highest active tick index strictly below `exclusive`, by
    /// leaf-word scan. Used by L2 depth iteration.
    pub fn highest_below(&self, exclusive: u32) -> Option<u32> {
        if exclusive == 0 {
            return None;
        }
        let leaf = &self.levels[0];
        let mut word_idx = (exclusive / 64) as usize;
        let offset = exclusive % 64;
        // Bits at or above the offset are out of range; with offset 0
        // the whole starting word is out of range and the scan simply
        // steps down from the word below it.
        let mut candidate = if offset == 0 {
            0
        } else {
            leaf[word_idx] & ((1u64 << offset) - 1)
        };
        loop {
            if candidate != 0 {
                let idx = word_idx * 64 + candidate.ilog2() as usize;
                return Some(u32::try_from(idx).unwrap_or(u32::MAX));
            }
            if word_idx == 0 {
                return None;
            }
            word_idx -= 1;
            candidate = leaf[word_idx];
        }
    }

    /// The lowest active tick index strictly above `exclusive`, by
    /// leaf-word scan. Used by L2 depth iteration.
    pub fn lowest_above(&self, exclusive: u32) -> Option<u32> {
        let leaf = &self.levels[0];
        let mut word_idx = (exclusive / 64) as usize;
        let offset = exclusive % 64;
        // Bits at or below the offset are out of range; with offset 63
        // the whole starting word is out of range and the scan simply
        // steps up from the word above it.
        let mut candidate = if offset == 63 {
            0
        } else {
            leaf[word_idx] & (!0u64 << (offset + 1))
        };
        loop {
            if candidate != 0 {
                let idx = word_idx * 64 + candidate.trailing_zeros() as usize;
                return Some(u32::try_from(idx).unwrap_or(u32::MAX));
            }
            word_idx += 1;
            if word_idx >= leaf.len() {
                return None;
            }
            candidate = leaf[word_idx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent summary check: every parent bit equals (child != 0).
    fn assert_summary(bitmap: &Bitmap) {
        for level in 1..bitmap.depth() {
            for w in 0..bitmap.words_at(level) {
                let word = bitmap.word(level, w);
                for bit in 0..64usize {
                    let child_idx = w * 64 + bit;
                    if child_idx >= bitmap.words_at(level - 1) {
                        assert_eq!((word >> bit) & 1, 0, "padding bit set");
                    } else {
                        let expect = u64::from(bitmap.word(level - 1, child_idx) != 0);
                        assert_eq!(
                            (word >> bit) & 1,
                            expect,
                            "summary mismatch at level {level} word {w} bit {bit}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn bitmap_empty_has_no_best() {
        let bitmap = Bitmap::new(4096);
        assert!(bitmap.is_empty());
        assert_eq!(bitmap.highest(), None);
        assert_eq!(bitmap.lowest(), None);
        assert_summary(&bitmap);
    }

    #[test]
    fn bitmap_best_tracks_sets_and_clears() {
        let mut bitmap = Bitmap::new(4096);
        bitmap.set(10);
        bitmap.set(4000);
        bitmap.set(64);
        assert_eq!(bitmap.highest(), Some(4000));
        assert_eq!(bitmap.lowest(), Some(10));
        assert_summary(&bitmap);
        bitmap.clear(4000);
        assert_eq!(bitmap.highest(), Some(64));
        assert_summary(&bitmap);
        bitmap.clear(10);
        bitmap.clear(64);
        assert!(bitmap.is_empty());
        assert_summary(&bitmap);
    }

    #[test]
    fn bitmap_clearing_one_bit_keeps_sibling_bits() {
        // Regression: the parent bit must be rewritten to
        // (child word != 0), never blindly cleared. Two indices in one
        // leaf word; clearing one must keep the other discoverable.
        let mut bitmap = Bitmap::new(4096);
        bitmap.set(99);
        bitmap.set(100);
        bitmap.clear(99);
        assert_eq!(bitmap.highest(), Some(100));
        assert_eq!(bitmap.lowest(), Some(100));
        assert!(bitmap.contains(100));
        assert!(!bitmap.contains(99));
        assert_summary(&bitmap);
    }

    #[test]
    fn bitmap_summary_sync_on_last_order_removal() {
        // The engine-level name for this edge is
        // bitmap_summary_sync_on_last_order_removal: clearing the final
        // active index must propagate zeros all the way to the root.
        let mut bitmap = Bitmap::new(131_072);
        bitmap.set(65_537);
        assert!(!bitmap.is_empty());
        bitmap.clear(65_537);
        for level in 0..bitmap.depth() {
            for w in 0..bitmap.words_at(level) {
                assert_eq!(
                    bitmap.word(level, w),
                    0,
                    "level {level} word {w} not zeroed"
                );
            }
        }
    }

    #[test]
    fn bitmap_neighbours_scan_leaf_words() {
        let mut bitmap = Bitmap::new(512);
        for idx in [3u32, 70, 129, 500] {
            bitmap.set(idx);
        }
        assert_eq!(bitmap.highest_below(500), Some(129));
        assert_eq!(bitmap.highest_below(129), Some(70));
        assert_eq!(bitmap.highest_below(3), None);
        assert_eq!(bitmap.lowest_above(3), Some(70));
        assert_eq!(bitmap.lowest_above(129), Some(500));
        assert_eq!(bitmap.lowest_above(500), None);
        assert_eq!(bitmap.lowest_above(0), Some(3));
    }
}
