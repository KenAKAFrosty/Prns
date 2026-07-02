//! The receive side of a channel: RNS 1.3.1 `Channel._receive`'s window
//! validation, duplicate rejection, and contiguous in-order drain, ported to
//! integer sequence arithmetic.
//!
//! The algorithm is storage-agnostic — it runs over a [`ChannelColumns`] so the
//! reorder buffer's backend (and the reorder *policy* itself) stays swappable,
//! with no const generic in the engine path. The arrival that fits the next
//! expected slot is delivered straight from the packet (no copy); only
//! out-of-order arrivals are buffered. When the gap fills, the whole contiguous
//! run is drained in one [`receive`] call.

use super::columns::{BufferOutcome, ChannelColumns, EnsureChannelError};
use super::MessageType;
use super::{ChannelSequence, SEQ_MODULUS};
use crate::routing::links::LinkId;

/// RNS 1.3.1 `Channel.WINDOW_MAX` (`= WINDOW_MAX_FAST`): the largest send
/// window, so the furthest ahead of the next expected sequence a well-behaved
/// peer ever transmits — and thus the most out-of-order messages worth holding.
pub const WINDOW_MAX: u16 = 48;

/// RNS 1.3.1 `Channel._receive`'s window guard. A sequence at or after the next
/// expected one is always in window. One *before* it is only valid when the
/// window `next_rx + WINDOW_MAX` wrapped past the modulus and the sequence
/// falls inside that wrapped tail — otherwise it is already delivered, or junk.
pub fn within_receive_window(sequence: ChannelSequence, next_rx: ChannelSequence) -> bool {
    if sequence.0 >= next_rx.0 {
        return true;
    }
    let window_overflow = ((next_rx.0 as u32 + WINDOW_MAX as u32) % SEQ_MODULUS) as u16;
    window_overflow < next_rx.0 && sequence.0 <= window_overflow
}

/// What a single [`receive`] did. Every outcome owes the sender a proof except
/// the two drop cases — a full buffer or an untrackable channel — which withhold
/// it so the sender retransmits once room frees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// `count` messages were delivered in order — the arrival plus any buffered
    /// run it unblocked.
    Delivered { count: u16 },
    /// An out-of-order arrival was stored to await the gap ahead of it.
    Buffered,
    /// The sequence is already buffered — re-acked, not delivered again.
    AlreadyHave,
    /// The sequence is below the window (already delivered, or junk) — dropped.
    OutOfWindow,
    /// The reorder buffer is full (or the body would not fit) — dropped unproven.
    BufferFull,
    /// The channel table had no room to even track this link — dropped unproven.
    Untracked,
}

impl ReceiveOutcome {
    /// Whether the receiver owes the sender a proof for this arrival. The two
    /// drop cases withhold it, to force a retransmission once room frees.
    pub const fn owes_proof(self) -> bool {
        !matches!(self, Self::BufferFull | Self::Untracked)
    }
}

/// Take one channel arrival for `link` into `columns`. The in-order arrival
/// (and any buffered run it unblocks) is handed to `on_deliver` in sequence
/// order; an out-of-order arrival is buffered. See [`ReceiveOutcome`].
pub fn receive<C: ChannelColumns>(
    columns: &mut C,
    link: &LinkId,
    sequence: ChannelSequence,
    message_type: MessageType,
    payload: &[u8],
    mut on_deliver: impl FnMut(MessageType, &[u8]),
) -> ReceiveOutcome {
    let index = match columns.ensure(link) {
        Ok(index) => index,
        Err(EnsureChannelError::TableFull) => return ReceiveOutcome::Untracked,
    };

    let mut next_rx = columns.next_expected(index);
    if sequence == next_rx {
        on_deliver(message_type, payload);
        next_rx = next_rx.next();
        let mut count: u16 = 1;
        while let Some(sub) = columns
            .buffered_sequences(index)
            .iter()
            .position(|buffered| *buffered == next_rx)
        {
            let message_type = columns.buffered_message_type(index, sub);
            on_deliver(message_type, columns.buffered_payload(index, sub));
            columns.swap_remove_buffered(index, sub);
            next_rx = next_rx.next();
            count += 1;
        }
        columns.set_next_expected(index, next_rx);
        return ReceiveOutcome::Delivered { count };
    }

    if !within_receive_window(sequence, next_rx) {
        return ReceiveOutcome::OutOfWindow;
    }
    if columns.buffered_sequences(index).contains(&sequence) {
        return ReceiveOutcome::AlreadyHave;
    }
    match columns.push_buffered(index, sequence, message_type, payload) {
        BufferOutcome::Stored => ReceiveOutcome::Buffered,
        BufferOutcome::Full => ReceiveOutcome::BufferFull,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::channel::impls::FixedArrayChannelColumns;
    use std::vec::Vec;

    type Columns = FixedArrayChannelColumns<2, 8, 16>;

    fn link() -> LinkId {
        LinkId::new([0xAB; 16])
    }
    fn seq(n: u16) -> ChannelSequence {
        ChannelSequence(n)
    }
    fn mt(n: u16) -> MessageType {
        MessageType(n)
    }

    /// Drive one arrival, collecting whatever it delivers in order.
    fn feed(columns: &mut Columns, sequence: u16, body: &[u8]) -> (ReceiveOutcome, Vec<Vec<u8>>) {
        let mut delivered = Vec::new();
        let outcome = receive(
            columns,
            &link(),
            seq(sequence),
            mt(sequence),
            body,
            |_, bytes| delivered.push(bytes.to_vec()),
        );
        (outcome, delivered)
    }

    #[test]
    fn in_order_arrivals_deliver_immediately() {
        let mut c = Columns::default();
        let (o0, d0) = feed(&mut c, 0, b"a");
        let (o1, d1) = feed(&mut c, 1, b"b");
        assert_eq!(o0, ReceiveOutcome::Delivered { count: 1 });
        assert_eq!(o1, ReceiveOutcome::Delivered { count: 1 });
        assert_eq!(d0, vec![b"a".to_vec()]);
        assert_eq!(d1, vec![b"b".to_vec()]);
    }

    #[test]
    fn an_out_of_order_arrival_waits_until_the_gap_fills() {
        let mut c = Columns::default();
        assert_eq!(feed(&mut c, 1, b"b").0, ReceiveOutcome::Buffered);
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::Buffered);
        let (outcome, delivered) = feed(&mut c, 0, b"a");
        assert_eq!(outcome, ReceiveOutcome::Delivered { count: 3 });
        assert_eq!(delivered, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn a_buffered_duplicate_is_not_redelivered() {
        let mut c = Columns::default();
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::Buffered);
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::AlreadyHave);
        let (_, delivered) = feed(&mut c, 0, b"a");
        assert_eq!(delivered, vec![b"a".to_vec()]); // only 0; the gap at 1 still holds 2 back
    }

    #[test]
    fn an_already_delivered_sequence_is_dropped_out_of_window() {
        let mut c = Columns::default();
        feed(&mut c, 0, b"a");
        feed(&mut c, 1, b"b");
        let (outcome, delivered) = feed(&mut c, 0, b"a");
        assert_eq!(outcome, ReceiveOutcome::OutOfWindow);
        assert!(delivered.is_empty());
    }

    #[test]
    fn the_window_guard_accepts_a_wrapped_future_but_not_a_stale_past() {
        let next_rx = seq(0xFFF0); // window 0xFFF0 + 48 wraps to 0x0020
        assert!(within_receive_window(seq(0xFFF0), next_rx));
        assert!(within_receive_window(seq(0xFFFF), next_rx));
        assert!(within_receive_window(seq(0x0000), next_rx));
        assert!(within_receive_window(seq(0x0020), next_rx));
        assert!(!within_receive_window(seq(0x0021), next_rx));
        assert!(!within_receive_window(seq(5), seq(10)));
        assert!(within_receive_window(seq(10), seq(10)));
    }

    #[test]
    fn delivery_continues_across_the_16_bit_wrap() {
        let mut c = Columns::default();
        let index = c.ensure(&link()).unwrap();
        c.set_next_expected(index, seq(0xFFFE));
        assert_eq!(feed(&mut c, 0xFFFF, b"y").0, ReceiveOutcome::Buffered);
        assert_eq!(feed(&mut c, 0x0000, b"z").0, ReceiveOutcome::Buffered);
        let (outcome, delivered) = feed(&mut c, 0xFFFE, b"x");
        assert_eq!(outcome, ReceiveOutcome::Delivered { count: 3 });
        assert_eq!(delivered, vec![b"x".to_vec(), b"y".to_vec(), b"z".to_vec()]);
    }

    #[test]
    fn a_full_reorder_buffer_drops_unproven() {
        // REORDER_CAP = 2 here so the third out-of-order arrival overflows.
        let mut c: FixedArrayChannelColumns<1, 2, 16> = FixedArrayChannelColumns::default();
        assert_eq!(
            receive(&mut c, &link(), seq(1), mt(1), b"b", |_, _| {}),
            ReceiveOutcome::Buffered
        );
        assert_eq!(
            receive(&mut c, &link(), seq(2), mt(2), b"c", |_, _| {}),
            ReceiveOutcome::Buffered
        );
        let outcome = receive(&mut c, &link(), seq(3), mt(3), b"d", |_, _| {});
        assert_eq!(outcome, ReceiveOutcome::BufferFull);
        assert!(!outcome.owes_proof());
    }

    #[test]
    fn a_full_channel_table_leaves_an_arrival_untracked() {
        let mut c: FixedArrayChannelColumns<1, 4, 16> = FixedArrayChannelColumns::default();
        // One link fills the single-slot table.
        assert_eq!(
            receive(
                &mut c,
                &LinkId::new([1; 16]),
                seq(0),
                mt(0),
                b"a",
                |_, _| {}
            ),
            ReceiveOutcome::Delivered { count: 1 }
        );
        // A second link has nowhere to be tracked.
        let outcome = receive(
            &mut c,
            &LinkId::new([2; 16]),
            seq(0),
            mt(0),
            b"a",
            |_, _| {},
        );
        assert_eq!(outcome, ReceiveOutcome::Untracked);
        assert!(!outcome.owes_proof());
    }

    #[test]
    fn the_two_drop_cases_withhold_the_proof_others_owe_it() {
        assert!(ReceiveOutcome::Delivered { count: 1 }.owes_proof());
        assert!(ReceiveOutcome::Buffered.owes_proof());
        assert!(ReceiveOutcome::AlreadyHave.owes_proof());
        assert!(ReceiveOutcome::OutOfWindow.owes_proof());
        assert!(!ReceiveOutcome::BufferFull.owes_proof());
        assert!(!ReceiveOutcome::Untracked.owes_proof());
    }
}
