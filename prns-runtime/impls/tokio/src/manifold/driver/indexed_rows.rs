use prns_core::lemire_index::{HeapLemireIndex, IndexRow};

/// A dense host-side table with a Lemire side index from each row's immutable key to its slot.
///
/// Rows may be mutated in place, but their [`IndexRow::index_key`] must never change. Removal uses
/// `swap_remove` and repoints the moved row before changing the dense storage.
#[derive(Debug)]
pub(super) struct IndexedRows<R: IndexRow> {
    rows: std::vec::Vec<R>,
    index: HeapLemireIndex,
}

impl<R: IndexRow> Default for IndexedRows<R> {
    fn default() -> Self {
        Self {
            rows: std::vec::Vec::new(),
            index: HeapLemireIndex::default(),
        }
    }
}

impl<R: IndexRow> IndexedRows<R> {
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(super) fn index_of(&self, key: &R::Key) -> Option<usize> {
        self.index.get(key, &self.rows)
    }

    pub(super) fn get(&self, key: &R::Key) -> Option<&R> {
        self.index_of(key).map(|row| &self.rows[row])
    }

    pub(super) fn get_mut(&mut self, key: &R::Key) -> Option<&mut R> {
        let row = self.index_of(key)?;
        Some(&mut self.rows[row])
    }

    pub(super) fn row_mut(&mut self, row: usize) -> &mut R {
        &mut self.rows[row]
    }

    pub(super) fn iter(&self) -> core::slice::Iter<'_, R> {
        self.rows.iter()
    }

    /// Mutate row payloads without changing their index keys.
    pub(super) fn iter_mut(&mut self) -> core::slice::IterMut<'_, R> {
        self.rows.iter_mut()
    }

    /// Inserts a row unless its key is already live, returning whether insertion occurred.
    pub(super) fn push(&mut self, row: R) -> bool {
        if self.index.contains(row.index_key(), &self.rows) {
            return false;
        }
        self.rows.push(row);
        self.index.insert(self.rows.len() - 1, &self.rows);
        true
    }

    pub(super) fn remove(&mut self, key: &R::Key) -> Option<R> {
        let row = self.index_of(key)?;
        self.index.remove(key, &self.rows);
        let last = self.rows.len() - 1;
        if row != last {
            let moved_key = *self.rows[last].index_key();
            self.index.repoint(&moved_key, row, &self.rows);
        }
        Some(self.rows.swap_remove(row))
    }

    /// Retains matching rows. Removing a row may exchange it with the final row; callers that
    /// care about traversal order can restore their desired rotation afterward.
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&R) -> bool) {
        let mut row = 0;
        while row < self.rows.len() {
            if keep(&self.rows[row]) {
                row += 1;
            } else {
                let key = *self.rows[row].index_key();
                self.remove(&key);
            }
        }
    }

    /// Rotates traversal order and rebuilds slot references in the side index.
    pub(super) fn rotate_left(&mut self, amount: usize) {
        if self.rows.is_empty() {
            return;
        }
        let rotation = amount % self.rows.len();
        self.rows.rotate_left(rotation);
        self.index.clear();
        for row in 0..self.rows.len() {
            self.index.insert(row, &self.rows);
        }
    }
}

impl<R: IndexRow> From<std::vec::Vec<R>> for IndexedRows<R> {
    fn from(rows: std::vec::Vec<R>) -> Self {
        let mut indexed = Self::default();
        for row in rows {
            let inserted = indexed.push(row);
            debug_assert!(inserted, "indexed rows require unique live keys");
        }
        indexed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InterfaceId;

    fn interface_id(n: u32) -> InterfaceId {
        let mut id = [0x07, 0, 0, 0, 0, 0, 0, 0];
        id[4..].copy_from_slice(&n.to_be_bytes());
        InterfaceId::new(id)
    }

    #[test]
    fn indexed_rows_match_a_linear_oracle_through_attach_detach_churn() {
        let mut rows = IndexedRows::default();
        let mut live = std::vec::Vec::new();
        let mut rng = 0xA24B_AED4_963E_E407u64;
        let mut next_id = 0u32;

        for _ in 0..2_000 {
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let attach = live.len() < 2 || !(rng >> 33).is_multiple_of(3);
            if attach {
                let id = interface_id(next_id);
                assert!(rows.push(id));
                live.push(id);
                next_id += 1;
            } else {
                let victim = ((rng >> 17) as usize) % live.len();
                let id = live.swap_remove(victim);
                assert_eq!(rows.remove(&id), Some(id));
            }

            assert_eq!(rows.len(), live.len());
            for id in &live {
                assert_eq!(rows.get(id), Some(id));
            }
            assert_eq!(rows.get(&interface_id(next_id + 7)), None);
        }

        assert!(live.len() > 100, "the run must force repeated index growth");
    }

    #[test]
    fn duplicate_and_unknown_keys_leave_the_table_unchanged() {
        let id = interface_id(7);
        let unknown = interface_id(9);
        let mut rows = IndexedRows::default();

        assert!(rows.push(id));
        assert!(!rows.push(id));
        assert_eq!(rows.remove(&unknown), None);
        assert_eq!(rows.iter().copied().collect::<std::vec::Vec<_>>(), [id]);
    }

    #[test]
    fn retain_and_rotation_keep_every_slot_reference_truthful() {
        let mut rows: IndexedRows<InterfaceId> = (0..128)
            .map(interface_id)
            .collect::<std::vec::Vec<_>>()
            .into();

        rows.retain(|id| id.as_bytes()[7].is_multiple_of(2));
        rows.rotate_left(17);

        assert_eq!(rows.len(), 64);
        for n in 0..128 {
            let id = interface_id(n);
            assert_eq!(rows.get(&id).copied(), n.is_multiple_of(2).then_some(id));
        }
    }
}
