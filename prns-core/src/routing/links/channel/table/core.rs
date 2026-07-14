use super::super::{ChannelSequence, ChannelWindow, MessageType};
use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::dedup::PacketHash;
use crate::routing::links::LinkId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferOutcome {
    Stored,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxOutcome {
    Tracked,
    Full,
}

pub struct OutstandingSend<'a> {
    pub packet_hash: PacketHash,
    pub command_id: CommandId,
    pub sequence: ChannelSequence,
    pub message_type: MessageType,
    pub body: &'a [u8],
    pub iv: [u8; 16],
    pub sent_at: InstantMillis,
    pub timeout_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureChannelError {
    TableFull,
}

/// One outstanding deadline changed; what happened and with which values, so the channel's cached earliest can absorb it without walking the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutstandingTimeoutChange {
    Pushed(InstantMillis),
    Rewritten {
        previous: InstantMillis,
        new: InstantMillis,
    },
    Retired(InstantMillis),
}

pub trait ChannelTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index_of(&self, link: &LinkId) -> Option<usize>;
    fn link_at(&self, index: usize) -> LinkId;
    fn ensure(&mut self, link: &LinkId) -> Result<usize, EnsureChannelError>;
    fn close(&mut self, link: &LinkId);

    fn next_expected(&self, index: usize) -> ChannelSequence;
    fn set_next_expected(&mut self, index: usize, sequence: ChannelSequence);

    fn buffered_sequences(&self, index: usize) -> &[ChannelSequence];
    fn buffered_message_type(&self, index: usize, sub: usize) -> MessageType;
    fn buffered_payload(&self, index: usize, sub: usize) -> &[u8];
    fn push_buffered(
        &mut self,
        index: usize,
        sequence: ChannelSequence,
        message_type: MessageType,
        payload: &[u8],
    ) -> BufferOutcome;
    fn swap_remove_buffered(&mut self, index: usize, sub: usize);

    fn next_tx_sequence(&self, index: usize) -> ChannelSequence;
    fn set_next_tx_sequence(&mut self, index: usize, sequence: ChannelSequence);

    fn window(&self, index: usize) -> ChannelWindow;
    fn set_window(&mut self, index: usize, window: ChannelWindow);

    fn outstanding_count(&self, index: usize) -> usize;
    fn outstanding_packet_hashes(&self, index: usize) -> &[PacketHash];
    fn outstanding_command_id(&self, index: usize, sub: usize) -> CommandId;
    fn outstanding_sent_at(&self, index: usize, sub: usize) -> InstantMillis;
    fn outstanding_timeout_at(&self, index: usize, sub: usize) -> InstantMillis;
    fn set_outstanding_timeout_at(&mut self, index: usize, sub: usize, timeout_at: InstantMillis);
    fn outstanding_tries(&self, index: usize, sub: usize) -> u8;
    fn set_outstanding_tries(&mut self, index: usize, sub: usize, tries: u8);
    fn outstanding_sequence(&self, index: usize, sub: usize) -> ChannelSequence;
    fn outstanding_message_type(&self, index: usize, sub: usize) -> MessageType;
    fn outstanding_body(&self, index: usize, sub: usize) -> &[u8];
    fn outstanding_iv(&self, index: usize, sub: usize) -> [u8; 16];
    fn push_outstanding(&mut self, index: usize, send: OutstandingSend<'_>) -> TxOutcome;
    fn retire_outstanding(&mut self, index: usize, sub: usize);

    /// The channel's cached earliest outstanding deadline: a due-scan skips any channel whose value sits in the future.
    fn channel_earliest_tx_timeout(&self, index: usize) -> Option<InstantMillis>;
    fn set_channel_earliest_tx_timeout(&mut self, index: usize, earliest: Option<InstantMillis>);

    fn rescan_channel_earliest_tx_timeout(&mut self, index: usize) {
        let earliest = (0..self.outstanding_count(index))
            .map(|sub| self.outstanding_timeout_at(index, sub))
            .min();
        self.set_channel_earliest_tx_timeout(index, earliest);
    }

    /// Folds one deadline mutation into the channel's cached earliest: a push or a lowered deadline settles in place, while raising or retiring the current holder re-walks that one ring.
    /// Every mutator routes through here, so the cache never desyncs and never costs a cross-channel scan.
    fn absorb_outstanding_timeout_change(
        &mut self,
        index: usize,
        change: OutstandingTimeoutChange,
    ) {
        let earliest = self.channel_earliest_tx_timeout(index);
        match change {
            OutstandingTimeoutChange::Pushed(new) => {
                if earliest.is_none_or(|current| new < current) {
                    self.set_channel_earliest_tx_timeout(index, Some(new));
                }
            }
            OutstandingTimeoutChange::Rewritten { previous, new } => match earliest {
                Some(current) if new <= current => {
                    self.set_channel_earliest_tx_timeout(index, Some(new));
                }
                Some(current) if previous == current => {
                    self.rescan_channel_earliest_tx_timeout(index);
                }
                Some(_) => {}
                None => self.set_channel_earliest_tx_timeout(index, Some(new)),
            },
            OutstandingTimeoutChange::Retired(retired) => {
                if earliest == Some(retired) {
                    self.rescan_channel_earliest_tx_timeout(index);
                }
            }
        }
    }

    /// The debug oracle for the cached earliests: the full channels × ring walk.
    fn scan_earliest_tx_timeout(&self) -> Option<InstantMillis> {
        (0..self.len())
            .flat_map(|index| {
                (0..self.outstanding_count(index))
                    .map(move |sub| self.outstanding_timeout_at(index, sub))
            })
            .min()
    }

    fn earliest_tx_timeout_at(&self) -> Option<InstantMillis> {
        let earliest = (0..self.len())
            .filter_map(|index| self.channel_earliest_tx_timeout(index))
            .min();
        debug_assert_eq!(
            earliest,
            self.scan_earliest_tx_timeout(),
            "a channel's cached earliest tx timeout desynced from its outstanding deadlines"
        );
        earliest
    }

    fn first_due_channel(&self, now: InstantMillis) -> Option<usize> {
        (0..self.len()).find(|&index| {
            self.channel_earliest_tx_timeout(index)
                .is_some_and(|at| at <= now)
        })
    }
}
