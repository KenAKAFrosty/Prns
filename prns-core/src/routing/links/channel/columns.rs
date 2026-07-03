//! The recipe chooses the backend, so the reorder policy itself stays swappable
//! and no const generic reaches engine logic.

use super::{ChannelSequence, ChannelWindow, MessageType};
use crate::engine::commands::CommandId;
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

/// The envelope's sequence/type/body and the IV it was sealed under reproduce the
/// exact ciphertext on retransmit, and so the same packet hash.
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

pub trait ChannelColumns {
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
}
