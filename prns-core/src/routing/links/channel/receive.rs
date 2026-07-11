//! RNS 1.3.5 `Channel._receive`'s window validation, duplicate rejection, and contiguous in-order drain, ported to integer sequence arithmetic (to match our integer units of ms).

use super::table::{BufferOutcome, ChannelTable, EnsureChannelError};
use super::MessageType;
use super::{ChannelSequence, SEQUENCE_MODULUS};
use crate::routing::links::LinkId;

/// RNS 1.3.5 `Channel.WINDOW_MAX` (`= WINDOW_MAX_FAST`): the furthest ahead a well-behaved peer ever transmits / the most out-of-order messages worth holding.
pub const WINDOW_MAX_MESSAGES: u16 = 48;

/// RNS 1.3.5 `Channel._receive`'s window guard
pub fn within_receive_window(sequence: ChannelSequence, next_rx: ChannelSequence) -> bool {
    if sequence.0 >= next_rx.0 {
        return true;
    }
    let window_overflow =
        ((next_rx.0 as u32 + WINDOW_MAX_MESSAGES as u32) % SEQUENCE_MODULUS) as u16;
    window_overflow < next_rx.0 && sequence.0 <= window_overflow
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Delivered { count: u16 },
    Buffered,
    AlreadyHave,
    OutOfWindow,
    BufferFull,
    Untracked,
}

impl ReceiveOutcome {
    pub const fn owes_proof(self) -> bool {
        !matches!(self, Self::BufferFull | Self::Untracked)
    }
}

pub fn receive<C: ChannelTable>(
    table: &mut C,
    link: &LinkId,
    sequence: ChannelSequence,
    message_type: MessageType,
    payload: &[u8],
    mut on_deliver: impl FnMut(MessageType, &[u8]),
) -> ReceiveOutcome {
    let index = match table.ensure(link) {
        Ok(index) => index,
        Err(EnsureChannelError::TableFull) => return ReceiveOutcome::Untracked,
    };

    let mut next_rx = table.next_expected(index);
    if sequence == next_rx {
        on_deliver(message_type, payload);
        next_rx = next_rx.next();
        let mut count: u16 = 1;
        while let Some(sub) = table
            .buffered_sequences(index)
            .iter()
            .position(|buffered| *buffered == next_rx)
        {
            let message_type = table.buffered_message_type(index, sub);
            on_deliver(message_type, table.buffered_payload(index, sub));
            table.swap_remove_buffered(index, sub);
            next_rx = next_rx.next();
            count += 1;
        }
        table.set_next_expected(index, next_rx);
        return ReceiveOutcome::Delivered { count };
    }

    if !within_receive_window(sequence, next_rx) {
        return ReceiveOutcome::OutOfWindow;
    }
    if table.buffered_sequences(index).contains(&sequence) {
        return ReceiveOutcome::AlreadyHave;
    }
    match table.push_buffered(index, sequence, message_type, payload) {
        BufferOutcome::Stored => ReceiveOutcome::Buffered,
        BufferOutcome::Full => ReceiveOutcome::BufferFull,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::channel::table::impls::FixedArrayChannelTable;
    use std::vec::Vec;

    type Table = FixedArrayChannelTable<2, 8, 16>;

    fn link() -> LinkId {
        LinkId::new([0xAB; 16])
    }
    fn seq(n: u16) -> ChannelSequence {
        ChannelSequence(n)
    }
    fn mt(n: u16) -> MessageType {
        MessageType(n)
    }

    fn feed(table: &mut Table, sequence: u16, body: &[u8]) -> (ReceiveOutcome, Vec<Vec<u8>>) {
        let mut delivered = Vec::new();
        let outcome = receive(
            table,
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
        let mut c = Table::default();
        let (o0, d0) = feed(&mut c, 0, b"a");
        let (o1, d1) = feed(&mut c, 1, b"b");
        assert_eq!(o0, ReceiveOutcome::Delivered { count: 1 });
        assert_eq!(o1, ReceiveOutcome::Delivered { count: 1 });
        assert_eq!(d0, vec![b"a".to_vec()]);
        assert_eq!(d1, vec![b"b".to_vec()]);
    }

    #[test]
    fn an_out_of_order_arrival_waits_until_the_gap_fills() {
        let mut c = Table::default();
        assert_eq!(feed(&mut c, 1, b"b").0, ReceiveOutcome::Buffered);
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::Buffered);
        let (outcome, delivered) = feed(&mut c, 0, b"a");
        assert_eq!(outcome, ReceiveOutcome::Delivered { count: 3 });
        assert_eq!(delivered, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn a_buffered_duplicate_is_not_redelivered() {
        let mut c = Table::default();
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::Buffered);
        assert_eq!(feed(&mut c, 2, b"c").0, ReceiveOutcome::AlreadyHave);
        let (_, delivered) = feed(&mut c, 0, b"a");
        assert_eq!(delivered, vec![b"a".to_vec()]); // only 0; the gap at 1 still holds 2 back
    }

    #[test]
    fn an_already_delivered_sequence_is_dropped_out_of_window() {
        let mut c = Table::default();
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
        let mut c = Table::default();
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
        let mut c: FixedArrayChannelTable<1, 2, 16> = FixedArrayChannelTable::default();
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
        let mut c: FixedArrayChannelTable<1, 4, 16> = FixedArrayChannelTable::default();
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
