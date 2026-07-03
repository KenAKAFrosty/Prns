//! Channel columns with the per-channel reorder and outstanding metadata inline (one row
//! per open channel) and the bulk message payloads in two shared pools (receive reorder,
//! send retransmit) in a caller-chosen heap region (PSRAM on the S3) via `A`.
//!
//! Pooling decouples concurrency from depth: the same `POOL` serves many channels holding
//! a few in flight, or a few channels holding many. A push that finds the pool dry returns
//! the same `Full` outcome a per-channel cap would, so the window simply cannot elevate
//! until another channel drains a slot: a local backpressure signal, not a new failure path.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::columns::{
    BufferOutcome, ChannelColumns, EnsureChannelError, OutstandingSend, TxOutcome,
};
use crate::routing::links::channel::{ChannelSequence, ChannelWindow, MessageType};
use crate::routing::links::LinkId;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

/// A payload pool's free list, seeded with every slot id `0..len`; the order slots come out
/// in does not matter. Built directly in `A`: the widest stack transient is one
/// `[0u8; MAX_PAYLOAD]` row template (in [`filled`]), never a whole pool.
fn free_list<A: Allocator>(len: usize, alloc: A) -> Box<[u16], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    for slot in 0..len {
        column.push(slot as u16);
    }
    column.into_boxed_slice()
}

pub struct FixedHeapChannelColumns<
    const SLOTS: usize,
    const REORDER_CAP: usize,
    const MAX_PAYLOAD: usize,
    const POOL: usize,
    A: Allocator = Global,
> {
    len: usize,
    link_ids: [LinkId; SLOTS],
    next_expected: [ChannelSequence; SLOTS],
    buffered_count: [usize; SLOTS],
    sequences: Box<[[ChannelSequence; REORDER_CAP]], A>,
    message_types: Box<[[MessageType; REORDER_CAP]], A>,
    payload_lens: Box<[[usize; REORDER_CAP]], A>,
    payload_slots: Box<[[u16; REORDER_CAP]], A>,
    payload_pool: Box<[[u8; MAX_PAYLOAD]], A>,
    payload_free: Box<[u16], A>,
    payload_free_len: usize,
    next_tx_sequence: [ChannelSequence; SLOTS],
    windows: [ChannelWindow; SLOTS],
    outstanding_count: [usize; SLOTS],
    outstanding_packet_hashes: Box<[[PacketHash; REORDER_CAP]], A>,
    outstanding_command_ids: Box<[[CommandId; REORDER_CAP]], A>,
    outstanding_sent_ats: Box<[[InstantMillis; REORDER_CAP]], A>,
    outstanding_timeout_ats: Box<[[InstantMillis; REORDER_CAP]], A>,
    outstanding_tries: Box<[[u8; REORDER_CAP]], A>,
    outstanding_sequences: Box<[[ChannelSequence; REORDER_CAP]], A>,
    outstanding_message_types: Box<[[MessageType; REORDER_CAP]], A>,
    outstanding_body_lens: Box<[[usize; REORDER_CAP]], A>,
    outstanding_body_slots: Box<[[u16; REORDER_CAP]], A>,
    outstanding_body_pool: Box<[[u8; MAX_PAYLOAD]], A>,
    outstanding_body_free: Box<[u16], A>,
    outstanding_body_free_len: usize,
    outstanding_ivs: Box<[[[u8; 16]; REORDER_CAP]], A>,
}

impl<
        const SLOTS: usize,
        const REORDER_CAP: usize,
        const MAX_PAYLOAD: usize,
        const POOL: usize,
        A: Allocator + Default,
    > Default for FixedHeapChannelColumns<SLOTS, REORDER_CAP, MAX_PAYLOAD, POOL, A>
{
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); SLOTS],
            next_expected: [ChannelSequence(0); SLOTS],
            buffered_count: [0; SLOTS],
            sequences: filled([ChannelSequence(0); REORDER_CAP], SLOTS, A::default()),
            message_types: filled([MessageType(0); REORDER_CAP], SLOTS, A::default()),
            payload_lens: filled([0; REORDER_CAP], SLOTS, A::default()),
            payload_slots: filled([0u16; REORDER_CAP], SLOTS, A::default()),
            payload_pool: filled([0u8; MAX_PAYLOAD], POOL, A::default()),
            payload_free: free_list(POOL, A::default()),
            payload_free_len: POOL,
            next_tx_sequence: [ChannelSequence(0); SLOTS],
            windows: [ChannelWindow::default(); SLOTS],
            outstanding_count: [0; SLOTS],
            outstanding_packet_hashes: filled(
                [PacketHash::new([0u8; 32]); REORDER_CAP],
                SLOTS,
                A::default(),
            ),
            outstanding_command_ids: filled([CommandId(0); REORDER_CAP], SLOTS, A::default()),
            outstanding_sent_ats: filled([InstantMillis(0); REORDER_CAP], SLOTS, A::default()),
            outstanding_timeout_ats: filled([InstantMillis(0); REORDER_CAP], SLOTS, A::default()),
            outstanding_tries: filled([0; REORDER_CAP], SLOTS, A::default()),
            outstanding_sequences: filled([ChannelSequence(0); REORDER_CAP], SLOTS, A::default()),
            outstanding_message_types: filled([MessageType(0); REORDER_CAP], SLOTS, A::default()),
            outstanding_body_lens: filled([0; REORDER_CAP], SLOTS, A::default()),
            outstanding_body_slots: filled([0u16; REORDER_CAP], SLOTS, A::default()),
            outstanding_body_pool: filled([0u8; MAX_PAYLOAD], POOL, A::default()),
            outstanding_body_free: free_list(POOL, A::default()),
            outstanding_body_free_len: POOL,
            outstanding_ivs: filled([[0u8; 16]; REORDER_CAP], SLOTS, A::default()),
        }
    }
}

impl<
        const SLOTS: usize,
        const REORDER_CAP: usize,
        const MAX_PAYLOAD: usize,
        const POOL: usize,
        A: Allocator,
    > ChannelColumns for FixedHeapChannelColumns<SLOTS, REORDER_CAP, MAX_PAYLOAD, POOL, A>
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
        for sub in 0..self.buffered_count[index] {
            self.payload_free[self.payload_free_len] = self.payload_slots[index][sub];
            self.payload_free_len += 1;
        }
        for sub in 0..self.outstanding_count[index] {
            self.outstanding_body_free[self.outstanding_body_free_len] =
                self.outstanding_body_slots[index][sub];
            self.outstanding_body_free_len += 1;
        }
        let last = self.len - 1;
        self.link_ids.swap(index, last);
        self.next_expected.swap(index, last);
        self.buffered_count.swap(index, last);
        self.sequences.swap(index, last);
        self.message_types.swap(index, last);
        self.payload_lens.swap(index, last);
        self.payload_slots.swap(index, last);
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
        self.outstanding_body_slots.swap(index, last);
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
        let slot = self.payload_slots[index][sub] as usize;
        &self.payload_pool[slot][..self.payload_lens[index][sub]]
    }

    fn push_buffered(
        &mut self,
        index: usize,
        sequence: ChannelSequence,
        message_type: MessageType,
        payload: &[u8],
    ) -> BufferOutcome {
        let count = self.buffered_count[index];
        if count >= REORDER_CAP || payload.len() > MAX_PAYLOAD || self.payload_free_len == 0 {
            return BufferOutcome::Full;
        }
        self.payload_free_len -= 1;
        let slot = self.payload_free[self.payload_free_len] as usize;
        self.sequences[index][count] = sequence;
        self.message_types[index][count] = message_type;
        self.payload_lens[index][count] = payload.len();
        self.payload_pool[slot][..payload.len()].copy_from_slice(payload);
        self.payload_slots[index][count] = slot as u16;
        self.buffered_count[index] = count + 1;
        BufferOutcome::Stored
    }

    fn swap_remove_buffered(&mut self, index: usize, sub: usize) {
        let last = self.buffered_count[index] - 1;
        self.payload_free[self.payload_free_len] = self.payload_slots[index][sub];
        self.payload_free_len += 1;
        self.sequences[index].swap(sub, last);
        self.message_types[index].swap(sub, last);
        self.payload_lens[index].swap(sub, last);
        self.payload_slots[index].swap(sub, last);
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
        let slot = self.outstanding_body_slots[index][sub] as usize;
        &self.outstanding_body_pool[slot][..self.outstanding_body_lens[index][sub]]
    }
    fn outstanding_iv(&self, index: usize, sub: usize) -> [u8; 16] {
        self.outstanding_ivs[index][sub]
    }

    fn push_outstanding(&mut self, index: usize, send: OutstandingSend<'_>) -> TxOutcome {
        let count = self.outstanding_count[index];
        if count >= REORDER_CAP
            || send.body.len() > MAX_PAYLOAD
            || self.outstanding_body_free_len == 0
        {
            return TxOutcome::Full;
        }
        self.outstanding_body_free_len -= 1;
        let slot = self.outstanding_body_free[self.outstanding_body_free_len] as usize;
        self.outstanding_packet_hashes[index][count] = send.packet_hash;
        self.outstanding_command_ids[index][count] = send.command_id;
        self.outstanding_sent_ats[index][count] = send.sent_at;
        self.outstanding_timeout_ats[index][count] = send.timeout_at;
        self.outstanding_tries[index][count] = 0;
        self.outstanding_sequences[index][count] = send.sequence;
        self.outstanding_message_types[index][count] = send.message_type;
        self.outstanding_body_lens[index][count] = send.body.len();
        self.outstanding_body_pool[slot][..send.body.len()].copy_from_slice(send.body);
        self.outstanding_body_slots[index][count] = slot as u16;
        self.outstanding_ivs[index][count] = send.iv;
        self.outstanding_count[index] = count + 1;
        TxOutcome::Tracked
    }

    fn retire_outstanding(&mut self, index: usize, sub: usize) {
        let last = self.outstanding_count[index] - 1;
        self.outstanding_body_free[self.outstanding_body_free_len] =
            self.outstanding_body_slots[index][sub];
        self.outstanding_body_free_len += 1;
        self.outstanding_packet_hashes[index].swap(sub, last);
        self.outstanding_command_ids[index].swap(sub, last);
        self.outstanding_sent_ats[index].swap(sub, last);
        self.outstanding_timeout_ats[index].swap(sub, last);
        self.outstanding_tries[index].swap(sub, last);
        self.outstanding_sequences[index].swap(sub, last);
        self.outstanding_message_types[index].swap(sub, last);
        self.outstanding_body_lens[index].swap(sub, last);
        self.outstanding_body_slots[index].swap(sub, last);
        self.outstanding_ivs[index].swap(sub, last);
        self.outstanding_count[index] = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Columns = FixedHeapChannelColumns<2, 4, 16, 8>;

    fn link(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    #[test]
    fn buffered_entries_round_trip_through_the_pool() {
        let mut columns = Columns::default();
        assert_eq!(columns.capacity(), 2);
        let i = columns.ensure(&link(1)).unwrap();
        assert_eq!(
            columns.push_buffered(i, ChannelSequence(5), MessageType(0x07), b"five"),
            BufferOutcome::Stored
        );
        let sub = columns
            .buffered_sequences(i)
            .iter()
            .position(|s| *s == ChannelSequence(5))
            .unwrap();
        assert_eq!(columns.buffered_message_type(i, sub), MessageType(0x07));
        assert_eq!(columns.buffered_payload(i, sub), b"five");
        columns.swap_remove_buffered(i, sub);
        assert!(columns.buffered_sequences(i).is_empty());
    }

    #[test]
    fn the_table_and_per_channel_window_enforce_their_caps() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        columns.ensure(&link(2)).unwrap();
        assert_eq!(columns.ensure(&link(3)), Err(EnsureChannelError::TableFull));
        for n in 0..4u16 {
            columns.push_buffered(i, ChannelSequence(n), MessageType(0), b"x");
        }
        assert_eq!(
            columns.push_buffered(i, ChannelSequence(99), MessageType(0), b"x"),
            BufferOutcome::Full
        );
    }

    #[test]
    fn a_payload_follows_its_slot_through_swap_remove() {
        let mut columns = Columns::default();
        let i = columns.ensure(&link(1)).unwrap();
        columns.push_buffered(i, ChannelSequence(10), MessageType(0), b"aa");
        columns.push_buffered(i, ChannelSequence(11), MessageType(0), b"bb");
        columns.push_buffered(i, ChannelSequence(12), MessageType(0), b"cc");
        let middle = columns
            .buffered_sequences(i)
            .iter()
            .position(|s| *s == ChannelSequence(11))
            .unwrap();
        columns.swap_remove_buffered(i, middle);
        assert_eq!(columns.buffered_sequences(i).len(), 2);
        for (seq, body) in [
            (ChannelSequence(10), b"aa".as_slice()),
            (ChannelSequence(12), b"cc".as_slice()),
        ] {
            let sub = columns
                .buffered_sequences(i)
                .iter()
                .position(|s| *s == seq)
                .unwrap();
            assert_eq!(columns.buffered_payload(i, sub), body);
        }
    }

    #[test]
    fn the_shared_pool_caps_total_buffering_and_reclaims_on_drain() {
        type Pooled = FixedHeapChannelColumns<4, 4, 16, 3>;
        let mut columns = Pooled::default();
        let a = columns.ensure(&link(1)).unwrap();
        let b = columns.ensure(&link(2)).unwrap();
        assert_eq!(
            columns.push_buffered(a, ChannelSequence(0), MessageType(0), b"x"),
            BufferOutcome::Stored
        );
        assert_eq!(
            columns.push_buffered(a, ChannelSequence(1), MessageType(0), b"y"),
            BufferOutcome::Stored
        );
        assert_eq!(
            columns.push_buffered(b, ChannelSequence(0), MessageType(0), b"z"),
            BufferOutcome::Stored
        );
        assert_eq!(
            columns.push_buffered(b, ChannelSequence(1), MessageType(0), b"w"),
            BufferOutcome::Full
        );
        assert_eq!(
            columns.push_buffered(a, ChannelSequence(2), MessageType(0), b"v"),
            BufferOutcome::Full
        );
        let sub = columns
            .buffered_sequences(a)
            .iter()
            .position(|s| *s == ChannelSequence(0))
            .unwrap();
        columns.swap_remove_buffered(a, sub);
        assert_eq!(
            columns.push_buffered(b, ChannelSequence(1), MessageType(0), b"w"),
            BufferOutcome::Stored
        );
    }

    #[test]
    fn close_frees_the_slot_and_keeps_the_other_findable() {
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
    fn close_returns_a_channels_payload_slots_to_the_pool() {
        type Pooled = FixedHeapChannelColumns<2, 4, 16, 2>;
        let mut columns = Pooled::default();
        let a = columns.ensure(&link(1)).unwrap();
        columns.push_buffered(a, ChannelSequence(0), MessageType(0), b"x");
        columns.push_buffered(a, ChannelSequence(1), MessageType(0), b"y");
        let b = columns.ensure(&link(2)).unwrap();
        assert_eq!(
            columns.push_buffered(b, ChannelSequence(0), MessageType(0), b"z"),
            BufferOutcome::Full
        );
        columns.close(&link(1));
        let b = columns.index_of(&link(2)).unwrap();
        assert_eq!(
            columns.push_buffered(b, ChannelSequence(0), MessageType(0), b"z"),
            BufferOutcome::Stored
        );
    }
}
