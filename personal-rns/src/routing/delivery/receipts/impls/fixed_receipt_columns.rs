use heapless::Vec as HeaplessVec;

use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{
    OutstandingReceipt, ReceiptColumns, ReceiptKind, TrackReceiptError,
};

#[derive(Debug, Default)]
pub struct FixedReceiptColumns<const MAX_OUTSTANDING_RECEIPTS: usize> {
    packet_hashes: HeaplessVec<PacketHash, MAX_OUTSTANDING_RECEIPTS>,
    command_ids: HeaplessVec<CommandId, MAX_OUTSTANDING_RECEIPTS>,
    kinds: HeaplessVec<ReceiptKind, MAX_OUTSTANDING_RECEIPTS>,
    signing_keys: HeaplessVec<IdentitySigningPublicKey, MAX_OUTSTANDING_RECEIPTS>,
    sent_ats: HeaplessVec<InstantMillis, MAX_OUTSTANDING_RECEIPTS>,
    timeout_ats: HeaplessVec<InstantMillis, MAX_OUTSTANDING_RECEIPTS>,
}

impl<const MAX_OUTSTANDING_RECEIPTS: usize> ReceiptColumns
    for FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>
{
    fn capacity(&self) -> usize {
        MAX_OUTSTANDING_RECEIPTS
    }
    fn len(&self) -> usize {
        self.packet_hashes.len()
    }

    fn packet_hashes(&self) -> &[PacketHash] {
        &self.packet_hashes
    }
    fn command_ids(&self) -> &[CommandId] {
        &self.command_ids
    }
    fn kinds(&self) -> &[ReceiptKind] {
        &self.kinds
    }
    fn signing_keys(&self) -> &[IdentitySigningPublicKey] {
        &self.signing_keys
    }
    fn sent_ats(&self) -> &[InstantMillis] {
        &self.sent_ats
    }
    fn timeout_ats(&self) -> &[InstantMillis] {
        &self.timeout_ats
    }

    fn push(&mut self, receipt: OutstandingReceipt) -> Result<usize, TrackReceiptError> {
        if self.packet_hashes.is_full() {
            return Err(TrackReceiptError::TableFull);
        }
        let index = self.packet_hashes.len();
        let _ = self.packet_hashes.push(receipt.packet_hash);
        let _ = self.command_ids.push(receipt.command_id);
        let _ = self.kinds.push(receipt.kind);
        let _ = self.signing_keys.push(receipt.peer_signing_key);
        let _ = self.sent_ats.push(receipt.sent_at);
        let _ = self.timeout_ats.push(receipt.timeout_at);
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.packet_hashes.len() {
            return;
        }
        self.packet_hashes.swap_remove(index);
        self.command_ids.swap_remove(index);
        self.kinds.swap_remove(index);
        self.signing_keys.swap_remove(index);
        self.sent_ats.swap_remove(index);
        self.timeout_ats.swap_remove(index);
    }
}
