//! The growable std/alloc channel store: `capacity()` is `usize::MAX`, so `ensure` effectively never fails and the reorder buffer effectively never overflows (RNS's own unbounded deque).

use alloc::vec::Vec;

use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::table::{
    BufferOutcome, ChannelTable, EnsureChannelError, OutstandingSend, OutstandingTimeoutChange,
    TxOutcome,
};
use crate::routing::links::channel::{ChannelSequence, ChannelWindow, MessageType};
use crate::routing::links::LinkId;
#[cfg(feature = "std")]
use crate::routing::temporal_index::HeapDeadlineIndex;

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
pub struct HeapChannelTable {
    link_ids: Vec<LinkId>,
    next_expected: Vec<ChannelSequence>,
    buffers: Vec<ReorderBuffer>,
    next_tx_sequence: Vec<ChannelSequence>,
    windows: Vec<ChannelWindow>,
    outstanding: Vec<OutstandingRing>,
    channel_earliest_tx_timeouts: Vec<Option<InstantMillis>>,
    index: HeapLemireIndex,
    #[cfg(feature = "std")]
    timeout_index: HeapDeadlineIndex,
}

impl ChannelTable for HeapChannelTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.link_ids.len()
    }

    fn index_of(&self, link: &LinkId) -> Option<usize> {
        self.index.get(link, &self.link_ids)
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
        self.channel_earliest_tx_timeouts.push(None);
        let row = self.link_ids.len() - 1;
        self.index.insert(row, &self.link_ids);
        #[cfg(feature = "std")]
        {
            let deadlines = &self.channel_earliest_tx_timeouts;
            self.timeout_index
                .insert(row, None, |row| deadlines.get(row).copied().flatten());
        }
        Ok(row)
    }

    fn close(&mut self, link: &LinkId) {
        if let Some(index) = self.index_of(link) {
            let last = self.link_ids.len() - 1;
            self.index.remove_slot(index, &self.link_ids);
            if index != last {
                self.index.repoint_slot(last, index, &self.link_ids);
            }
            #[cfg(feature = "std")]
            {
                let deadlines = &self.channel_earliest_tx_timeouts;
                self.timeout_index
                    .swap_remove(index, last, |row| deadlines.get(row).copied().flatten());
            }
            self.link_ids.swap_remove(index);
            self.next_expected.swap_remove(index);
            self.buffers.swap_remove(index);
            self.next_tx_sequence.swap_remove(index);
            self.windows.swap_remove(index);
            self.outstanding.swap_remove(index);
            self.channel_earliest_tx_timeouts.swap_remove(index);
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
        let previous = self.outstanding[index].timeout_ats[sub];
        self.outstanding[index].timeout_ats[sub] = timeout_at;
        self.absorb_outstanding_timeout_change(
            index,
            OutstandingTimeoutChange::Rewritten {
                previous,
                new: timeout_at,
            },
        );
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
        self.absorb_outstanding_timeout_change(
            index,
            OutstandingTimeoutChange::Pushed(send.timeout_at),
        );
        TxOutcome::Tracked
    }

    fn retire_outstanding(&mut self, index: usize, sub: usize) {
        let ring = &mut self.outstanding[index];
        let retired = ring.timeout_ats[sub];
        ring.packet_hashes.swap_remove(sub);
        ring.command_ids.swap_remove(sub);
        ring.sent_ats.swap_remove(sub);
        ring.timeout_ats.swap_remove(sub);
        ring.tries.swap_remove(sub);
        ring.sequences.swap_remove(sub);
        ring.message_types.swap_remove(sub);
        ring.bodies.swap_remove(sub);
        ring.ivs.swap_remove(sub);
        self.absorb_outstanding_timeout_change(index, OutstandingTimeoutChange::Retired(retired));
    }

    fn channel_earliest_tx_timeout(&self, index: usize) -> Option<InstantMillis> {
        self.channel_earliest_tx_timeouts[index]
    }
    fn set_channel_earliest_tx_timeout(&mut self, index: usize, earliest: Option<InstantMillis>) {
        self.channel_earliest_tx_timeouts[index] = earliest;
        #[cfg(feature = "std")]
        {
            let deadlines = &self.channel_earliest_tx_timeouts;
            self.timeout_index
                .update(index, earliest, |row| deadlines.get(row).copied().flatten());
        }
    }

    fn earliest_tx_timeout_at(&self) -> Option<InstantMillis> {
        #[cfg(feature = "std")]
        {
            let earliest = self.timeout_index.eager_earliest_exact(
                self.channel_earliest_tx_timeouts.len(),
                |row| {
                    self.channel_earliest_tx_timeouts
                        .get(row)
                        .copied()
                        .flatten()
                },
            );
            debug_assert_eq!(earliest, self.scan_earliest_tx_timeout());
            earliest
        }
        #[cfg(not(feature = "std"))]
        {
            let earliest = self
                .channel_earliest_tx_timeouts
                .iter()
                .flatten()
                .min()
                .copied();
            debug_assert_eq!(earliest, self.scan_earliest_tx_timeout());
            earliest
        }
    }

    fn first_due_channel(&self, now: InstantMillis) -> Option<usize> {
        #[cfg(feature = "std")]
        {
            self.timeout_index.eager_first_due(
                self.channel_earliest_tx_timeouts.len(),
                now,
                |row| {
                    self.channel_earliest_tx_timeouts
                        .get(row)
                        .copied()
                        .flatten()
                },
            )
        }
        #[cfg(not(feature = "std"))]
        self.channel_earliest_tx_timeouts
            .iter()
            .position(|deadline| deadline.is_some_and(|at| at <= now))
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
        let mut table = HeapChannelTable::default();
        assert_eq!(table.capacity(), usize::MAX);
        for n in 0..100u8 {
            table.ensure(&link(n)).unwrap();
        }
        assert_eq!(table.len(), 100);
        let again = table.ensure(&link(7)).unwrap();
        assert_eq!(table.index_of(&link(7)), Some(again));
    }

    #[test]
    fn the_reorder_buffer_grows_and_never_reports_full() {
        let mut table = HeapChannelTable::default();
        let i = table.ensure(&link(1)).unwrap();
        for n in 0..200u16 {
            assert_eq!(
                table.push_buffered(i, ChannelSequence(n), MessageType(n), b"x"),
                BufferOutcome::Stored
            );
        }
        assert_eq!(table.buffered_sequences(i).len(), 200);
    }

    #[test]
    fn buffered_entries_round_trip_and_swap_remove() {
        let mut table = HeapChannelTable::default();
        let i = table.ensure(&link(1)).unwrap();
        table.push_buffered(i, ChannelSequence(5), MessageType(0x07), b"five");
        table.push_buffered(i, ChannelSequence(6), MessageType(0x08), b"six");

        let sub = table
            .buffered_sequences(i)
            .iter()
            .position(|s| *s == ChannelSequence(5))
            .unwrap();
        assert_eq!(table.buffered_message_type(i, sub), MessageType(0x07));
        assert_eq!(table.buffered_payload(i, sub), b"five");
        table.swap_remove_buffered(i, sub);
        assert_eq!(table.buffered_sequences(i), &[ChannelSequence(6)]);
    }

    #[test]
    fn close_frees_the_slot() {
        let mut table = HeapChannelTable::default();
        table.ensure(&link(1)).unwrap();
        let b = table.ensure(&link(2)).unwrap();
        table.set_next_expected(b, ChannelSequence(42));
        table.close(&link(1));
        assert_eq!(table.len(), 1);
        let b = table.index_of(&link(2)).unwrap();
        assert_eq!(table.next_expected(b), ChannelSequence(42));
        assert_eq!(table.index_of(&link(1)), None);
    }

    #[cfg(feature = "std")]
    #[test]
    fn deadline_and_link_indexes_follow_cache_updates_and_row_moves() {
        let mut table = HeapChannelTable::default();
        let a = table.ensure(&link(1)).unwrap();
        let b = table.ensure(&link(2)).unwrap();
        let c = table.ensure(&link(3)).unwrap();
        table.push_outstanding(a, outstanding(1, 100));
        table.push_outstanding(b, outstanding(2, 50));
        table.push_outstanding(c, outstanding(3, 200));

        assert_eq!(table.earliest_tx_timeout_at(), Some(InstantMillis(1_500)));
        assert_eq!(table.first_due_channel(InstantMillis(1_499)), None);
        assert_eq!(table.first_due_channel(InstantMillis(1_500)), Some(b));

        table.set_outstanding_timeout_at(b, 0, InstantMillis(4_000));
        assert_eq!(table.earliest_tx_timeout_at(), Some(InstantMillis(2_000)));
        table.close(&link(1));
        assert_eq!(table.index_of(&link(3)), Some(0));
        assert_eq!(table.earliest_tx_timeout_at(), Some(InstantMillis(3_000)));
    }

    #[cfg(feature = "std")]
    fn outstanding(byte: u8, command: u64) -> OutstandingSend<'static> {
        OutstandingSend {
            packet_hash: PacketHash::new([byte; 32]),
            command_id: CommandId(command),
            sequence: ChannelSequence(u16::from(byte)),
            message_type: MessageType(0x07),
            body: b"body",
            iv: [byte; 16],
            sent_at: InstantMillis(command * 10),
            timeout_at: InstantMillis(command * 10 + 1_000),
        }
    }
}
