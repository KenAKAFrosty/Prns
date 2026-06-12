//! Outstanding receipts for packets we sent expecting proof — RNS 1.3.1
//! `Transport.receipts` + `PacketReceipt`. A row lives from send until its
//! proof arrives or its timeout passes; removal IS the settlement, so every
//! tracked send settles exactly once. The peer's signing key is copied in at
//! send time, so proof validation never depends on the route surviving.

mod impls;

pub use impls::*;

use crate::crypto::{ed25519_verify, Ed25519Signature};
use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;

/// Which command a receipt settles as when it concludes — the store tracks
/// more than one kind of send in one table (RNS 1.3.1 keeps every
/// `PacketReceipt` in the one `Transport.receipts` list the same way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptKind {
    SendSingle,
    SendLink,
    SendRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutstandingReceipt {
    pub packet_hash: PacketHash,
    pub command_id: CommandId,
    pub kind: ReceiptKind,
    pub peer_signing_key: IdentitySigningPublicKey,
    pub sent_at: InstantMillis,
    pub timeout_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvenReceipt {
    pub command_id: CommandId,
    pub kind: ReceiptKind,
    pub sent_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiredReceipt {
    pub command_id: CommandId,
    pub kind: ReceiptKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CulledReceipt {
    pub command_id: CommandId,
    pub kind: ReceiptKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackReceiptError {
    TableFull,
}

pub trait ReceiptColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn packet_hashes(&self) -> &[PacketHash];
    fn command_ids(&self) -> &[CommandId];
    fn kinds(&self) -> &[ReceiptKind];
    fn signing_keys(&self) -> &[IdentitySigningPublicKey];
    fn sent_ats(&self) -> &[InstantMillis];
    fn timeout_ats(&self) -> &[InstantMillis];

    fn push(&mut self, receipt: OutstandingReceipt) -> Result<usize, TrackReceiptError>;
    /// Removal must preserve insertion order (shift, not swap): index order IS
    /// the implicit-proof trial order, and proofs return in send order over a
    /// FIFO wire — so the match is almost always the first trial. A swap-style
    /// remove scrambles that order and was measured paying ~5 full Ed25519
    /// verifies per proof at window depth where one suffices. The reference
    /// holds the same invariant for free (`Transport.receipts` is an
    /// append-only Python list).
    fn remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct Receipts<C: ReceiptColumns> {
    columns: C,
}

impl<C: ReceiptColumns> Receipts<C> {
    /// A full table culls its stalest receipt to make room — RNS 1.3.1
    /// `Transport.jobs()` does the same past `MAX_RECEIPTS`, always favoring
    /// the new send. The culled command still settles, typed, through the
    /// returned receipt.
    pub fn track(&mut self, receipt: OutstandingReceipt) -> Option<CulledReceipt> {
        let mut culled = None;
        if self.columns.len() >= self.columns.capacity() {
            culled = self.cull_stalest();
        }
        match self.columns.push(receipt) {
            Ok(_) => culled,
            Err(TrackReceiptError::TableFull) => Some(CulledReceipt {
                command_id: receipt.command_id,
                kind: receipt.kind,
            }),
        }
    }

    fn cull_stalest(&mut self) -> Option<CulledReceipt> {
        let index = self
            .columns
            .sent_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, sent_at)| **sent_at)
            .map(|(index, _)| index)?;
        let culled = CulledReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
        };
        self.columns.remove(index);
        Some(culled)
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        self.columns.timeout_ats().iter().min().copied()
    }

    pub fn pop_expired(&mut self, now: InstantMillis) -> Option<ExpiredReceipt> {
        let index = self
            .columns
            .timeout_ats()
            .iter()
            .position(|timeout_at| *timeout_at <= now)?;
        let expired = ExpiredReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
        };
        self.columns.remove(index);
        Some(expired)
    }

    /// RNS 1.3.1 explicit proof: full packet hash named in the proof, so match
    /// the row first, then verify. A failed signature leaves the row
    /// outstanding (reference parity; the timeout still owns it).
    pub fn settle_by_explicit_proof(
        &mut self,
        proof_hash: &PacketHash,
        signature: &Ed25519Signature,
    ) -> Option<ProvenReceipt> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.kinds().get(*index) != Some(&ReceiptKind::SendRequest)
                && self.columns.packet_hashes().get(*index) == Some(proof_hash)
        })?;
        self.settle_verified(index, signature)
    }

    /// RNS 1.3.1 implicit proof: a bare signature, trial-verified against
    /// every outstanding row (Packet.py validates against each receipt), in
    /// insertion order — which [`ReceiptColumns::remove`]'s ordering invariant
    /// makes the send order, so a FIFO wire's proofs match on the first trial.
    pub fn settle_by_implicit_proof(
        &mut self,
        signature: &Ed25519Signature,
    ) -> Option<ProvenReceipt> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.kinds().get(*index) != Some(&ReceiptKind::SendRequest)
                && self.row_signature_valid(*index, signature)
        })?;
        let proven = ProvenReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
            sent_at: *self.columns.sent_ats().get(index)?,
        };
        self.columns.remove(index);
        Some(proven)
    }

    /// A response names its request by the truncated hash of the request
    /// packet — the first sixteen bytes of the hash already tracked here. The
    /// session key authenticated the response, so no signature gates this.
    pub fn settle_by_request_id(&mut self, request_id: &[u8; 16]) -> Option<ProvenReceipt> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.kinds().get(*index) == Some(&ReceiptKind::SendRequest)
                && self
                    .columns
                    .packet_hashes()
                    .get(*index)
                    .is_some_and(|hash| &hash.as_bytes()[..16] == request_id)
        })?;
        let proven = ProvenReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
            sent_at: *self.columns.sent_ats().get(index)?,
        };
        self.columns.remove(index);
        Some(proven)
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    fn settle_verified(
        &mut self,
        index: usize,
        signature: &Ed25519Signature,
    ) -> Option<ProvenReceipt> {
        if !self.row_signature_valid(index, signature) {
            return None;
        }
        let proven = ProvenReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
            sent_at: *self.columns.sent_ats().get(index)?,
        };
        self.columns.remove(index);
        Some(proven)
    }

    fn row_signature_valid(&self, index: usize, signature: &Ed25519Signature) -> bool {
        let (Some(packet_hash), Some(signing_key)) = (
            self.columns.packet_hashes().get(index),
            self.columns.signing_keys().get(index),
        ) else {
            return false;
        };
        ed25519_verify(signing_key.as_ed25519(), packet_hash.as_bytes(), signature).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    type TestReceipts = Receipts<FixedReceiptColumns<3>>;

    fn signer(fill: u8) -> (Ed25519SecretKey, IdentitySigningPublicKey) {
        let secret = Ed25519SecretKey::new([fill; 32]);
        let public = IdentitySigningPublicKey::new(ed25519_public_key(&secret));
        (secret, public)
    }

    fn outstanding(
        hash_fill: u8,
        command_id: u64,
        key: IdentitySigningPublicKey,
        sent_at: u64,
        timeout_at: u64,
    ) -> OutstandingReceipt {
        OutstandingReceipt {
            packet_hash: PacketHash::new([hash_fill; 32]),
            command_id: CommandId(command_id),
            kind: ReceiptKind::SendSingle,
            peer_signing_key: key,
            sent_at: InstantMillis(sent_at),
            timeout_at: InstantMillis(timeout_at),
        }
    }

    #[test]
    fn a_full_table_culls_its_stalest_receipt_for_the_new_send() {
        let (_, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        assert_eq!(receipts.track(outstanding(1, 1, key, 300, 7_000)), None);
        assert_eq!(receipts.track(outstanding(2, 2, key, 100, 7_000)), None);
        assert_eq!(receipts.track(outstanding(3, 3, key, 200, 7_000)), None);

        assert_eq!(
            receipts.track(outstanding(4, 4, key, 400, 7_000)),
            Some(CulledReceipt {
                command_id: CommandId(2),
                kind: ReceiptKind::SendSingle,
            }),
            "the stalest send (earliest sent_at) is culled, not the newest",
        );
        assert_eq!(receipts.len(), 3);
        assert_eq!(
            receipts
                .pop_expired(InstantMillis(8_000))
                .map(|r| r.command_id),
            Some(CommandId(1)),
        );
    }

    #[test]
    fn the_earliest_timeout_drives_the_wakeup() {
        let (_, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        assert_eq!(receipts.earliest_timeout_at(), None);
        assert_eq!(receipts.track(outstanding(1, 1, key, 100, 9_000)), None);
        assert_eq!(receipts.track(outstanding(2, 2, key, 200, 7_000)), None);
        assert_eq!(receipts.earliest_timeout_at(), Some(InstantMillis(7_000)));
    }

    #[test]
    fn expiry_pops_every_due_receipt_and_leaves_the_rest() {
        let (_, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        assert_eq!(receipts.track(outstanding(1, 1, key, 100, 5_000)), None);
        assert_eq!(receipts.track(outstanding(2, 2, key, 100, 9_000)), None);
        assert_eq!(receipts.track(outstanding(3, 3, key, 100, 5_500)), None);

        let mut expired = std::vec::Vec::new();
        while let Some(receipt) = receipts.pop_expired(InstantMillis(6_000)) {
            expired.push(receipt.command_id);
        }
        expired.sort_unstable_by_key(|id| id.0);
        assert_eq!(expired, std::vec![CommandId(1), CommandId(3)]);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts.earliest_timeout_at(), Some(InstantMillis(9_000)));
    }

    #[test]
    fn an_explicit_proof_settles_its_named_receipt() {
        let (secret, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        assert_eq!(receipts.track(outstanding(1, 1, key, 100, 9_000)), None);
        assert_eq!(receipts.track(outstanding(2, 2, key, 250, 9_000)), None);

        let named = PacketHash::new([2; 32]);
        let signature = ed25519_sign(&secret, named.as_bytes());
        assert_eq!(
            receipts.settle_by_explicit_proof(&named, &signature),
            Some(ProvenReceipt {
                command_id: CommandId(2),
                kind: ReceiptKind::SendSingle,
                sent_at: InstantMillis(250),
            }),
        );
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts.settle_by_explicit_proof(&named, &signature),
            None,
            "a settled receipt is gone — the proof cannot settle twice",
        );
    }

    #[test]
    fn a_bad_signature_leaves_the_receipt_outstanding_for_its_timeout() {
        let (_, key) = signer(0x21);
        let (stranger_secret, _) = signer(0x77);
        let mut receipts = TestReceipts::default();
        assert_eq!(receipts.track(outstanding(1, 1, key, 100, 9_000)), None);

        let named = PacketHash::new([1; 32]);
        let forged = ed25519_sign(&stranger_secret, named.as_bytes());
        assert_eq!(receipts.settle_by_explicit_proof(&named, &forged), None);
        assert_eq!(receipts.len(), 1);
    }

    #[test]
    fn an_implicit_proof_finds_its_receipt_by_trial_verification() {
        let (first_secret, first_key) = signer(0x21);
        let (second_secret, second_key) = signer(0x42);
        let mut receipts = TestReceipts::default();
        assert_eq!(
            receipts.track(outstanding(1, 1, first_key, 100, 9_000)),
            None
        );
        assert_eq!(
            receipts.track(outstanding(2, 2, second_key, 300, 9_000)),
            None
        );

        let signature = ed25519_sign(&second_secret, PacketHash::new([2; 32]).as_bytes());
        assert_eq!(
            receipts.settle_by_implicit_proof(&signature),
            Some(ProvenReceipt {
                command_id: CommandId(2),
                kind: ReceiptKind::SendSingle,
                sent_at: InstantMillis(300),
            }),
        );
        assert_eq!(receipts.len(), 1);

        let stale = ed25519_sign(&first_secret, PacketHash::new([9; 32]).as_bytes());
        assert_eq!(receipts.settle_by_implicit_proof(&stale), None);
    }
}
