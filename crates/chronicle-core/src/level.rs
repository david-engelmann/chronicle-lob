//! Per-price-level aggregate state (ARCHITECTURE.md section 6.1).
//!
//! One entry per tick index, shared by both sides: a tick can hold
//! resting orders of only one side at a time, because orders of the
//! other side at the same tick would have matched them (the book is
//! never crossed, INV-BOOK-1). Which side is active at a tick is read
//! from the side bitmaps, not stored here.

use crate::arena::NIL;

/// Fixed arrays over the tick grid, allocated once at construction.
pub struct Levels {
    /// First slot at the level, or NIL.
    pub head: Vec<u32>,
    /// Last slot at the level, or NIL.
    pub tail: Vec<u32>,
    /// Exact sum of remaining quantities at the level.
    pub qty: Vec<u64>,
    /// Number of live orders at the level.
    pub count: Vec<u32>,
}

impl Levels {
    /// Allocate the arrays for `grid_len` tick indices.
    pub fn new(grid_len: u32) -> Self {
        let n = grid_len as usize;
        Self {
            head: vec![NIL; n],
            tail: vec![NIL; n],
            qty: vec![0; n],
            count: vec![0; n],
        }
    }
}
