//! Open-addressed `OrderId` to slot lookup table (ARCHITECTURE.md
//! section 5). Fixed power-of-two capacity, load factor at most 50
//! percent, `SplitMix64` finalizer hashing, linear probing, and Knuth
//! Algorithm R backward-shift deletion: zero tombstones, so probe
//! length is a function of load factor only, never of delete history.

use crate::arena::NIL;
use crate::error::BookError;

/// `SplitMix64` finalizer. Deterministic on every platform and in every
/// process; identity hashing is rejected because sequential order ids
/// would cluster into one probe run (section 5.2).
pub const fn hash(order_id: u64) -> u64 {
    let mut z = order_id.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The lookup table. Columns are allocated once; the table never
/// resizes, so `insert` can refuse at the load cap instead of growing.
pub struct Lookup {
    /// Order ids; zero means the bucket is empty (order id zero is
    /// rejected at the book boundary, so the sentinel is unambiguous).
    keys: Vec<u64>,
    /// Slot per bucket.
    slots: Vec<u32>,
    /// Generation per bucket, captured at insert.
    gens: Vec<u32>,
    /// capacity - 1, for bucket arithmetic.
    mask: usize,
    /// Entries present.
    len: usize,
}

impl Lookup {
    /// Capacity is the smallest power of two at least `2 * max_orders`,
    /// which is what caps the load factor at 50 percent.
    pub fn new(max_orders: u32) -> Self {
        let want = (max_orders as usize).saturating_mul(2).max(2);
        let capacity = want.next_power_of_two();
        Self {
            keys: vec![0; capacity],
            slots: vec![NIL; capacity],
            gens: vec![0; capacity],
            mask: capacity - 1,
            len: 0,
        }
    }

    /// Buckets allocated.
    pub const fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Entries present.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Entries the load cap allows: capacity / 2.
    pub const fn load_cap(&self) -> usize {
        self.capacity() / 2
    }

    /// Home bucket of an order id: the masked hash, converted without
    /// a truncating cast (the mask keeps it inside `usize` range).
    fn home_bucket(&self, order_id: u64) -> usize {
        let mask_u64 = u64::try_from(self.mask).unwrap_or(u64::MAX);
        usize::try_from(hash(order_id) & mask_u64).unwrap_or(0)
    }

    /// Find the bucket holding `order_id`, if any, by linear scan from
    /// its home bucket. Scans stop at the first empty bucket, which is
    /// exactly the reachability property backward-shift deletion
    /// preserves (INV-LOOKUP-2).
    fn find_bucket(&self, order_id: u64) -> Option<usize> {
        let mut bucket = self.home_bucket(order_id);
        loop {
            let key = self.keys[bucket];
            if key == 0 {
                return None;
            }
            if key == order_id {
                return Some(bucket);
            }
            bucket = (bucket + 1) & self.mask;
        }
    }

    /// Look up an order id, returning its slot and generation.
    pub fn get(&self, order_id: u64) -> Option<(u32, u32)> {
        let bucket = self.find_bucket(order_id)?;
        Some((self.slots[bucket], self.gens[bucket]))
    }

    /// Insert a new order id. Callers have already rejected duplicates.
    ///
    /// # Errors
    ///
    /// Returns [`BookError::LookupCapacityFull`] when the load cap is
    /// reached, before any bucket changes.
    pub fn insert(&mut self, order_id: u64, slot: u32, generation: u32) -> Result<(), BookError> {
        if self.len >= self.load_cap() {
            return Err(BookError::LookupCapacityFull {
                capacity: self.capacity(),
                len: self.len,
            });
        }
        debug_assert!(self.find_bucket(order_id).is_none(), "duplicate insert");
        let mut bucket = self.home_bucket(order_id);
        while self.keys[bucket] != 0 {
            bucket = (bucket + 1) & self.mask;
        }
        self.keys[bucket] = order_id;
        self.slots[bucket] = slot;
        self.gens[bucket] = generation;
        self.len += 1;
        Ok(())
    }

    /// Remove an order id using backward-shift deletion (Knuth
    /// Algorithm R), returning the slot and generation it held.
    pub fn remove(&mut self, order_id: u64) -> Option<(u32, u32)> {
        let start = self.find_bucket(order_id)?;
        let removed = (self.slots[start], self.gens[start]);
        // Knuth Algorithm R: walk the cluster forward from the gap.
        // An entry stays when its home bucket lies in the cyclic
        // interval (gap, scan]; otherwise it shifts back into the gap
        // and its old bucket becomes the new gap.
        let mut gap = start;
        let mut scan = start;
        loop {
            scan = (scan + 1) & self.mask;
            if self.keys[scan] == 0 {
                break;
            }
            let entry_home = self.home_bucket(self.keys[scan]);
            let home_distance = entry_home.wrapping_sub(gap) & self.mask;
            let scan_distance = scan.wrapping_sub(gap) & self.mask;
            if home_distance >= 1 && home_distance <= scan_distance {
                continue; // home is in (gap, scan]: the entry must stay
            }
            self.keys[gap] = self.keys[scan];
            self.slots[gap] = self.slots[scan];
            self.gens[gap] = self.gens[scan];
            gap = scan;
        }
        self.keys[gap] = 0;
        self.slots[gap] = NIL;
        self.gens[gap] = 0;
        self.len -= 1;
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn lookup_hash_is_splitmix64_golden() {
        // Golden values computed independently from the SplitMix64
        // definition in ARCHITECTURE.md section 5.2.
        assert_eq!(hash(0), 0xe220_a839_7b1d_cdaf);
        assert_eq!(hash(1), 0x910a_2dec_8902_5cc1);
        assert_eq!(hash(2), 0x9758_35de_1c97_56ce);
        assert_eq!(hash(42), 0xbdd7_3226_2feb_6e95);
        assert_eq!(hash(u64::MAX), 0xe4d9_7177_1b65_2c20);
        assert_eq!(hash(0xdead_beef), 0x4adf_b90f_68c9_eb9b);
    }

    #[test]
    fn lookup_capacity_is_twice_max_orders_rounded_up() {
        assert_eq!(Lookup::new(1).capacity(), 2);
        assert_eq!(Lookup::new(100).capacity(), 256);
        assert_eq!(Lookup::new(128).capacity(), 256);
        assert_eq!(Lookup::new(129).capacity(), 512);
    }

    #[test]
    fn lookup_load_cap_enforced() {
        let mut table = Lookup::new(4); // capacity 8, cap 4
        for id in 1..=4u64 {
            table.insert(id, 0, 1).expect("insert under cap");
        }
        assert_eq!(table.len(), 4);
        let err = table.insert(99, 0, 1).expect_err("insert at cap must fail");
        assert_eq!(
            err,
            BookError::LookupCapacityFull {
                capacity: 8,
                len: 4
            }
        );
        assert_eq!(table.len(), 4, "failed insert mutates nothing");
        assert_eq!(table.get(99), None);
    }

    #[test]
    fn lookup_insert_get_remove_roundtrip() {
        let mut table = Lookup::new(16);
        assert_eq!(table.get(7), None);
        table.insert(7, 3, 11).expect("insert");
        assert_eq!(table.get(7), Some((3, 11)));
        assert_eq!(table.remove(7), Some((3, 11)));
        assert_eq!(table.get(7), None);
        assert_eq!(table.remove(7), None);
        assert_eq!(table.len(), 0);
    }

    proptest::proptest! {
        /// INV-LOOKUP-2: after any insert and delete sequence, every
        /// remaining key is findable from its home bucket without
        /// crossing an empty bucket, and contents equal the oracle.
        /// The small id range forces dense probe clusters, which is
        /// exactly where backward-shift deletion earns its keep.
        #[test]
        fn prop_lookup_backward_shift_reachability(
            ops in proptest::collection::vec((1u64..60, proptest::bool::ANY), 0..300)
        ) {
            let mut table = Lookup::new(64);
            let mut oracle: BTreeMap<u64, (u32, u32)> = BTreeMap::new();
            for (id, is_insert) in ops {
                if is_insert {
                    if !oracle.contains_key(&id) && oracle.len() < table.load_cap() {
                        table.insert(id, 5, 9).expect("insert under cap");
                        oracle.insert(id, (5, 9));
                    }
                } else {
                    let expected = oracle.remove(&id);
                    proptest::prop_assert_eq!(table.remove(id), expected);
                }
                for (k, v) in &oracle {
                    proptest::prop_assert_eq!(table.get(*k), Some(*v));
                }
                proptest::prop_assert_eq!(table.len(), oracle.len());
            }
        }
    }
}
