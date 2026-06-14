//! The fully-inline, no-alloc channel store: the channel table and every
//! channel's reorder buffer live in fixed arrays. `SLOTS` is the most channels
//! tracked at once, `REORDER_CAP` the most out-of-order messages held per
//! channel (size it to the link tier's window so a conforming sender never
//! overflows), `MAX_PAYLOAD` the channel MDU. The boxed-bulk twin for scale is
//! `FixedHeapChannelColumns`.

use crate::routing::links::channel::columns::{BufferOutcome, ChannelColumns, EnsureChannelError};
use crate::routing::links::channel::{ChannelSequence, MessageType};
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
}
