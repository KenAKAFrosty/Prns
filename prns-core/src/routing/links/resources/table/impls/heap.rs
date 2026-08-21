use alloc::vec;
use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::links::resources::table::{
    ResourceBuffers, ResourceRowState, ResourceTable, ResourceTableAdmission,
    ResourceTablePushError,
};
use crate::routing::links::resources::{
    checked_resource_buffer_bytes, max_part_count, sealed_transfer_bytes, ResourceBufferShape,
    ResourceHash, DEFAULT_RESOURCE_MEMORY_BYTES, MAP_HASH_LEN, MAX_EFFICIENT_SIZE,
};
use crate::routing::links::LinkId;

/// A deliberate bound where RNS 1.4.2 grows `Link.outgoing_resources` and `incoming_resources` without limit.
/// Overflow refuses by name on both faces: `SendResourceRejection::TableFull` going out, `IgnoreReason::CapacityExhausted` coming in.
pub const DEFAULT_MAX_RESOURCES: usize = 4_096;

/// Heap table for a std host: each active row owns only the transfer, part-name,
/// and part-state lengths required by that Resource. Removing a row drops those
/// bulk buffers immediately.
#[derive(Debug)]
pub struct HeapResourceTable<State: ResourceRowState> {
    link_ids: Vec<LinkId>,
    hashes: Vec<ResourceHash>,
    timeout_ats: Vec<Option<InstantMillis>>,
    states: Vec<State>,
    transfers: Vec<Vec<u8>>,
    part_names: Vec<Vec<[u8; MAP_HASH_LEN]>>,
    part_flags: Vec<Vec<bool>>,
    streamed_opens: Vec<State::StreamedOpenSlot>,
    active_buffer_bytes: usize,
    memory_limit: usize,
}

const HEAP_TRANSFER_CAPACITY: usize = sealed_transfer_bytes(MAX_EFFICIENT_SIZE);
const HEAP_PART_CAPACITY: usize = max_part_count(HEAP_TRANSFER_CAPACITY);

impl<State: ResourceRowState> Default for HeapResourceTable<State> {
    fn default() -> Self {
        Self {
            link_ids: Vec::new(),
            hashes: Vec::new(),
            timeout_ats: Vec::new(),
            states: Vec::new(),
            transfers: Vec::new(),
            part_names: Vec::new(),
            part_flags: Vec::new(),
            streamed_opens: Vec::new(),
            active_buffer_bytes: 0,
            memory_limit: DEFAULT_RESOURCE_MEMORY_BYTES,
        }
    }
}

impl<State: ResourceRowState> HeapResourceTable<State> {
    #[must_use]
    pub(crate) const fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    pub(crate) fn set_memory_limit(&mut self, memory_limit: usize) {
        self.memory_limit = memory_limit;
    }
}

impl<State: ResourceRowState + Default> ResourceTable<State> for HeapResourceTable<State> {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_RESOURCES
    }
    fn transfer_capacity(&self) -> usize {
        HEAP_TRANSFER_CAPACITY
    }
    fn part_capacity(&self) -> usize {
        HEAP_PART_CAPACITY
    }
    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn active_buffer_bytes(&self) -> usize {
        self.active_buffer_bytes
    }

    fn buffer_memory_limit(&self) -> usize {
        self.memory_limit
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids
    }
    fn hashes(&self) -> &[ResourceHash] {
        &self.hashes
    }
    fn timeout_ats(&self) -> &[Option<InstantMillis>] {
        &self.timeout_ats
    }
    fn states(&self) -> &[State] {
        &self.states
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
        if shape.transfer_bytes() > HEAP_TRANSFER_CAPACITY
            || shape.part_count() > HEAP_PART_CAPACITY
            || shape.buffer_bytes() > self.memory_limit
        {
            ResourceTableAdmission::Impossible
        } else if self.len() >= self.capacity()
            || shape.buffer_bytes() > self.memory_limit.saturating_sub(self.active_buffer_bytes)
        {
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
        if shape.transfer_bytes() > HEAP_TRANSFER_CAPACITY {
            return Err(ResourceTablePushError::TransferTooLarge);
        }
        if shape.part_count() > HEAP_PART_CAPACITY {
            return Err(ResourceTablePushError::TooManyParts);
        }
        if self.len() >= self.capacity() {
            return Err(ResourceTablePushError::TableFull);
        }
        let row_bytes = shape.buffer_bytes();
        if row_bytes > self.memory_limit.saturating_sub(self.active_buffer_bytes) {
            return Err(ResourceTablePushError::MemoryLimit);
        }
        self.link_ids.push(link_id);
        self.hashes.push(hash);
        self.timeout_ats.push(None);
        self.states.push(state);
        self.transfers.push(vec![0u8; shape.transfer_bytes()]);
        self.part_names
            .push(vec![[0u8; MAP_HASH_LEN]; shape.part_count()]);
        self.part_flags.push(vec![false; shape.part_count()]);
        self.streamed_opens.push(Default::default());
        self.active_buffer_bytes += row_bytes;
        Ok(self.link_ids.len() - 1)
    }

    fn swap_remove(&mut self, index: usize) {
        debug_assert_eq!(self.part_names[index].len(), self.part_flags[index].len());
        let Some(row_bytes) = checked_resource_buffer_bytes(
            self.transfers[index].len(),
            self.part_names[index].len(),
        ) else {
            unreachable!("stored Resource buffers were validated on insertion")
        };
        self.link_ids.swap_remove(index);
        self.hashes.swap_remove(index);
        self.timeout_ats.swap_remove(index);
        self.states.swap_remove(index);
        self.streamed_opens.swap_remove(index);
        self.transfers.swap_remove(index);
        self.part_names.swap_remove(index);
        self.part_flags.swap_remove(index);
        self.active_buffer_bytes -= row_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(transfer_bytes: usize, sdu: usize) -> ResourceBufferShape {
        ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).unwrap()
    }

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn hash(byte: u8) -> ResourceHash {
        ResourceHash::new([byte; 32])
    }

    #[test]
    fn rows_allocate_their_exact_shapes_and_removal_drops_bulk_buffers() {
        let mut table = HeapResourceTable::<u8>::default();
        assert_eq!(table.active_buffer_bytes(), 0);
        assert_eq!(table.buffer_memory_limit(), DEFAULT_RESOURCE_MEMORY_BYTES);
        let first = table.push(link(1), hash(1), 11, shape(144, 464)).unwrap();
        assert_eq!(table.transfer(first).len(), 144);
        assert_eq!(table.part_names(first).len(), 1);
        assert_eq!(table.part_flags(first).len(), 1);
        assert_eq!(table.active_buffer_bytes, 149);
        assert_eq!(table.active_buffer_bytes(), 149);
        table.buffers_mut(first).part_flags[0] = true;

        table.swap_remove(first);
        assert!(table.transfers.is_empty());
        assert!(table.part_names.is_empty());
        assert!(table.part_flags.is_empty());
        assert_eq!(table.active_buffer_bytes, 0);

        let second = table.push(link(2), hash(2), 22, shape(1_568, 464)).unwrap();

        assert_eq!(table.transfer(second).len(), 1_568);
        assert_eq!(table.part_names(second).len(), 4);
        assert_eq!(table.part_flags(second).len(), 4);
        assert_eq!(table.active_buffer_bytes, 1_588);
        assert!(!table.part_flags(second).iter().any(|flag| *flag));
        assert_eq!(table.states(), &[22]);
    }

    #[test]
    fn small_rows_replace_the_former_maximum_sized_bulk_payload() {
        let former_row_payload_bytes = HEAP_TRANSFER_CAPACITY
            + HEAP_PART_CAPACITY * MAP_HASH_LEN
            + HEAP_PART_CAPACITY * core::mem::size_of::<bool>();
        assert_eq!(former_row_payload_bytes, 1_059_940);

        let compressed_row_payload_bytes = 144 + MAP_HASH_LEN + core::mem::size_of::<bool>();
        let four_part_row_payload_bytes =
            1_568 + 4 * MAP_HASH_LEN + 4 * core::mem::size_of::<bool>();
        assert_eq!(compressed_row_payload_bytes, 149);
        assert_eq!(four_part_row_payload_bytes, 1_588);
        assert_eq!(64 * former_row_payload_bytes, 67_836_160);
        assert_eq!(64 * compressed_row_payload_bytes, 9_536);
        assert_eq!(
            DEFAULT_MAX_RESOURCES * compressed_row_payload_bytes,
            610_304
        );
    }

    #[test]
    fn the_default_budget_is_per_direction_and_rows_have_a_separate_guard() {
        let mut table = HeapResourceTable::<u8>::default();
        assert_eq!(table.memory_limit(), 64 * 1024 * 1024);
        assert_eq!(table.capacity(), 4_096);
        assert_eq!(table.active_buffer_bytes, 0);

        for row in 0..DEFAULT_MAX_RESOURCES {
            let byte = row as u8;
            table
                .push(link(byte), hash(byte), byte, shape(1, 1))
                .unwrap();
        }
        assert_eq!(table.len(), 4_096);
        assert_eq!(table.active_buffer_bytes, 4_096 * 6);
        assert_eq!(
            table.push(link(0), hash(0), 0, shape(1, 1)),
            Err(ResourceTablePushError::TableFull),
        );
    }

    #[test]
    fn the_budget_accepts_its_exact_boundary_and_releases_on_removal() {
        let mut table = HeapResourceTable::<u8>::default();
        table.set_memory_limit(2 * 149);
        let first = table.push(link(1), hash(1), 11, shape(144, 464)).unwrap();
        table.push(link(2), hash(2), 22, shape(144, 464)).unwrap();
        assert_eq!(table.active_buffer_bytes, 298);
        assert_eq!(
            table.push(link(3), hash(3), 33, shape(144, 464)),
            Err(ResourceTablePushError::MemoryLimit),
        );

        table.swap_remove(first);
        assert_eq!(table.active_buffer_bytes, 149);
        table.push(link(3), hash(3), 33, shape(144, 464)).unwrap();
        assert_eq!(table.active_buffer_bytes, 298);
    }

    #[test]
    fn admission_distinguishes_live_pressure_from_an_impossible_row() {
        let mut table = HeapResourceTable::<u8>::default();
        table.set_memory_limit(298);
        assert_eq!(
            table.admission_for_shape(shape(144, 464)),
            ResourceTableAdmission::Available,
        );
        table.push(link(1), hash(1), 11, shape(144, 464)).unwrap();
        table.push(link(2), hash(2), 22, shape(144, 464)).unwrap();
        assert_eq!(
            table.admission_for_shape(shape(144, 464)),
            ResourceTableAdmission::TemporarilyFull,
        );
        assert_eq!(
            table.admission_for_shape(shape(300, 464)),
            ResourceTableAdmission::Impossible,
        );
    }

    #[test]
    fn zero_budget_disables_rows_without_allocating() {
        let mut table = HeapResourceTable::<u8>::default();
        table.set_memory_limit(0);
        assert_eq!(
            table.push(link(1), hash(1), 11, shape(144, 464)),
            Err(ResourceTablePushError::MemoryLimit),
        );
        assert!(table.is_empty());
        assert_eq!(table.active_buffer_bytes, 0);
    }

    #[test]
    fn lowering_a_budget_keeps_live_rows_but_blocks_replacement_until_headroom_returns() {
        let mut table = HeapResourceTable::<u8>::default();
        table.set_memory_limit(298);
        table.push(link(1), hash(1), 11, shape(144, 464)).unwrap();
        table.push(link(2), hash(2), 22, shape(144, 464)).unwrap();

        table.set_memory_limit(149);
        assert_eq!(table.active_buffer_bytes, 298);
        assert_eq!(
            table.push(link(3), hash(3), 33, shape(144, 464)),
            Err(ResourceTablePushError::MemoryLimit),
        );
        table.swap_remove(0);
        assert_eq!(table.active_buffer_bytes, 149);
        assert_eq!(
            table.push(link(3), hash(3), 33, shape(144, 464)),
            Err(ResourceTablePushError::MemoryLimit),
        );
        table.swap_remove(0);
        table.push(link(3), hash(3), 33, shape(144, 464)).unwrap();
        assert_eq!(table.active_buffer_bytes, 149);
    }

    #[test]
    fn row_shapes_cannot_exceed_protocol_capacities() {
        let mut table = HeapResourceTable::<u8>::default();
        assert_eq!(
            table.push(link(1), hash(1), 11, shape(HEAP_TRANSFER_CAPACITY + 1, 464),),
            Err(ResourceTablePushError::TransferTooLarge),
        );
        assert_eq!(
            table.push(link(1), hash(1), 11, shape(HEAP_TRANSFER_CAPACITY, 1),),
            Err(ResourceTablePushError::TooManyParts),
        );
        assert!(table.is_empty());
    }
}
