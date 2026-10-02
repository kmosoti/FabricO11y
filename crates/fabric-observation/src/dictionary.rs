//! Level 4: dictionaries in first-use order.
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
use std::collections::{BTreeMap, BTreeSet};

/// Interns values in first-use order while encoding.
pub struct Intern<T: Ord + Clone> {
    ids: BTreeMap<T, u64>,
    order: Vec<T>,
}

impl<T: Ord + Clone> Default for Intern<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord + Clone> Intern<T> {
    pub fn new() -> Self {
        Self {
            ids: BTreeMap::new(),
            order: Vec::new(),
        }
    }
    /// The id of `value`, assigning the next id on first use.
    pub fn id(&mut self, value: &T) -> u64 {
        if let Some(id) = self.ids.get(value) {
            return *id;
        }
        let id = self.order.len() as u64;
        self.ids.insert(value.clone(), id);
        self.order.push(value.clone());
        id
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
