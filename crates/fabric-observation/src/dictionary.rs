//! Level 5: dictionaries in first-use order.
//!
//! A repeated value (a node id, an attribute key, a metric name) is written
//! once in a table and referenced by its index. The table has one canonical
//! form: entries distinct, in the order of their first reference, none
//! unused. The encoder produces that form by construction ([`Intern`]); the
//! decoder enforces it ([`Lookup`]): an id may only be referenced after every
//! lower id has been, a repeated table entry is refused, and at the end every
//! entry must have been used. Without these three rules the same records
//! would have many encodings (any permutation of the table, any padding).

use crate::bytes::{Cursor, DecodeError};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

/// FNV-1a: a few instructions per byte, no randomness, no dependency. The
/// encode-side dictionary hashes the producer's own strings, so flooding by
/// an adversary is not a concern at this level.
#[derive(Clone, Copy)]
struct Fnv1a(u64);

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

fn fnv<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut h = Fnv1a(0xcbf2_9ce4_8422_2325);
    value.hash(&mut h);
    h.finish()
}

/// Interns values in first-use order while encoding: the table of values and
/// an open-addressing index over it (linear probing, power-of-two capacity,
/// at most half full), built here so the crate stays on `core` and `alloc`.
pub struct Intern<T: Hash + Eq + Clone> {
    order: Vec<T>,
    hashes: Vec<u64>,
    /// Slot holds `id + 1`, or 0 when empty.
    slots: Vec<u32>,
}

impl<T: Hash + Eq + Clone> Default for Intern<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Hash + Eq + Clone> Intern<T> {
    pub fn new() -> Self {
        Self {
            order: Vec::with_capacity(32),
            hashes: Vec::with_capacity(32),
            slots: alloc::vec![0; 64],
        }
    }

    fn mask(&self) -> usize {
        self.slots.len().wrapping_sub(1)
    }

    /// The id of `value`, assigning the next id on first use.
    pub fn id(&mut self, value: &T) -> u64 {
        let h = fnv(value);
        let mut at = (h as usize) & self.mask();
        loop {
            let Some(slot) = self.slots.get(at).copied() else {
                at = 0;
                continue;
            };
            if slot == 0 {
                break;
            }
            let idx = (slot as usize).wrapping_sub(1);
            if self.hashes.get(idx) == Some(&h) && self.order.get(idx) == Some(value) {
                return idx as u64;
            }
            at = at.wrapping_add(1) & self.mask();
        }
        let id = self.order.len();
        self.order.push(value.clone());
        self.hashes.push(h);
        if let Some(slot) = self.slots.get_mut(at) {
            *slot = (id as u32).wrapping_add(1);
        }
        if self.order.len().saturating_mul(2) > self.slots.len() {
            self.grow();
        }
        id as u64
    }

    fn grow(&mut self) {
        let new_len = self.slots.len().saturating_mul(2);
        let mut slots = alloc::vec![0_u32; new_len];
        let mask = new_len.wrapping_sub(1);
        for (idx, h) in self.hashes.iter().enumerate() {
            let mut at = (*h as usize) & mask;
            while slots.get(at).is_some_and(|s| *s != 0) {
                at = at.wrapping_add(1) & mask;
            }
            if let Some(slot) = slots.get_mut(at) {
                *slot = (idx as u32).wrapping_add(1);
            }
        }
        self.slots = slots;
    }

    /// The table, in first-use order.
    pub fn table(&self) -> &[T] {
        &self.order
    }
}

/// Resolves ids while decoding and enforces the canonical table.
pub struct Lookup<T> {
    table: Vec<T>,
    seen: usize,
}

impl<T: Clone + Ord> Lookup<T> {
    /// Accepts a table only if its entries are distinct.
    pub fn new(
        cur: &mut Cursor<'_>,
        table: Vec<T>,
        starts: &[usize],
        what: &'static str,
    ) -> Result<Self, DecodeError> {
        let mut distinct = BTreeSet::new();
        for (i, entry) in table.iter().enumerate() {
            if !distinct.insert(entry.clone()) {
                let at = starts.get(i).copied().unwrap_or(cur.position());
                return cur.fail_at(at, what);
            }
        }
        Ok(Self { table, seen: 0 })
    }

    /// The value for `id`, which must be at most one past the highest id seen.
    pub fn get(&mut self, cur: &mut Cursor<'_>, at: usize, id: u64) -> Result<T, DecodeError> {
        let Ok(id) = usize::try_from(id) else {
            return cur.fail_at(at, "dictionary id out of range");
        };
        if id > self.seen {
            return cur.fail_at(at, "dictionary id used before its first-use turn");
        }
        if id == self.seen {
            self.seen = self.seen.saturating_add(1);
        }
        match self.table.get(id) {
            Some(v) => Ok(v.clone()),
            None => cur.fail_at(at, "dictionary id out of range"),
        }
    }

    /// True once every entry has been referenced.
    pub fn all_used(&self) -> bool {
        self.seen == self.table.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_assigns_ids_in_first_use_order() {
        let mut d = Intern::new();
        assert_eq!(d.id(&"b"), 0);
        assert_eq!(d.id(&"a"), 1);
        assert_eq!(d.id(&"b"), 0);
        assert_eq!(d.table(), &["b", "a"]);
    }

    #[test]
    fn interning_survives_growth_and_collisions() {
        // Far more entries than the initial 64 slots, interleaved with repeats.
        let mut d = Intern::new();
        let values: Vec<alloc::string::String> = (0..5_000_u32)
            .map(|i| alloc::format!("v{}", i % 1_700))
            .collect();
        let ids: Vec<u64> = values.iter().map(|v| d.id(v)).collect();
        assert_eq!(d.table().len(), 1_700);
        for (v, id) in values.iter().zip(&ids) {
            assert_eq!(&d.table()[*id as usize], v);
            assert_eq!(d.id(v), *id, "stable after growth");
        }
    }

    #[test]
    fn lookup_enforces_first_use_order_distinctness_and_use() {
        let data = [0_u8; 8];
        let mut cur = Cursor::new(&data);
        let mut l = Lookup::new(&mut cur, vec!["x", "y", "z"], &[0, 1, 2], "duplicate").unwrap();
        assert_eq!(
            l.get(&mut cur, 0, 1).unwrap_err().reason,
            "dictionary id used before its first-use turn"
        );
        assert_eq!(l.get(&mut cur, 0, 0).unwrap(), "x");
        assert_eq!(l.get(&mut cur, 0, 1).unwrap(), "y");
        assert_eq!(l.get(&mut cur, 0, 0).unwrap(), "x");
        assert!(!l.all_used());
        assert_eq!(l.get(&mut cur, 0, 2).unwrap(), "z");
        assert!(l.all_used());
        assert_eq!(
            l.get(&mut cur, 0, 3).unwrap_err().reason,
            "dictionary id out of range"
        );
        let dup = Lookup::new(&mut cur, vec!["x", "x"], &[4, 6], "duplicate entry");
        let err = dup.err().unwrap();
        assert_eq!((err.reason, err.offset), ("duplicate entry", 6));
    }
}
