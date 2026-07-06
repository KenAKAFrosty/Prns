//! The fixed-capacity, heap-backed twin of [`PackedAppDataArena`]: the byte arena and
//! span/free-slot tables live in a caller-chosen heap region (PSRAM on the S3) via `A`.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::stored::{AnnounceAppData, AnnounceAppDataError, AppDataHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: usize,
    len: usize,
}

fn zeroed<A: Allocator>(len: usize, alloc: A) -> Box<[u8], A> {
    let mut bytes = Vec::with_capacity_in(len, alloc);
    bytes.resize(len, 0u8);
    bytes.into_boxed_slice()
}

pub struct FixedHeapPackedAppDataArena<
    const ARENA_BYTES: usize,
    const MAX_ENTRIES: usize,
    A: Allocator = Global,
> {
    arena: Box<[u8], A>,
    used: usize,
    spans: Vec<Span, A>,
    free_slots: Vec<usize, A>,
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize, A: Allocator + Default> Default
    for FixedHeapPackedAppDataArena<ARENA_BYTES, MAX_ENTRIES, A>
{
    fn default() -> Self {
        Self {
            arena: zeroed(ARENA_BYTES, A::default()),
            used: 0,
            spans: Vec::with_capacity_in(MAX_ENTRIES, A::default()),
            free_slots: Vec::with_capacity_in(MAX_ENTRIES, A::default()),
        }
    }
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize, A: Allocator>
    FixedHeapPackedAppDataArena<ARENA_BYTES, MAX_ENTRIES, A>
{
    pub fn get(&self, handle: AppDataHandle) -> &[u8] {
        let span = self.spans[handle.slot()];
        &self.arena[span.offset..span.offset + span.len]
    }

    pub fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, AnnounceAppDataError> {
        if self.free_slots.is_empty() && self.spans.len() >= MAX_ENTRIES {
            return Err(AnnounceAppDataError::TooManyEntries);
        }
        if bytes.len() > ARENA_BYTES - self.used {
            return Err(AnnounceAppDataError::ArenaFull);
        }

        let offset = self.used;
        self.arena[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.used += bytes.len();
        let span = Span {
            offset,
            len: bytes.len(),
        };
        if let Some(slot) = self.free_slots.pop() {
            self.spans[slot] = span;
            Ok(AppDataHandle::new(slot))
        } else {
            self.spans.push(span);
            Ok(AppDataHandle::new(self.spans.len() - 1))
        }
    }

    pub fn free(&mut self, handle: AppDataHandle) {
        let span = self.spans[handle.slot()];
        let tail_start = span.offset + span.len;
        let tail_len = self.used - tail_start;
        self.arena
            .copy_within(tail_start..tail_start + tail_len, span.offset);
        for other in self.spans.iter_mut() {
            if other.offset > span.offset {
                other.offset -= span.len;
            }
        }
        self.used -= span.len;
        self.spans[handle.slot()] = Span { offset: 0, len: 0 };
        self.free_slots.push(handle.slot());
    }

    pub fn replace(
        &mut self,
        handle: AppDataHandle,
        bytes: &[u8],
    ) -> Result<(), AnnounceAppDataError> {
        let span = self.spans[handle.slot()];
        let new_len = bytes.len();

        if new_len == span.len {
            self.arena[span.offset..span.offset + new_len].copy_from_slice(bytes);
            return Ok(());
        }

        let new_used = self.used - span.len + new_len;
        if new_used > ARENA_BYTES {
            return Err(AnnounceAppDataError::ArenaFull);
        }

        let tail_start = span.offset + span.len;
        let tail_len = self.used - tail_start;
        let new_tail_start = span.offset + new_len;
        self.arena
            .copy_within(tail_start..tail_start + tail_len, new_tail_start);
        self.arena[span.offset..span.offset + new_len].copy_from_slice(bytes);

        self.spans[handle.slot()].len = new_len;
        for other in self.spans.iter_mut() {
            if other.offset > span.offset {
                other.offset =
                    (other.offset as isize + new_len as isize - span.len as isize) as usize;
            }
        }
        self.used = new_used;
        Ok(())
    }
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize, A: Allocator> AnnounceAppData
    for FixedHeapPackedAppDataArena<ARENA_BYTES, MAX_ENTRIES, A>
{
    fn get(&self, handle: AppDataHandle) -> &[u8] {
        FixedHeapPackedAppDataArena::get(self, handle)
    }

    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, AnnounceAppDataError> {
        FixedHeapPackedAppDataArena::insert(self, bytes)
    }

    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), AnnounceAppDataError> {
        FixedHeapPackedAppDataArena::replace(self, handle, bytes)
    }

    fn free(&mut self, handle: AppDataHandle) {
        FixedHeapPackedAppDataArena::free(self, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_packed<const A: usize, const M: usize>(store: &FixedHeapPackedAppDataArena<A, M>) {
        let mut covered = std::vec![false; store.used];
        let mut total = 0;
        for (slot, span) in store.spans.iter().enumerate() {
            if store.free_slots.contains(&slot) {
                continue;
            }
            total += span.len;
            for byte in &mut covered[span.offset..span.offset + span.len] {
                assert!(!*byte, "live spans must not overlap");
                *byte = true;
            }
        }
        assert_eq!(
            total, store.used,
            "used must equal the sum of live span lengths"
        );
        assert!(
            covered.iter().all(|&c| c),
            "live spans must cover [0, used) with no gaps"
        );
    }

    #[test]
    fn insert_then_get_round_trips() {
        let mut store = FixedHeapPackedAppDataArena::<64, 4>::default();
        let h = store.insert(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(store.get(h), &[1, 2, 3, 4, 5]);
        assert_packed(&store);
    }

    #[test]
    fn multiple_payloads_pack_contiguously_and_read_back() {
        let mut store = FixedHeapPackedAppDataArena::<64, 4>::default();
        let a = store.insert(&[0xAA; 3]).unwrap();
        let b = store.insert(&[0xBB; 5]).unwrap();
        let c = store.insert(&[0xCC; 2]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 3]);
        assert_eq!(store.get(b), &[0xBB; 5]);
        assert_eq!(store.get(c), &[0xCC; 2]);
        assert_eq!(store.used, 10);
        assert_packed(&store);
    }

    #[test]
    fn replace_larger_shifts_the_tail_and_preserves_neighbors() {
        let mut store = FixedHeapPackedAppDataArena::<64, 4>::default();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 4]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[0x22; 9]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(b), &[0x22; 9]);
        assert_eq!(store.get(c), &[0xCC; 4]);
        assert_eq!(store.used, 17);
        assert_packed(&store);
    }

    #[test]
    fn free_then_insert_reuses_the_freed_slot() {
        let mut store = FixedHeapPackedAppDataArena::<64, 4>::default();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 6]).unwrap();

        store.free(a);
        let d = store.insert(&[0xDD; 3]).unwrap();

        assert_eq!(d, a, "the freed slot index is reused, not appended");
        assert_eq!(store.get(d), &[0xDD; 3]);
        assert_eq!(store.get(b), &[0xBB; 6]);
        assert_packed(&store);
    }

    #[test]
    fn insert_past_the_entry_cap_errors() {
        let mut store = FixedHeapPackedAppDataArena::<64, 2>::default();
        store.insert(&[1]).unwrap();
        store.insert(&[2]).unwrap();
        assert_eq!(
            store.insert(&[3]),
            Err(AnnounceAppDataError::TooManyEntries)
        );
    }

    #[test]
    fn insert_past_the_byte_budget_errors_and_leaves_the_store_unchanged() {
        let mut store = FixedHeapPackedAppDataArena::<8, 4>::default();
        let a = store.insert(&[0xAA; 6]).unwrap();
        assert_eq!(
            store.insert(&[0xBB; 4]),
            Err(AnnounceAppDataError::ArenaFull)
        );
        assert_eq!(store.used, 6);
        assert_eq!(store.get(a), &[0xAA; 6]);
        assert_packed(&store);
    }
}
