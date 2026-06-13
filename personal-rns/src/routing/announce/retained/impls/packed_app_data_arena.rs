//! A fixed-capacity, alloc-free store for variable-length byte payloads.
//!
//! Built to retain one announce payload per known path without paying a
//! fixed-maximum slot for every entry: announces vary widely in size (an
//! app-data-less one is ~148 bytes, the cap is ~481), so a packed arena keyed
//! by an opaque [`AppDataHandle`] holds only the bytes actually present.
//!
//! The arena is kept *packed* — the live payloads occupy a contiguous prefix
//! `[0, used)` with no gaps — so growing, shrinking, or clearing a payload
//! shifts the trailing bytes and fixes up offsets. A handle is an index into
//! the span table, which those moves never reorder, so a handle stays valid for
//! the life of its entry. This is the seam: callers hold handles and never see
//! the packing, so the internal strategy (compacting today; a free-list later
//! if profiling ever demands) can change without touching them.
//!
//! `PartialEq` is structural — it compares the whole backing arena, including
//! the dead tail past `used` that a shrink leaves behind, so `==` means
//! "identical representation," not "same set of payloads." That is deliberate
//! and is the only use: determinism tests feed two stores the same operations
//! in the same order and assert byte-identical results. Do not use `==` to ask
//! whether two stores built by different routes hold the same payloads.

use crate::routing::storage::{AppDataHandle, RetainedAppData, RetainedAppDataError};
use heapless::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: usize,
    len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedAppDataArena<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> {
    arena: [u8; ARENA_BYTES],
    used: usize,
    spans: Vec<Span, MAX_ENTRIES>,
    free_slots: Vec<usize, MAX_ENTRIES>,
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> Default
    for PackedAppDataArena<ARENA_BYTES, MAX_ENTRIES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize>
    PackedAppDataArena<ARENA_BYTES, MAX_ENTRIES>
{
    pub const fn new() -> Self {
        Self {
            arena: [0u8; ARENA_BYTES],
            used: 0,
            spans: Vec::new(),
            free_slots: Vec::new(),
        }
    }

    pub fn get(&self, handle: AppDataHandle) -> &[u8] {
        let span = self.spans[handle.slot()];
        &self.arena[span.offset..span.offset + span.len]
    }

    pub fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, RetainedAppDataError> {
        if self.free_slots.is_empty() && self.spans.is_full() {
            return Err(RetainedAppDataError::TooManyEntries);
        }
        if bytes.len() > ARENA_BYTES - self.used {
            return Err(RetainedAppDataError::ArenaFull);
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
            let _ = self.spans.push(span);
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
        let _ = self.free_slots.push(handle.slot());
    }

    pub fn replace(
        &mut self,
        handle: AppDataHandle,
        bytes: &[u8],
    ) -> Result<(), RetainedAppDataError> {
        let span = self.spans[handle.slot()];
        let new_len = bytes.len();

        if new_len == span.len {
            self.arena[span.offset..span.offset + new_len].copy_from_slice(bytes);
            return Ok(());
        }

        // The old bytes are reclaimed as part of the move, so this only fails
        // when the new payload won't fit even after that. Check before touching
        // anything, so the error path leaves a valid store.
        let new_used = self.used - span.len + new_len;
        if new_used > ARENA_BYTES {
            return Err(RetainedAppDataError::ArenaFull);
        }

        let tail_start = span.offset + span.len;
        let tail_len = self.used - tail_start;
        let new_tail_start = span.offset + new_len;
        // copy_within is memmove: correct whether the tail moves up (grow) or
        // down (shrink), including overlap.
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

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> RetainedAppData
    for PackedAppDataArena<ARENA_BYTES, MAX_ENTRIES>
{
    fn get(&self, handle: AppDataHandle) -> &[u8] {
        PackedAppDataArena::get(self, handle)
    }

    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, RetainedAppDataError> {
        PackedAppDataArena::insert(self, bytes)
    }

    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), RetainedAppDataError> {
        PackedAppDataArena::replace(self, handle, bytes)
    }

    fn free(&mut self, handle: AppDataHandle) {
        PackedAppDataArena::free(self, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_packed<const A: usize, const M: usize>(store: &PackedAppDataArena<A, M>) {
        // Live spans (those not on the free list) must tile [0, used) with no gaps
        // and no overlap. Free-slot reuse means spans are no longer offset-ordered,
        // so coverage is checked directly rather than by a running offset.
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
        let mut store = PackedAppDataArena::<64, 4>::new();
        let h = store.insert(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(store.get(h), &[1, 2, 3, 4, 5]);
        assert_packed(&store);
    }

    #[test]
    fn multiple_payloads_pack_contiguously_and_read_back() {
        let mut store = PackedAppDataArena::<64, 4>::new();
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
    fn replace_same_size_overwrites_in_place_without_moving_neighbors() {
        let mut store = PackedAppDataArena::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 4]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[0x11; 4]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(b), &[0x11; 4]);
        assert_eq!(store.get(c), &[0xCC; 4]);
        assert_packed(&store);
    }

    #[test]
    fn replace_larger_shifts_the_tail_and_preserves_neighbors() {
        let mut store = PackedAppDataArena::<64, 4>::new();
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
    fn replace_smaller_shifts_the_tail_down_and_preserves_neighbors() {
        let mut store = PackedAppDataArena::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 8]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[0x33; 2]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(b), &[0x33; 2]);
        assert_eq!(store.get(c), &[0xCC; 4]);
        assert_eq!(store.used, 10);
        assert_packed(&store);
    }

    #[test]
    fn replace_to_empty_clears_and_reclaims_while_keeping_the_handle_valid() {
        let mut store = PackedAppDataArena::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 6]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[]).unwrap();
        assert_eq!(store.get(b), &[] as &[u8]);
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(c), &[0xCC; 4]);
        assert_eq!(store.used, 8);
        assert_packed(&store);
    }

    #[test]
    fn insert_past_the_byte_budget_errors_and_leaves_the_store_unchanged() {
        let mut store = PackedAppDataArena::<8, 4>::new();
        let a = store.insert(&[0xAA; 6]).unwrap();
        let before = store.clone();
        assert_eq!(
            store.insert(&[0xBB; 4]),
            Err(RetainedAppDataError::ArenaFull)
        );
        assert_eq!(store, before);
        assert_eq!(store.get(a), &[0xAA; 6]);
        assert_packed(&store);
    }

    #[test]
    fn replace_larger_past_the_budget_errors_and_leaves_the_store_unchanged() {
        let mut store = PackedAppDataArena::<8, 4>::new();
        let a = store.insert(&[0xAA; 3]).unwrap();
        let b = store.insert(&[0xBB; 3]).unwrap();
        let before = store.clone();
        assert_eq!(
            store.replace(b, &[0xBB; 7]),
            Err(RetainedAppDataError::ArenaFull)
        );
        assert_eq!(store, before);
        assert_eq!(store.get(a), &[0xAA; 3]);
        assert_eq!(store.get(b), &[0xBB; 3]);
        assert_packed(&store);
    }

    #[test]
    fn insert_past_the_entry_cap_errors() {
        let mut store = PackedAppDataArena::<64, 2>::new();
        store.insert(&[1]).unwrap();
        store.insert(&[2]).unwrap();
        assert_eq!(
            store.insert(&[3]),
            Err(RetainedAppDataError::TooManyEntries)
        );
    }

    #[test]
    fn fills_the_arena_to_exact_capacity() {
        let mut store = PackedAppDataArena::<8, 4>::new();
        let h = store.insert(&[0x7; 8]).unwrap();
        assert_eq!(store.used, 8);
        assert_eq!(store.get(h), &[0x7; 8]);
        assert_eq!(store.insert(&[0x9]), Err(RetainedAppDataError::ArenaFull));
        assert_packed(&store);
    }

    #[test]
    fn free_reclaims_bytes_and_preserves_neighbors() {
        let mut store = PackedAppDataArena::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 6]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();

        store.free(b);

        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(c), &[0xCC; 4]);
        assert_eq!(store.used, 8);
        assert_packed(&store);
    }

    #[test]
    fn free_then_insert_reuses_the_freed_slot() {
        let mut store = PackedAppDataArena::<64, 4>::new();
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
    fn a_freed_slot_keeps_the_entry_cap_from_filling() {
        let mut store = PackedAppDataArena::<64, 2>::new();
        let a = store.insert(&[1]).unwrap();
        store.insert(&[2]).unwrap();
        assert_eq!(
            store.insert(&[3]),
            Err(RetainedAppDataError::TooManyEntries)
        );

        store.free(a);
        assert!(
            store.insert(&[3]).is_ok(),
            "the freed slot admits a new entry"
        );
        assert_packed(&store);
    }

    #[test]
    fn freeing_the_only_entry_empties_the_store() {
        let mut store = PackedAppDataArena::<64, 4>::new();
        let a = store.insert(&[0xAA; 5]).unwrap();
        store.free(a);
        assert_eq!(store.used, 0);
        assert_packed(&store);
    }

    #[test]
    fn identical_operation_sequences_yield_byte_identical_stores() {
        fn build() -> PackedAppDataArena<64, 4> {
            let mut s = PackedAppDataArena::<64, 4>::new();
            let a = s.insert(&[0xAA; 4]).unwrap();
            let b = s.insert(&[0xBB; 4]).unwrap();
            s.insert(&[0xCC; 4]).unwrap();
            s.replace(a, &[0x11; 7]).unwrap();
            s.replace(b, &[]).unwrap();
            s
        }
        assert_eq!(build(), build());
    }
}
