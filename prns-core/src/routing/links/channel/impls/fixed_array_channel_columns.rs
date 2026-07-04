//! The fully-inline, no-alloc channel store. Size `REORDER_CAP` to the link tier's window
//! so a conforming sender never overflows; `MAX_PAYLOAD` is the channel MDU. The boxed-bulk
//! twin for scale is `FixedHeapChannelColumns`.

use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::columns::{
    BufferOutcome, ChannelColumns, EnsureChannelError, OutstandingSend, TxOutcome,
};
use crate::routing::links::channel::{ChannelSequence, ChannelWindow, MessageType};
use crate::routing::links::LinkId;

pub struct FixedArrayChannelColumns<
    const SLOTS: usize,
    const REORDER_CAP: usize,
    const MAX_PAYLOAD: usize,
> {
    len: usize,
    link_ids: [LinkId; SLOTS],
    next_expected: [ChannelSequence; SLOTS],
    buffered_count: [usize; SLOTS],
    sequences: [[ChannelSequence; REORDER_CAP]; SLOTS],
    message_types: [[MessageType; REORDER_CAP]; SLOTS],
    payload_lens: [[usize; REORDER_CAP]; SLOTS],
    payloads: [[[u8; MAX_PAYLOAD]; REORDER_CAP]; SLOTS],
    next_tx_sequence: [ChannelSequence; SLOTS],
    windows: [ChannelWindow; SLOTS],
    outstanding_count: [usize; SLOTS],
    outstanding_packet_hashes: [[PacketHash; REORDER_CAP]; SLOTS],
    outstanding_command_ids: [[CommandId; REORDER_CAP]; SLOTS],
    outstanding_sent_ats: [[InstantMillis; REORDER_CAP]; SLOTS],
    outstanding_timeout_ats: [[InstantMillis; REORDER_CAP]; SLOTS],
    outstanding_tries: [[u8; REORDER_CAP]; SLOTS],
    outstanding_sequences: [[ChannelSequence; REORDER_CAP]; SLOTS],
    outstanding_message_types: [[MessageType; REORDER_CAP]; SLOTS],
    outstanding_body_lens: [[usize; REORDER_CAP]; SLOTS],
    outstanding_bodies: [[[u8; MAX_PAYLOAD]; REORDER_CAP]; SLOTS],
    outstanding_ivs: [[[u8; 16]; REORDER_CAP]; SLOTS],
}

impl<const SLOTS: usize, const REORDER_CAP: usize, const MAX_PAYLOAD: usize> Default
    for FixedArrayChannelColumns<SLOTS, REORDER_CAP, MAX_PAYLOAD>
{
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); SLOTS],
            next_expected: [ChannelSequence(0); SLOTS],
            buffered_count: [0; SLOTS],
            sequences: [[ChannelSequence(0); REORDER_CAP]; SLOTS],
            message_types: [[MessageType(0); REORDER_CAP]; SLOTS],
            payload_lens: [[0; REORDER_CAP]; SLOTS],
            payloads: [[[0u8; MAX_PAYLOAD]; REORDER_CAP]; SLOTS],
            next_tx_sequence: [ChannelSequence(0); SLOTS],
            windows: [ChannelWindow::default(); SLOTS],
            outstanding_count: [0; SLOTS],
            outstanding_packet_hashes: [[PacketHash::new([0u8; 32]); REORDER_CAP]; SLOTS],
            outstanding_command_ids: [[CommandId(0); REORDER_CAP]; SLOTS],
            outstanding_sent_ats: [[InstantMillis(0); REORDER_CAP]; SLOTS],
            outstanding_timeout_ats: [[InstantMillis(0); REORDER_CAP]; SLOTS],
            outstanding_tries: [[0; REORDER_CAP]; SLOTS],
            outstanding_sequences: [[ChannelSequence(0); REORDER_CAP]; SLOTS],
            outstanding_message_types: [[MessageType(0); REORDER_CAP]; SLOTS],
            outstanding_body_lens: [[0; REORDER_CAP]; SLOTS],
            outstanding_bodies: [[[0u8; MAX_PAYLOAD]; REORDER_CAP]; SLOTS],
            outstanding_ivs: [[[0u8; 16]; REORDER_CAP]; SLOTS],
        }
    }
}

impl<const SLOTS: usize, const REORDER_CAP: usize, const MAX_PAYLOAD: usize> ChannelColumns
    for FixedArrayChannelColumns<SLOTS, REORDER_CAP, MAX_PAYLOAD>
{
    fn capacity(&self) -> usize {
        SLOTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn index_of(&self, link: &LinkId) -> Option<usize> {
        self.link_ids[..self.len].iter().position(|id| id == link)
    }
    fn link_at(&self, index: usize) -> LinkId {
        self.link_ids[index]
    }

    fn ensure(&mut self, link: &LinkId) -> Result<usize, EnsureChannelError> {
        if let Some(index) = self.index_of(link) {
            return Ok(index);
        }
        if self.len >= SLOTS {
            return Err(EnsureChannelError::TableFull);
        }
        let index = self.len;
        self.link_ids[index] = *link;
        self.next_expected[index] = ChannelSequence(0);
        self.buffered_count[index] = 0;
        self.next_tx_sequence[index] = ChannelSequence(0);
        self.windows[index] = ChannelWindow::default();
        self.outstanding_count[index] = 0;
        self.len += 1;
        Ok(index)
    }

    fn close(&mut self, link: &LinkId) {
        let Some(index) = self.index_of(link) else {
            return;
        };
        let last = self.len - 1;
        self.link_ids.swap(index, last);
        self.next_expected.swap(index, last);
        self.buffered_count.swap(index, last);
        self.sequences.swap(index, last);
        self.message_types.swap(index, last);
        self.payload_lens.swap(index, last);
        self.payloads.swap(index, last);
        self.next_tx_sequence.swap(index, last);
        self.windows.swap(index, last);
        self.outstanding_count.swap(index, last);
        self.outstanding_packet_hashes.swap(index, last);
        self.outstanding_command_ids.swap(index, last);
        self.outstanding_sent_ats.swap(index, last);
        self.outstanding_timeout_ats.swap(index, last);
        self.outstanding_tries.swap(index, last);
        self.outstanding_sequences.swap(index, last);
        self.outstanding_message_types.swap(index, last);
        self.outstanding_body_lens.swap(index, last);
        self.outstanding_bodies.swap(index, last);
        self.outstanding_ivs.swap(index, last);
        self.len = last;
    }

    fn next_expected(&self, index: usize) -> ChannelSequence {
        self.next_expected[index]
    }
    fn set_next_expected(&mut self, index: usize, sequence: ChannelSequence) {
        self.next_expected[index] = sequence;
    }

    fn buffered_sequences(&self, index: usize) -> &[ChannelSequence] {
        &self.sequences[index][..self.buffered_count[index]]
    }
    fn buffered_message_type(&self, index: usize, sub: usize) -> MessageType {
        self.message_types[index][sub]
    }
    fn buffered_payload(&self, index: usize, sub: usize) -> &[u8] {
        &self.payloads[index][sub][..self.payload_lens[index][sub]]
    }

    fn push_buffered(
        &mut self,
        index: usize,
        sequence: ChannelSequence,
        message_type: MessageType,
        payload: &[u8],
    ) -> BufferOutcome {
        let count = self.buffered_count[index];
        if count >= REORDER_CAP || payload.len() > MAX_PAYLOAD {
            return BufferOutcome::Full;
        }
        self.sequences[index][count] = sequence;
        self.message_types[index][count] = message_type;
        self.payload_lens[index][count] = payload.len();
        self.payloads[index][count][..payload.len()].copy_from_slice(payload);
        self.buffered_count[index] = count + 1;
        BufferOutcome::Stored
    }

    fn swap_remove_buffered(&mut self, index: usize, sub: usize) {
        let last = self.buffered_count[index] - 1;
        self.sequences[index].swap(sub, last);
        self.message_types[index].swap(sub, last);
        self.payload_lens[index].swap(sub, last);
        self.payloads[index].swap(sub, last);
        self.buffered_count[index] = last;
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
        self.outstanding_count[index]
    }
    fn outstanding_packet_hashes(&self, index: usize) -> &[PacketHash] {
        &self.outstanding_packet_hashes[index][..self.outstanding_count[index]]
    }
    fn outstanding_command_id(&self, index: usize, sub: usize) -> CommandId {
        self.outstanding_command_ids[index][sub]
    }
    fn outstanding_sent_at(&self, index: usize, sub: usize) -> InstantMillis {
        self.outstanding_sent_ats[index][sub]
    }
    fn outstanding_timeout_at(&self, index: usize, sub: usize) -> InstantMillis {
        self.outstanding_timeout_ats[index][sub]
    }
    fn set_outstanding_timeout_at(&mut self, index: usize, sub: usize, timeout_at: InstantMillis) {
        self.outstanding_timeout_ats[index][sub] = timeout_at;
    }
    fn outstanding_tries(&self, index: usize, sub: usize) -> u8 {
        self.outstanding_tries[index][sub]
    }
    fn set_outstanding_tries(&mut self, index: usize, sub: usize, tries: u8) {
        self.outstanding_tries[index][sub] = tries;
    }
    fn outstanding_sequence(&self, index: usize, sub: usize) -> ChannelSequence {
        self.outstanding_sequences[index][sub]
    }
    fn outstanding_message_type(&self, index: usize, sub: usize) -> MessageType {
        self.outstanding_message_types[index][sub]
    }
    fn outstanding_body(&self, index: usize, sub: usize) -> &[u8] {
        &self.outstanding_bodies[index][sub][..self.outstanding_body_lens[index][sub]]
    }
    fn outstanding_iv(&self, index: usize, sub: usize) -> [u8; 16] {
        self.outstanding_ivs[index][sub]
    }

    fn push_outstanding(&mut self, index: usize, send: OutstandingSend<'_>) -> TxOutcome {
        let count = self.outstanding_count[index];
        if count >= REORDER_CAP || send.body.len() > MAX_PAYLOAD {
            return TxOutcome::Full;
        }
        self.outstanding_packet_hashes[index][count] = send.packet_hash;
        self.outstanding_command_ids[index][count] = send.command_id;
        self.outstanding_sent_ats[index][count] = send.sent_at;
        self.outstanding_timeout_ats[index][count] = send.timeout_at;
        self.outstanding_tries[index][count] = 0;
        self.outstanding_sequences[index][count] = send.sequence;
        self.outstanding_message_types[index][count] = send.message_type;
        self.outstanding_body_lens[index][count] = send.body.len();
        self.outstanding_bodies[index][count][..send.body.len()].copy_from_slice(send.body);
        self.outstanding_ivs[index][count] = send.iv;
        self.outstanding_count[index] = count + 1;
        TxOutcome::Tracked
    }

    fn retire_outstanding(&mut self, index: usize, sub: usize) {
        let last = self.outstanding_count[index] - 1;
        self.outstanding_packet_hashes[index].swap(sub, last);
        self.outstanding_command_ids[index].swap(sub, last);
        self.outstanding_sent_ats[index].swap(sub, last);
        self.outstanding_timeout_ats[index].swap(sub, last);
        self.outstanding_tries[index].swap(sub, last);
        self.outstanding_sequences[index].swap(sub, last);
        self.outstanding_message_types[index].swap(sub, last);
        self.outstanding_body_lens[index].swap(sub, last);
        self.outstanding_bodies[index].swap(sub, last);
        self.outstanding_ivs[index].swap(sub, last);
        self.outstanding_count[index] = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Columns = FixedArrayChannelColumns<2, 4, 16>;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    #[test]
    fn ensure_is_idempotent_and_starts_at_sequence_zero() {
        let mut columns = Columns::default();
        let a = columns.ensure(&link(1)).unwrap();
        assert_eq!(columns.ensure(&link(1)).unwrap(), a, "same link, same slot");
        assert_eq!(columns.len(), 1);
        assert_eq!(columns.next_expected(a), ChannelSequence(0));
    }

    #[test]
    fn a_full_channel_table_refuses_a_new_link() {
        let mut columns = Columns::default();
        columns.ensure(&link(1)).unwrap();
        columns.ensure(&link(2)).unwrap();
        assert_eq!(columns.ensure(&link(3)), Err(EnsureChannelError::TableFull));
    }

    #[test]
    fn buffered_entries_round_trip_and_pack() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        assert_eq!(
            columns.push_buffered(i, ChannelSequence(5), MessageType(0x07), b"five"),
            BufferOutcome::Stored
        );
        assert_eq!(
            columns.push_buffered(i, ChannelSequence(6), MessageType(0x08), b"six"),
            BufferOutcome::Stored
        );
        assert_eq!(
            columns.buffered_sequences(i),
            &[ChannelSequence(5), ChannelSequence(6)]
        );
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
    fn a_full_reorder_buffer_or_an_oversized_body_is_refused() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        for n in 0..4u16 {
            assert_eq!(
                columns.push_buffered(i, ChannelSequence(n), MessageType(0), b"x"),
                BufferOutcome::Stored
            );
        }
        assert_eq!(
            columns.push_buffered(i, ChannelSequence(99), MessageType(0), b"x"),
            BufferOutcome::Full,
            "REORDER_CAP reached",
        );

        let mut empty = Columns::default();
        let j = empty.ensure(&link(2)).unwrap();
        assert_eq!(
            empty.push_buffered(j, ChannelSequence(0), MessageType(0), &[0u8; 17]),
            BufferOutcome::Full,
            "body past MAX_PAYLOAD",
        );
    }

    #[test]
    fn close_frees_the_slot_and_keeps_the_other_channel_findable() {
        let mut columns = Columns::default();
        columns.ensure(&link(1)).unwrap();
        let b = columns.ensure(&link(2)).unwrap();
        columns.set_next_expected(b, ChannelSequence(42));

        columns.close(&link(1));
        assert_eq!(columns.len(), 1);
        let b = columns.index_of(&link(2)).unwrap();
        assert_eq!(columns.next_expected(b), ChannelSequence(42));
        assert_eq!(columns.index_of(&link(1)), None);
    }

    #[test]
    fn the_tx_sequence_advances_and_is_per_channel() {
        let mut columns = Columns::default();
        let a = columns.ensure(&link(1)).unwrap();
        let b = columns.ensure(&link(2)).unwrap();
        assert_eq!(columns.next_tx_sequence(a), ChannelSequence(0));
        columns.set_next_tx_sequence(a, ChannelSequence(1));
        assert_eq!(columns.next_tx_sequence(a), ChannelSequence(1));
        assert_eq!(
            columns.next_tx_sequence(b),
            ChannelSequence(0),
            "per channel"
        );
    }

    #[test]
    fn outstanding_sends_track_their_resend_material_match_by_hash_and_retire() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        assert_eq!(columns.outstanding_count(i), 0);
        assert_eq!(
            columns.push_outstanding(i, outstanding(5, 50)),
            TxOutcome::Tracked
        );
        assert_eq!(
            columns.push_outstanding(i, outstanding(6, 60)),
            TxOutcome::Tracked
        );
        assert_eq!(columns.outstanding_count(i), 2);

        let sub = columns
            .outstanding_packet_hashes(i)
            .iter()
            .position(|h| *h == hash(5))
            .unwrap();
        assert_eq!(columns.outstanding_command_id(i, sub), CommandId(50));
        assert_eq!(columns.outstanding_sent_at(i, sub), InstantMillis(500));
        assert_eq!(columns.outstanding_tries(i, sub), 0);
        assert_eq!(columns.outstanding_sequence(i, sub), ChannelSequence(5));
        assert_eq!(columns.outstanding_message_type(i, sub), MessageType(0x07));
        assert_eq!(columns.outstanding_body(i, sub), b"body");
        assert_eq!(columns.outstanding_iv(i, sub), [5u8; 16]);

        columns.set_outstanding_tries(i, sub, 2);
        columns.set_outstanding_timeout_at(i, sub, InstantMillis(9_999));
        assert_eq!(columns.outstanding_tries(i, sub), 2);
        assert_eq!(columns.outstanding_timeout_at(i, sub), InstantMillis(9_999));

        columns.retire_outstanding(i, sub);
        assert_eq!(columns.outstanding_count(i), 1);
        assert_eq!(columns.outstanding_packet_hashes(i), &[hash(6)]);
    }

    #[test]
    fn a_full_outstanding_ring_is_refused() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        for n in 0..4u8 {
            assert_eq!(
                columns.push_outstanding(i, outstanding(n, u64::from(n))),
                TxOutcome::Tracked
            );
        }
        assert_eq!(
            columns.push_outstanding(i, outstanding(99, 99)),
            TxOutcome::Full,
            "REORDER_CAP bounds the outstanding ring too",
        );
    }

    fn hash(byte: u8) -> PacketHash {
        PacketHash::new([byte; 32])
    }

    fn outstanding(byte: u8, command: u64) -> OutstandingSend<'static> {
        OutstandingSend {
            packet_hash: hash(byte),
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
