//! Heap-backed announce `app_data`: one owned `Vec<u8>` per entry, addressed by an
//! [`AppDataHandle`] index that never reorders. No packing, no byte budget, no entry cap, so
//! `insert`/`replace` can't fail (the `ArenaFull`/`TooManyEntries` arms are unreachable here).

use alloc::vec::Vec;

use crate::routing::announce::stored::{AnnounceAppData, AnnounceAppDataError, AppDataHandle};

#[derive(Debug, Default)]
pub struct HeapAnnounceAppData {
    entries: Vec<Vec<u8>>,
    free_slots: Vec<usize>,
}

impl AnnounceAppData for HeapAnnounceAppData {
    fn get(&self, handle: AppDataHandle) -> &[u8] {
        &self.entries[handle.slot()]
    }

    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, AnnounceAppDataError> {
        if let Some(slot) = self.free_slots.pop() {
            self.entries[slot] = bytes.to_vec();
            Ok(AppDataHandle::new(slot))
        } else {
            let slot = self.entries.len();
            self.entries.push(bytes.to_vec());
            Ok(AppDataHandle::new(slot))
        }
    }

    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), AnnounceAppDataError> {
        self.entries[handle.slot()] = bytes.to_vec();
        Ok(())
    }

    fn free(&mut self, handle: AppDataHandle) {
        self.entries[handle.slot()] = Vec::new();
        self.free_slots.push(handle.slot());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_replaces_and_grows_without_a_budget() {
        let mut store = HeapAnnounceAppData::default();
        let a = store.insert(&[0xAA; 3]).unwrap();
        let b = store.insert(&[0xBB; 5]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 3]);
        assert_eq!(store.get(b), &[0xBB; 5]);

        store.replace(a, &[0x11; 9]).unwrap();
        assert_eq!(store.get(a), &[0x11; 9]);
        assert_eq!(store.get(b), &[0xBB; 5]);
        store.replace(b, &[]).unwrap();
        assert_eq!(store.get(b), &[] as &[u8]);

        for n in 0..500u32 {
            assert!(store.insert(&[n as u8; 200]).is_ok());
        }
        assert_eq!(store.get(a), &[0x11; 9]);
    }

    #[test]
    fn free_then_insert_reuses_the_freed_slot() {
        let mut store = HeapAnnounceAppData::default();
        let a = store.insert(&[0xAA; 3]).unwrap();
        let b = store.insert(&[0xBB; 5]).unwrap();

        store.free(a);
        let d = store.insert(&[0xDD; 2]).unwrap();

        assert_eq!(d, a, "the freed slot index is reused, not appended");
        assert_eq!(store.get(d), &[0xDD; 2]);
        assert_eq!(store.get(b), &[0xBB; 5]);
    }
}
