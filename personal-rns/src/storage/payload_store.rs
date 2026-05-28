//! A fixed-capacity, alloc-free store for variable-length byte payloads.
//!
//! Built to retain one announce payload per known path without paying a
//! fixed-maximum slot for every entry: announces vary widely in size (an
//! app-data-less one is ~148 bytes, the cap is ~481), so a packed arena keyed
//! by an opaque [`PayloadHandle`] holds only the bytes actually present.
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

use heapless::Vec;

/// An opaque, stable reference to a payload held by a [`PayloadStore`]. Indexes
/// the span table, which compaction never reorders, so the handle survives the
/// arena moves that grow/shrink other payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadHandle(usize);

/// Where one payload sits in the arena. Spans are held in offset order with no
/// gaps between them, mirroring the packed arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadStoreError {
    ArenaFull,      //full of? memory?
    TooManyEntries, // same but just count? why do we even need this? do we?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadStore<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> {
    arena: [u8; ARENA_BYTES],
    used: usize,
    spans: Vec<Span, MAX_ENTRIES>,
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> Default
    for PayloadStore<ARENA_BYTES, MAX_ENTRIES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> PayloadStore<ARENA_BYTES, MAX_ENTRIES> {
    pub const fn new() -> Self {
        Self {
            arena: [0u8; ARENA_BYTES],
            used: 0,
            spans: Vec::new(),
        }
    }

    /// A handle obtained from this store is always
    /// valid (there is no removal yet), so this never fails; a cleared payload
    /// reads back as an empty slice.
    pub fn get(&self, handle: PayloadHandle) -> &[u8] {
        let span = self.spans[handle.0];
        &self.arena[span.offset..span.offset + span.len]
    }

    /// Copy `bytes` into the arena and return a handle to them.
    pub fn insert(&mut self, bytes: &[u8]) -> Result<PayloadHandle, PayloadStoreError> {
        if self.spans.is_full() {
            return Err(PayloadStoreError::TooManyEntries);
        }
        if bytes.len() > ARENA_BYTES - self.used {
            return Err(PayloadStoreError::ArenaFull);
        }

        let offset = self.used;
        self.arena[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.used += bytes.len();
        // Cannot fail: capacity was checked above.
        let _ = self.spans.push(Span {
            offset,
            len: bytes.len(),
        });
        Ok(PayloadHandle(self.spans.len() - 1))
    }

    /// Replace the payload behind `handle`, which may change its size. On a size
    /// change the trailing payloads shift to keep the arena packed and their
    /// offsets are fixed up. Replacing with an empty slice clears the payload and
    /// reclaims its bytes (the handle stays valid and reads back empty). On
    /// `ArenaFull` the store is left exactly as it was.
    pub fn replace(
        &mut self,
        handle: PayloadHandle,
        bytes: &[u8],
    ) -> Result<(), PayloadStoreError> {
        let span = self.spans[handle.0];
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
            return Err(PayloadStoreError::ArenaFull);
        }

        let tail_start = span.offset + span.len;
        let tail_len = self.used - tail_start;
        let new_tail_start = span.offset + new_len;
        // copy_within is memmove: correct whether the tail moves up (grow) or
        // down (shrink), including overlap.
        self.arena
            .copy_within(tail_start..tail_start + tail_len, new_tail_start);
        self.arena[span.offset..span.offset + new_len].copy_from_slice(bytes);

        self.spans[handle.0].len = new_len;
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

impl<const ARENA_BYTES: usize, const MAX_ENTRIES: usize> crate::storage::AppDataBackend
    for PayloadStore<ARENA_BYTES, MAX_ENTRIES>
{
    fn get(&self, handle: PayloadHandle) -> &[u8] {
        PayloadStore::get(self, handle)
    }

    fn insert(&mut self, bytes: &[u8]) -> Result<PayloadHandle, PayloadStoreError> {
        PayloadStore::insert(self, bytes)
    }

    fn replace(&mut self, handle: PayloadHandle, bytes: &[u8]) -> Result<(), PayloadStoreError> {
        PayloadStore::replace(self, handle, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store's core invariant: live payloads pack a contiguous prefix with
    /// no gaps, and `used` equals the sum of their lengths.
    fn assert_packed<const A: usize, const M: usize>(store: &PayloadStore<A, M>) {
        let mut expected_offset = 0;
        let mut total = 0;
        for span in &store.spans {
            assert_eq!(span.offset, expected_offset, "spans must be gap-free");
            expected_offset += span.len;
            total += span.len;
        }
        assert_eq!(total, store.used, "used must equal the sum of span lengths");
    }

    #[test]
    fn insert_then_get_round_trips() {
        let mut store = PayloadStore::<64, 4>::new();
        let h = store.insert(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(store.get(h), &[1, 2, 3, 4, 5]);
        assert_packed(&store);
    }

    #[test]
    fn multiple_payloads_pack_contiguously_and_read_back() {
        let mut store = PayloadStore::<64, 4>::new();
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
        let mut store = PayloadStore::<64, 4>::new();
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
        let mut store = PayloadStore::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 4]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[0x22; 9]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(b), &[0x22; 9]);
        assert_eq!(store.get(c), &[0xCC; 4]); // shifted up, still intact
        assert_eq!(store.used, 17);
        assert_packed(&store);
    }

    #[test]
    fn replace_smaller_shifts_the_tail_down_and_preserves_neighbors() {
        let mut store = PayloadStore::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 8]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[0x33; 2]).unwrap();
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(b), &[0x33; 2]);
        assert_eq!(store.get(c), &[0xCC; 4]); // shifted down, still intact
        assert_eq!(store.used, 10);
        assert_packed(&store);
    }

    #[test]
    fn replace_to_empty_clears_and_reclaims_while_keeping_the_handle_valid() {
        let mut store = PayloadStore::<64, 4>::new();
        let a = store.insert(&[0xAA; 4]).unwrap();
        let b = store.insert(&[0xBB; 6]).unwrap();
        let c = store.insert(&[0xCC; 4]).unwrap();
        store.replace(b, &[]).unwrap();
        assert_eq!(store.get(b), &[] as &[u8]); // still resolves, now empty
        assert_eq!(store.get(a), &[0xAA; 4]);
        assert_eq!(store.get(c), &[0xCC; 4]); // reclaimed b's 6 bytes
        assert_eq!(store.used, 8);
        assert_packed(&store);
    }

    #[test]
    fn insert_past_the_byte_budget_errors_and_leaves_the_store_unchanged() {
        let mut store = PayloadStore::<8, 4>::new();
        let a = store.insert(&[0xAA; 6]).unwrap();
        let before = store.clone();
        assert_eq!(store.insert(&[0xBB; 4]), Err(PayloadStoreError::ArenaFull));
        assert_eq!(store, before); // error path mutated nothing
        assert_eq!(store.get(a), &[0xAA; 6]);
        assert_packed(&store);
    }

    #[test]
    fn replace_larger_past_the_budget_errors_and_leaves_the_store_unchanged() {
        let mut store = PayloadStore::<8, 4>::new();
        let a = store.insert(&[0xAA; 3]).unwrap();
        let b = store.insert(&[0xBB; 3]).unwrap();
        let before = store.clone();
        // Replacing b (3) with 7 needs used = 6-3+7 = 10 > 8.
        assert_eq!(
            store.replace(b, &[0xBB; 7]),
            Err(PayloadStoreError::ArenaFull)
        );
        assert_eq!(store, before);
        assert_eq!(store.get(a), &[0xAA; 3]);
        assert_eq!(store.get(b), &[0xBB; 3]);
        assert_packed(&store);
    }

    #[test]
    fn insert_past_the_entry_cap_errors() {
        let mut store = PayloadStore::<64, 2>::new();
        store.insert(&[1]).unwrap();
        store.insert(&[2]).unwrap();
        assert_eq!(store.insert(&[3]), Err(PayloadStoreError::TooManyEntries));
    }

    #[test]
    fn fills_the_arena_to_exact_capacity() {
        let mut store = PayloadStore::<8, 4>::new();
        let h = store.insert(&[0x7; 8]).unwrap();
        assert_eq!(store.used, 8);
        assert_eq!(store.get(h), &[0x7; 8]);
        assert_eq!(store.insert(&[0x9]), Err(PayloadStoreError::ArenaFull));
        assert_packed(&store);
    }

    #[test]
    fn identical_operation_sequences_yield_byte_identical_stores() {
        fn build() -> PayloadStore<64, 4> {
            let mut s = PayloadStore::<64, 4>::new();
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
