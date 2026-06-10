use alloc::vec::Vec;

use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptColumns, TrackReceiptError};

/// RNS 1.3.1 `Transport.MAX_RECEIPTS`: past this, the wrapper culls the
/// stalest receipt so the new send always proceeds — and the culled command
/// settles typed instead of silently.
pub const DEFAULT_MAX_OUTSTANDING_RECEIPTS: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapReceiptColumns {
    packet_hashes: Vec<PacketHash>,
    command_ids: Vec<CommandId>,
    signing_keys: Vec<IdentitySigningPublicKey>,
    sent_ats: Vec<InstantMillis>,
    timeout_ats: Vec<InstantMillis>,
}

impl ReceiptColumns for HeapReceiptColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_OUTSTANDING_RECEIPTS
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
        if self.packet_hashes.len() >= DEFAULT_MAX_OUTSTANDING_RECEIPTS {
            return Err(TrackReceiptError::TableFull);
        }
        let index = self.packet_hashes.len();
        self.packet_hashes.push(receipt.packet_hash);
        self.command_ids.push(receipt.command_id);
        self.signing_keys.push(receipt.peer_signing_key);
        self.sent_ats.push(receipt.sent_at);
        self.timeout_ats.push(receipt.timeout_at);
        Ok(index)
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.packet_hashes.len() {
            return;
        }
        self.packet_hashes.swap_remove(index);
        self.command_ids.swap_remove(index);
        self.signing_keys.swap_remove(index);
        self.sent_ats.swap_remove(index);
        self.timeout_ats.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_to_the_reference_cap_then_reports_full_for_the_wrapper_to_cull() {
        let key = IdentitySigningPublicKey::new(crate::crypto::ed25519_public_key(
            &crate::crypto::Ed25519SecretKey::new([0x21; 32]),
        ));
        let mut columns = HeapReceiptColumns::default();
        for i in 0..DEFAULT_MAX_OUTSTANDING_RECEIPTS {
            let receipt = OutstandingReceipt {
                packet_hash: PacketHash::new([(i % 251) as u8; 32]),
                command_id: CommandId(i as u64),
                peer_signing_key: key,
                sent_at: InstantMillis(0),
                timeout_at: InstantMillis(7_000),
            };
            assert_eq!(columns.push(receipt), Ok(i));
        }
        let overflow = OutstandingReceipt {
            packet_hash: PacketHash::new([0xFF; 32]),
            command_id: CommandId(9_999),
            peer_signing_key: key,
            sent_at: InstantMillis(0),
            timeout_at: InstantMillis(7_000),
        };
        assert_eq!(columns.push(overflow), Err(TrackReceiptError::TableFull));
        assert_eq!(columns.len(), DEFAULT_MAX_OUTSTANDING_RECEIPTS);
    }
}
