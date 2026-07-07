//! The growable std/alloc channel store: `capacity()` is `usize::MAX`, so `ensure` never
//! fails and the reorder buffer never overflows (RNS's own unbounded deque).

use alloc::vec::Vec;

use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::columns::{
    BufferOutcome, ChannelColumns, EnsureChannelError, OutstandingSend, TxOutcome,
};
use crate::routing::links::channel::{ChannelSequence, ChannelWindow, MessageType};
use crate::routing::links::LinkId;

#[derive(Debug, Default)]
struct ReorderBuffer {
    sequences: Vec<ChannelSequence>,
    message_types: Vec<MessageType>,
    payloads: Vec<Vec<u8>>,
}

#[derive(Debug, Default)]
struct OutstandingRing {
    packet_hashes: Vec<PacketHash>,
    command_ids: Vec<CommandId>,
    sent_ats: Vec<InstantMillis>,
    timeout_ats: Vec<InstantMillis>,
    tries: Vec<u8>,
    sequences: Vec<ChannelSequence>,
    message_types: Vec<MessageType>,
    bodies: Vec<Vec<u8>>,
    ivs: Vec<[u8; 16]>,
}

#[derive(Debug, Default)]
pub struct HeapChannelColumns {
    link_ids: Vec<LinkId>,
    next_expected: Vec<ChannelSequence>,
    buffers: Vec<ReorderBuffer>,
    next_tx_sequence: Vec<ChannelSequence>,
    windows: Vec<ChannelWindow>,
    outstanding: Vec<OutstandingRing>,
    earliest_tx_timeout: Option<InstantMillis>,
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
    fn link_at(&self, index: usize) -> LinkId {
        self.link_ids[index]
    }

    fn ensure(&mut self, link: &LinkId) -> Result<usize, EnsureChannelError> {
        if let Some(index) = self.index_of(link) {
            return Ok(index);
        }
        self.link_ids.push(*link);
        self.next_expected.push(ChannelSequence(0));
        self.buffers.push(ReorderBuffer::default());
        self.next_tx_sequence.push(ChannelSequence(0));
        self.windows.push(ChannelWindow::default());
        self.outstanding.push(OutstandingRing::default());
        Ok(self.link_ids.len() - 1)
    }

    fn close(&mut self, link: &LinkId) {
        if let Some(index) = self.index_of(link) {
            self.link_ids.swap_remove(index);
            self.next_expected.swap_remove(index);
            self.buffers.swap_remove(index);
            self.next_tx_sequence.swap_remove(index);
            self.windows.swap_remove(index);
            self.outstanding.swap_remove(index);
        }
        self.earliest_tx_timeout = self.scan_earliest_tx_timeout();
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

    fn next_tx_sequence(&self, index: usize) -> ChannelSequence {
        self.next_tx_sequence[index]
    }
    fn set_next_tx_sequence(&mut self, index: usize, sequence: ChannelSequence) {
        self.next_tx_sequence[index] = sequence;
    }

    fn window(&self, index: usize) -> ChannelWindow {
        self.windows[index]
    }
    fn set_window(&mut self, index: usize, window: ChannelWindow) {
        self.windows[index] = window;
    }

    fn outstanding_count(&self, index: usize) -> usize {
        self.outstanding[index].packet_hashes.len()
    }
    fn outstanding_packet_hashes(&self, index: usize) -> &[PacketHash] {
        &self.outstanding[index].packet_hashes
    }
    fn outstanding_command_id(&self, index: usize, sub: usize) -> CommandId {
        self.outstanding[index].command_ids[sub]
    }
    fn outstanding_sent_at(&self, index: usize, sub: usize) -> InstantMillis {
        self.outstanding[index].sent_ats[sub]
    }
    fn outstanding_timeout_at(&self, index: usize, sub: usize) -> InstantMillis {
        self.outstanding[index].timeout_ats[sub]
    }
    fn set_outstanding_timeout_at(&mut self, index: usize, sub: usize, timeout_at: InstantMillis) {
        self.outstanding[index].timeout_ats[sub] = timeout_at;
        self.earliest_tx_timeout = self.scan_earliest_tx_timeout();
    }
    fn outstanding_tries(&self, index: usize, sub: usize) -> u8 {
        self.outstanding[index].tries[sub]
    }
    fn set_outstanding_tries(&mut self, index: usize, sub: usize, tries: u8) {
        self.outstanding[index].tries[sub] = tries;
    }
    fn outstanding_sequence(&self, index: usize, sub: usize) -> ChannelSequence {
        self.outstanding[index].sequences[sub]
    }
    fn outstanding_message_type(&self, index: usize, sub: usize) -> MessageType {
        self.outstanding[index].message_types[sub]
    }
    fn outstanding_body(&self, index: usize, sub: usize) -> &[u8] {
        &self.outstanding[index].bodies[sub]
    }
    fn outstanding_iv(&self, index: usize, sub: usize) -> [u8; 16] {
        self.outstanding[index].ivs[sub]
    }

    fn push_outstanding(&mut self, index: usize, send: OutstandingSend<'_>) -> TxOutcome {
        let ring = &mut self.outstanding[index];
        ring.packet_hashes.push(send.packet_hash);
        ring.command_ids.push(send.command_id);
        ring.sent_ats.push(send.sent_at);
        ring.timeout_ats.push(send.timeout_at);
        ring.tries.push(0);
        ring.sequences.push(send.sequence);
        ring.message_types.push(send.message_type);
        ring.bodies.push(send.body.to_vec());
        ring.ivs.push(send.iv);
        self.earliest_tx_timeout = self.scan_earliest_tx_timeout();
        TxOutcome::Tracked
    }

    fn retire_outstanding(&mut self, index: usize, sub: usize) {
        let ring = &mut self.outstanding[index];
        ring.packet_hashes.swap_remove(sub);
        ring.command_ids.swap_remove(sub);
        ring.sent_ats.swap_remove(sub);
        ring.timeout_ats.swap_remove(sub);
        ring.tries.swap_remove(sub);
        ring.sequences.swap_remove(sub);
        ring.message_types.swap_remove(sub);
        ring.bodies.swap_remove(sub);
        ring.ivs.swap_remove(sub);
        self.earliest_tx_timeout = self.scan_earliest_tx_timeout();
    }

    fn earliest_tx_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_tx_timeout,
            self.scan_earliest_tx_timeout(),
            "earliest_tx_timeout cache desynced from the outstanding timeouts"
        );
        self.earliest_tx_timeout
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
