//! [`FixedResourceTable`]'s bulk per-slot buffers (transfer bytes, part-name map, part flags) in a caller-chosen heap region (PSRAM on the S3) via `A`; the tiny slot metadata stays inline. `MAX_PARTS` must cover `TRANSFER_BYTES` at the broadcast-MTU sdu; the constructor proves it at compile time.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::resources::table::{
    ResourceBuffers, ResourceRowState, ResourceTable, ResourceTableAdmission,
    ResourceTablePushError,
};
use crate::routing::links::resources::{
    max_part_count, ResourceBufferShape, ResourceHash, MAP_HASH_LEN,
};
use crate::routing::links::LinkId;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

/// `slots` independent zeroed byte buffers of `width` bytes, built directly in `A`. The widest thing it ever stages on the caller's stack is a single `0u8`; it never stages a whole `[0u8; width]` slot block, which at the resource transfer width is a stack-resident transient the size of an entire slot. The per-slot box is the seam that keeps construction off the stack as `TRANSFER_BYTES` grows.
fn flat_slots<A: Allocator + Default>(width: usize, slots: usize) -> Box<[Box<[u8], A>], A> {
    let mut outer = Vec::with_capacity_in(slots, A::default());
    for _ in 0..slots {
        outer.push(filled(0u8, width, A::default()));
    }
    outer.into_boxed_slice()
}

pub struct FixedHeapResourceTable<
    State: ResourceRowState,
    const SLOTS: usize,
    const TRANSFER_BYTES: usize,
    const MAX_PARTS: usize,
    A: Allocator = Global,
> {
    len: usize,
    link_ids: [LinkId; SLOTS],
    hashes: [ResourceHash; SLOTS],
    timeout_ats: [Option<InstantMillis>; SLOTS],
    states: [State; SLOTS],
    transfers: Box<[Box<[u8], A>], A>,
    part_names: Box<[[[u8; MAP_HASH_LEN]; MAX_PARTS]], A>,
    part_flags: Box<[[bool; MAX_PARTS]], A>,
    streamed_opens: [State::StreamedOpenSlot; SLOTS],
}

impl<
        State: ResourceRowState + Default,
        const SLOTS: usize,
        const TRANSFER_BYTES: usize,
        const MAX_PARTS: usize,
        A: Allocator + Default,
    > Default for FixedHeapResourceTable<State, SLOTS, TRANSFER_BYTES, MAX_PARTS, A>
{
    fn default() -> Self {
        const {
            assert!(
                MAX_PARTS >= max_part_count(TRANSFER_BYTES),
                "MAX_PARTS must cover TRANSFER_BYTES at the broadcast-MTU sdu",
            );
        }
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); SLOTS],
            hashes: [ResourceHash::new([0u8; 32]); SLOTS],
            timeout_ats: [None; SLOTS],
            states: core::array::from_fn(|_| State::default()),
            transfers: flat_slots::<A>(TRANSFER_BYTES, SLOTS),
            part_names: filled([[0u8; MAP_HASH_LEN]; MAX_PARTS], SLOTS, A::default()),
            part_flags: filled([false; MAX_PARTS], SLOTS, A::default()),
            streamed_opens: core::array::from_fn(|_| Default::default()),
        }
    }
}

impl<
        State: ResourceRowState + Default,
        const SLOTS: usize,
        const TRANSFER_BYTES: usize,
        const MAX_PARTS: usize,
        A: Allocator,
    > ResourceTable<State> for FixedHeapResourceTable<State, SLOTS, TRANSFER_BYTES, MAX_PARTS, A>
{
    fn capacity(&self) -> usize {
        SLOTS
    }
    fn transfer_capacity(&self) -> usize {
        TRANSFER_BYTES
    }
    fn part_capacity(&self) -> usize {
        MAX_PARTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids[..self.len]
    }
    fn hashes(&self) -> &[ResourceHash] {
        &self.hashes[..self.len]
    }
    fn timeout_ats(&self) -> &[Option<InstantMillis>] {
        &self.timeout_ats[..self.len]
    }
    fn states(&self) -> &[State] {
        &self.states[..self.len]
    }

    fn set_hash(&mut self, index: usize, hash: ResourceHash) {
        self.hashes[index] = hash;
    }
    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.timeout_ats[index] = timeout_at;
    }
    fn state_mut(&mut self, index: usize) -> &mut State {
        &mut self.states[index]
    }

    fn transfer(&self, index: usize) -> &[u8] {
        &self.transfers[index][..]
    }
    fn part_names(&self, index: usize) -> &[[u8; MAP_HASH_LEN]] {
        &self.part_names[index]
    }
    fn part_flags(&self, index: usize) -> &[bool] {
        &self.part_flags[index]
    }
    fn streamed_open(&self, index: usize) -> &State::StreamedOpenSlot {
        &self.streamed_opens[index]
    }
    fn buffers_mut(&mut self, index: usize) -> ResourceBuffers<'_> {
        ResourceBuffers {
            transfer: &mut self.transfers[index][..],
            part_names: &mut self.part_names[index],
            part_flags: &mut self.part_flags[index],
        }
    }
    fn transfer_and_streamed_open_mut(
        &mut self,
        index: usize,
    ) -> (&mut [u8], &mut State::StreamedOpenSlot) {
        (
            &mut self.transfers[index][..],
            &mut self.streamed_opens[index],
        )
    }

    fn admission_for_shape(&self, shape: ResourceBufferShape) -> ResourceTableAdmission {
        if shape.transfer_bytes() > TRANSFER_BYTES || shape.part_count() > MAX_PARTS {
            ResourceTableAdmission::Impossible
        } else if self.len >= SLOTS {
            ResourceTableAdmission::TemporarilyFull
        } else {
            ResourceTableAdmission::Available
        }
    }

    fn push(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        state: State,
        shape: ResourceBufferShape,
    ) -> Result<usize, ResourceTablePushError> {
        if shape.transfer_bytes() > TRANSFER_BYTES {
            return Err(ResourceTablePushError::TransferTooLarge);
        }
        if shape.part_count() > MAX_PARTS {
            return Err(ResourceTablePushError::TooManyParts);
        }
        if self.len >= SLOTS {
            return Err(ResourceTablePushError::TableFull);
        }
        let index = self.len;
        self.link_ids[index] = link_id;
        self.hashes[index] = hash;
        self.timeout_ats[index] = None;
        self.states[index] = state;
        self.part_flags[index] = [false; MAX_PARTS];
        self.len += 1;
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        self.link_ids.swap(index, last);
        self.hashes.swap(index, last);
        self.timeout_ats.swap(index, last);
        self.states.swap(index, last);
        self.transfers.swap(index, last);
        self.part_names.swap(index, last);
        self.part_flags.swap(index, last);
        self.streamed_opens.swap(index, last);
        self.states[last] = State::default();
        self.streamed_opens[last] = Default::default();
        self.len = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Table = FixedHeapResourceTable<u8, 2, 1024, 3>;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }
    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }
    fn shape() -> ResourceBufferShape {
        ResourceBufferShape::try_for_transfer(1024, 464).unwrap()
    }

    fn sized_shape(transfer_bytes: usize, sdu: usize) -> ResourceBufferShape {
        ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).unwrap()
    }

    #[test]
    fn push_then_read_back_and_seal_into_the_boxed_transfer() {
        let mut table = Table::default();
        assert_eq!(table.capacity(), 2);
        assert_eq!(table.transfer_capacity(), 1024);
        assert_eq!(table.active_buffer_bytes(), 0);
        assert_eq!(table.buffer_memory_limit(), 2 * (1024 + 3 * 5));

        let i = table.push(link(1), hash(0xA1), 7, shape()).unwrap();
        assert_eq!(table.active_buffer_bytes(), 1024 + 3 * 5);
        table.set_hash(i, hash(0xB2));
        let buffers = table.buffers_mut(i);
        buffers.transfer[..4].copy_from_slice(&[1, 2, 3, 4]);
        buffers.part_flags[0] = true;

        assert_eq!(table.len(), 1);
        assert_eq!(table.hashes(), &[hash(0xB2)]);
        assert_eq!(&table.transfer(i)[..4], &[1, 2, 3, 4]);
        assert!(table.part_flags(i)[0]);
        assert_eq!(table.states(), &[7]);
    }

    #[test]
    fn a_full_table_refuses_the_next_push() {
        let mut table = Table::default();
        table.push(link(1), hash(1), 1, shape()).unwrap();
        table.push(link(2), hash(2), 2, shape()).unwrap();
        assert_eq!(
            table.push(link(3), hash(3), 3, shape()),
            Err(ResourceTablePushError::TableFull)
        );
    }

    #[test]
    fn swap_remove_moves_the_last_slot_and_clears_its_state() {
        let mut table = Table::default();
        let a = table.push(link(1), hash(1), 11, shape()).unwrap();
        table.push(link(2), hash(2), 22, shape()).unwrap();
        table.buffers_mut(a).transfer[0] = 0xEE;

        table.swap_remove(a);

        assert_eq!(table.len(), 1);
        assert_eq!(table.states(), &[22]);
        assert_eq!(table.link_ids(), &[link(2)]);
    }

    #[test]
    fn requested_shape_must_fit_the_fixed_psram_regions() {
        let mut table = Table::default();
        assert_eq!(
            table.push(link(1), hash(1), 1, sized_shape(1025, 464),),
            Err(ResourceTablePushError::TransferTooLarge),
        );
        assert_eq!(
            table.push(link(1), hash(1), 1, sized_shape(1024, 256),),
            Err(ResourceTablePushError::TooManyParts),
        );
    }
}
