//! The growable, std/alloc channel store: the channel table and each channel's
//! reorder buffer are `Vec`s that grow with the network — `capacity()` is
//! `usize::MAX`, so `ensure` never fails and the reorder buffer never overflows
//! (RNS's own unbounded deque). The typical host backend.

use alloc::vec::Vec;

use crate::routing::links::channel::columns::{BufferOutcome, ChannelColumns, EnsureChannelError};
use crate::routing::links::channel::{ChannelSequence, MessageType};
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
struct ReorderBuffer {
    sequences: Vec<ChannelSequence>,
    message_types: Vec<MessageType>,
    payloads: Vec<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct HeapChannelColumns {
    link_ids: Vec<LinkId>,
    next_expected: Vec<ChannelSequence>,
    buffers: Vec<ReorderBuffer>,
}

impl ChannelColumns for HeapChannelColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn index_of(&self, link: &LinkId) -> Option<usize> {
        self.link_ids.iter().position(|id| id == link)
    }

    fn ensure(&mut self, link: &LinkId) -> Result<usize, EnsureChannelError> {
        if let Some(index) = self.index_of(link) {
            return Ok(index);
        }
        self.link_ids.push(*link);
        self.next_expected.push(ChannelSequence(0));
        self.buffers.push(ReorderBuffer::default());
        Ok(self.link_ids.len() - 1)
    }

    fn close(&mut self, link: &LinkId) {
        if let Some(index) = self.index_of(link) {
            self.link_ids.swap_remove(index);
            self.next_expected.swap_remove(index);
            self.buffers.swap_remove(index);
        }
    }

    fn next_expected(&self, index: usize) -> ChannelSequence {
        self.next_expected[index]
    }
    fn set_next_expected(&mut self, index: usize, sequence: ChannelSequence) {
        self.next_expected[index] = sequence;
    }

    fn buffered_sequences(&self, index: usize) -> &[ChannelSequence] {
        &self.buffers[index].sequences
    }
    fn buffered_message_type(&self, index: usize, sub: usize) -> MessageType {
        self.buffers[index].message_types[sub]
    }
    fn buffered_payload(&self, index: usize, sub: usize) -> &[u8] {
        &self.buffers[index].payloads[sub]
    }

    fn push_buffered(
        &mut self,
        index: usize,
        sequence: ChannelSequence,
        message_type: MessageType,
        payload: &[u8],
    ) -> BufferOutcome {
        let buffer = &mut self.buffers[index];
        buffer.sequences.push(sequence);
        buffer.message_types.push(message_type);
        buffer.payloads.push(payload.to_vec());
        BufferOutcome::Stored
    }

    fn swap_remove_buffered(&mut self, index: usize, sub: usize) {
        let buffer = &mut self.buffers[index];
        buffer.sequences.swap_remove(sub);
        buffer.message_types.swap_remove(sub);
        buffer.payloads.swap_remove(sub);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    #[test]
    fn the_table_grows_without_a_ceiling() {
        let mut columns = HeapChannelColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);
        for n in 0..100u8 {
            columns.ensure(&link(n)).unwrap();
        }
        assert_eq!(columns.len(), 100);
        // re-ensuring a known link returns its existing slot
        let again = columns.ensure(&link(7)).unwrap();
        assert_eq!(columns.index_of(&link(7)), Some(again));
    }

    #[test]
    fn the_reorder_buffer_grows_and_never_reports_full() {
        let mut columns = HeapChannelColumns::default();
        let i = columns.ensure(&link(1)).unwrap();
        for n in 0..200u16 {
            assert_eq!(
                columns.push_buffered(i, ChannelSequence(n), MessageType(n), b"x"),
                BufferOutcome::Stored
            );
        }
        assert_eq!(columns.buffered_sequences(i).len(), 200);
    }

    #[test]
    fn buffered_entries_round_trip_and_swap_remove() {
        let mut columns = HeapChannelColumns::default();
        let i = columns.ensure(&link(1)).unwrap();
        columns.push_buffered(i, ChannelSequence(5), MessageType(0x07), b"five");
        columns.push_buffered(i, ChannelSequence(6), MessageType(0x08), b"six");

        let sub = columns
            .buffered_sequences(i)
            .iter()
            .position(|s| *s == ChannelSequence(5))
            .unwrap();
        assert_eq!(columns.buffered_message_type(i, sub), MessageType(0x07));
        assert_eq!(columns.buffered_payload(i, sub), b"five");
        columns.swap_remove_buffered(i, sub);
        assert_eq!(columns.buffered_sequences(i), &[ChannelSequence(6)]);
    }

    #[test]
    fn close_frees_the_slot() {
        let mut columns = HeapChannelColumns::default();
        columns.ensure(&link(1)).unwrap();
        let b = columns.ensure(&link(2)).unwrap();
        columns.set_next_expected(b, ChannelSequence(42));
        columns.close(&link(1));
        assert_eq!(columns.len(), 1);
        let b = columns.index_of(&link(2)).unwrap();
        assert_eq!(columns.next_expected(b), ChannelSequence(42));
        assert_eq!(columns.index_of(&link(1)), None);
    }
}
