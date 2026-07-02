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

use super::{ChannelSequence, ChannelWindow, MessageType};
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

/// Everything a channel keeps about a sent message so it can match the proof
/// that acks it and re-seal an identical packet if it has to retransmit: the
/// envelope's sequence/type/body and the IV it was sealed under reproduce the
/// exact ciphertext (and so the same packet hash), while `timeout_at` arms the
/// retry watchdog.
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
    /// The link the channel at `index` belongs to — the timeout watchdog scans by
    /// index and needs the link back to re-seal and to tear down.
    fn link_at(&self, index: usize) -> LinkId;
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

    /// The channel's adaptive send window — the in-flight allowance the send
    /// algorithm gates against, grown on each ack and shrunk on each loss.
    fn window(&self, index: usize) -> ChannelWindow;
    fn set_window(&mut self, index: usize, window: ChannelWindow);

    /// How many sent messages are still awaiting their proof — the channel's
    /// in-flight count the send window is measured against.
    fn outstanding_count(&self, index: usize) -> usize;
    /// The packet hashes of the outstanding sends, packed, for matching an
    /// arriving proof to the message it acks.
    fn outstanding_packet_hashes(&self, index: usize) -> &[PacketHash];
    /// The command id of the outstanding send at sub-index `sub`.
    fn outstanding_command_id(&self, index: usize, sub: usize) -> CommandId;
    /// When the outstanding send at sub-index `sub` was first transmitted.
    fn outstanding_sent_at(&self, index: usize, sub: usize) -> InstantMillis;
    /// When the outstanding send at sub-index `sub` next times out.
    fn outstanding_timeout_at(&self, index: usize, sub: usize) -> InstantMillis;
    fn set_outstanding_timeout_at(&mut self, index: usize, sub: usize, timeout_at: InstantMillis);
    /// How many times the outstanding send at sub-index `sub` has been retransmitted.
    fn outstanding_tries(&self, index: usize, sub: usize) -> u8;
    fn set_outstanding_tries(&mut self, index: usize, sub: usize, tries: u8);
    /// The envelope sequence/type/body and IV of the outstanding send at sub-index
    /// `sub`, enough to re-seal a byte-identical retransmission.
    fn outstanding_sequence(&self, index: usize, sub: usize) -> ChannelSequence;
    fn outstanding_message_type(&self, index: usize, sub: usize) -> MessageType;
    fn outstanding_body(&self, index: usize, sub: usize) -> &[u8];
    fn outstanding_iv(&self, index: usize, sub: usize) -> [u8; 16];
    /// Track a freshly sent message awaiting its proof.
    fn push_outstanding(&mut self, index: usize, send: OutstandingSend<'_>) -> TxOutcome;
    /// Remove the outstanding send at sub-index `sub` (its proof arrived).
    fn retire_outstanding(&mut self, index: usize, sub: usize);
}
