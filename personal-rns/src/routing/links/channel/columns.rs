//! The storage abstraction for per-link channel state — modelled on
//! [`ResourceColumns`](crate::routing::links::resources::table::ResourceColumns):
//! thin, index-based, SoA accessors, with the receive algorithm living above in
//! [`receive`](super::receive). The recipe chooses the backend — a fixed inline
//! array for embedded, a boxed `FixedHeap` (PSRAM) for scale, a growable heap
//! for std, even a zero-reorder "require in-order, drop-and-retransmit" strategy
//! — so the reorder *policy*, not just its allocation, stays swappable and no
//! const generic reaches engine logic. The transmit side adds, per channel, the
//! next sequence to stamp and a ring of sent-but-unacked messages awaiting their
//! proof; the send algorithm lives above in [`send`](super::send).

use super::{ChannelSequence, MessageType};
use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::routing::dedup::PacketHash;
use crate::routing::links::LinkId;

/// Whether an out-of-order arrival found room in a channel's reorder buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferOutcome {
    Stored,
    Full,
}

/// Whether a sent message found room in a channel's outstanding (unacked) ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxOutcome {
    Tracked,
    Full,
}

/// The channel table had no slot for a new link's channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureChannelError {
    TableFull,
}

pub trait ChannelColumns {
    /// The most channels this backend tracks at once.
    fn capacity(&self) -> usize;
    /// How many channels are live.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The slot index of `link`'s channel, if it has one.
    fn index_of(&self, link: &LinkId) -> Option<usize>;
    /// `link`'s channel slot, creating it (next-expected `0`, empty buffer) on
    /// first use. Returns the existing slot if one is already open.
    fn ensure(&mut self, link: &LinkId) -> Result<usize, EnsureChannelError>;
    /// Drop a torn-down link's channel state.
    fn close(&mut self, link: &LinkId);

    /// The next sequence the channel at `index` expects to deliver.
    fn next_expected(&self, index: usize) -> ChannelSequence;
    fn set_next_expected(&mut self, index: usize, sequence: ChannelSequence);

    /// The sequences currently held in the channel's reorder buffer, packed.
    fn buffered_sequences(&self, index: usize) -> &[ChannelSequence];
    /// The message type of the buffered entry at sub-index `sub`.
    fn buffered_message_type(&self, index: usize, sub: usize) -> MessageType;
    /// The body of the buffered entry at sub-index `sub`, borrowed from storage.
    fn buffered_payload(&self, index: usize, sub: usize) -> &[u8];
    /// Store an out-of-order arrival in the channel's reorder buffer.
    fn push_buffered(
        &mut self,
        index: usize,
        sequence: ChannelSequence,
        message_type: MessageType,
        payload: &[u8],
    ) -> BufferOutcome;
    /// Remove the buffered entry at sub-index `sub` (it has been delivered).
    fn swap_remove_buffered(&mut self, index: usize, sub: usize);

    /// The next sequence the channel at `index` will stamp on an outbound message.
    fn next_tx_sequence(&self, index: usize) -> ChannelSequence;
    fn set_next_tx_sequence(&mut self, index: usize, sequence: ChannelSequence);

    /// How many sent messages are still awaiting their proof — the channel's
    /// in-flight count the send window is measured against.
    fn outstanding_count(&self, index: usize) -> usize;
    /// The packet hashes of the outstanding sends, packed, for matching an
    /// arriving proof to the message it acks.
    fn outstanding_packet_hashes(&self, index: usize) -> &[PacketHash];
    /// The command id of the outstanding send at sub-index `sub`.
    fn outstanding_command_id(&self, index: usize, sub: usize) -> CommandId;
    /// When the outstanding send at sub-index `sub` was transmitted.
    fn outstanding_sent_at(&self, index: usize, sub: usize) -> InstantMillis;
    /// Track a freshly sent message awaiting its proof.
    fn push_outstanding(
        &mut self,
        index: usize,
        packet_hash: PacketHash,
        command_id: CommandId,
        sent_at: InstantMillis,
    ) -> TxOutcome;
    /// Remove the outstanding send at sub-index `sub` (its proof arrived).
    fn retire_outstanding(&mut self, index: usize, sub: usize);
}
