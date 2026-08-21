use crate::engine::InstantMillis;
use crate::routing::links::resources::table::{
    ResourceBuffers, ResourceRowState, ResourceTable, ResourceTableAdmission,
    ResourceTablePushError,
};
use crate::routing::links::resources::{
    max_part_count, ResourceBufferShape, ResourceHash, MAP_HASH_LEN,
};
use crate::routing::links::LinkId;

/// Inline table for a no_std target: every byte the slots can hold lives in the struct, sized where the storage recipe is assembled. `MAX_PARTS` must cover `TRANSFER_BYTES` at the broadcast-MTU sdu; the constructor proves it at compile time.
#[derive(Debug)]
pub struct FixedResourceTable<
    State: ResourceRowState,
    const SLOTS: usize,
    const TRANSFER_BYTES: usize,
    const MAX_PARTS: usize,
> {
    len: usize,
    link_ids: [LinkId; SLOTS],
    hashes: [ResourceHash; SLOTS],
    timeout_ats: [Option<InstantMillis>; SLOTS],
    states: [State; SLOTS],
    transfers: [[u8; TRANSFER_BYTES]; SLOTS],
    part_names: [[[u8; MAP_HASH_LEN]; MAX_PARTS]; SLOTS],
    part_flags: [[bool; MAX_PARTS]; SLOTS],
    streamed_opens: [State::StreamedOpenSlot; SLOTS],
}

impl<
        State: ResourceRowState + Default,
        const SLOTS: usize,
        const TRANSFER_BYTES: usize,
        const MAX_PARTS: usize,
    > Default for FixedResourceTable<State, SLOTS, TRANSFER_BYTES, MAX_PARTS>
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
            transfers: [[0u8; TRANSFER_BYTES]; SLOTS],
            part_names: [[[0u8; MAP_HASH_LEN]; MAX_PARTS]; SLOTS],
            part_flags: [[false; MAX_PARTS]; SLOTS],
            streamed_opens: core::array::from_fn(|_| Default::default()),
        }
    }
}

impl<
        State: ResourceRowState + Default,
        const SLOTS: usize,
        const TRANSFER_BYTES: usize,
        const MAX_PARTS: usize,
    > ResourceTable<State> for FixedResourceTable<State, SLOTS, TRANSFER_BYTES, MAX_PARTS>
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
        &self.transfers[index]
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
            transfer: &mut self.transfers[index],
            part_names: &mut self.part_names[index],
            part_flags: &mut self.part_flags[index],
        }
    }
    fn transfer_and_streamed_open_mut(
        &mut self,
        index: usize,
    ) -> (&mut [u8], &mut State::StreamedOpenSlot) {
        (&mut self.transfers[index], &mut self.streamed_opens[index])
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

    type Table = FixedResourceTable<u8, 2, 1024, 3>;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    fn shape(transfer_bytes: usize, sdu: usize) -> ResourceBufferShape {
        ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).unwrap()
    }

    #[test]
    fn requested_shape_is_validated_without_changing_inline_regions() {
        let mut table = Table::default();
        assert_eq!(table.active_buffer_bytes(), 0);
        assert_eq!(table.buffer_memory_limit(), 2 * (1024 + 3 * 5));
        let index = table.push(link(1), hash(1), 7, shape(144, 464)).unwrap();

        assert_eq!(table.active_buffer_bytes(), 1024 + 3 * 5);
        assert_eq!(table.transfer(index).len(), 1024);
        assert_eq!(table.part_names(index).len(), 3);
        assert_eq!(table.part_flags(index).len(), 3);
        assert_eq!(
            table.push(link(2), hash(2), 8, shape(1025, 464),),
            Err(ResourceTablePushError::TransferTooLarge),
        );
        assert_eq!(
            table.push(link(2), hash(2), 8, shape(1024, 256),),
            Err(ResourceTablePushError::TooManyParts),
        );
    }
}
