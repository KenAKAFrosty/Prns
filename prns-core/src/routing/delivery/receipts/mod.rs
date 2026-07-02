//! Outstanding receipts for packets we sent expecting proof — RNS 1.3.1
//! `Transport.receipts` + `PacketReceipt`. A row lives from send until its
//! proof arrives or its timeout passes; removal IS the settlement, so every
//! tracked send settles exactly once. The peer's signing key is copied in at
//! send time, so proof validation never depends on the route surviving.

mod impls;

pub use impls::*;

use crate::crypto::{Ed25519Signature, Ed25519Verifier};
use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

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
pub struct DeferredVerify {
    pub proven: ProvenReceipt,
    pub packet_hash: PacketHash,
    pub signing_key: IdentitySigningPublicKey,
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
    /// One-slot cache of the last-used signing key, decompressed. Proofs on a
    /// busy link all name one peer, and an implicit proof trial-verifies one
    /// key against many rows — decompressing per verify was a measured ~8% of
    /// a firehose initiator's CPU.
    verifier_memo: Option<Ed25519Verifier>,
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
        let mut matched = None;
        for index in 0..self.columns.len() {
            if self.columns.kinds().get(index) != Some(&ReceiptKind::SendRequest)
                && self.row_signature_valid(index, signature)
            {
                matched = Some(index);
                break;
            }
        }
        let index = matched?;
        let proven = ProvenReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
            sent_at: *self.columns.sent_ats().get(index)?,
        };
        self.columns.remove(index);
        Some(proven)
    }

    /// The host-threaded-verify counterpart to [`Self::settle_by_explicit_proof`]:
    /// an explicit proof names the packet hash, so the row is found by that hash
    /// (not by trial order). It is read, not removed — the signature check is
    /// deferred to the host pool, and the row stays outstanding until a valid
    /// verdict settles it through [`Self::settle_resolved`], exactly as in
    /// [`Self::resolve_proof_by_destination`].
    pub fn resolve_explicit_for_deferred_verify(
        &mut self,
        proof_hash: &PacketHash,
    ) -> Option<DeferredVerify> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.kinds().get(*index) != Some(&ReceiptKind::SendRequest)
                && self.columns.packet_hashes().get(*index) == Some(proof_hash)
        })?;
        self.read_for_deferred_verify(index)
    }

    /// The host-threaded-verify counterpart to [`Self::settle_by_implicit_proof`]:
    /// it identifies the same candidate but does NOT verify, handing the Ed25519
    /// check to a host crypto pool instead of the engine thread. An implicit proof
    /// is addressed to its packet hash's [`PacketHash::proof_destination`], so that
    /// destination names the exact receipt deterministically (no trial order, no
    /// FIFO guess) — this finds it and returns the [`DeferredVerify`] the pool needs
    /// (the packet hash the signature must cover and the peer's signing key). The
    /// row is left outstanding: it settles only when a valid verdict reaches
    /// [`Self::settle_resolved`], so a forged signature can neither settle it nor
    /// evict it, and the timeout still owns it if no valid proof ever arrives. A
    /// proof addressed to no tracked send resolves to `None` and settles nothing.
    /// Embedded and the default path keep `settle_by_implicit_proof`, which
    /// verifies before it removes.
    pub fn resolve_proof_by_destination(
        &mut self,
        proof_destination: &DestinationHash,
    ) -> Option<DeferredVerify> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.kinds().get(*index) != Some(&ReceiptKind::SendRequest)
                && self
                    .columns
                    .packet_hashes()
                    .get(*index)
                    .map(PacketHash::proof_destination)
                    .as_ref()
                    == Some(proof_destination)
        })?;
        self.read_for_deferred_verify(index)
    }

    fn read_for_deferred_verify(&self, index: usize) -> Option<DeferredVerify> {
        let proven = ProvenReceipt {
            command_id: *self.columns.command_ids().get(index)?,
            kind: *self.columns.kinds().get(index)?,
            sent_at: *self.columns.sent_ats().get(index)?,
        };
        let packet_hash = *self.columns.packet_hashes().get(index)?;
        let signing_key = *self.columns.signing_keys().get(index)?;
        Some(DeferredVerify {
            proven,
            packet_hash,
            signing_key,
        })
    }

    /// Settle the receipt a deferred verify just confirmed valid. The host pool
    /// checked the signature off the engine thread; on success the reactor calls
    /// this to take the row out and conclude the command. Keyed by command id
    /// (unique per send), so a second verdict for the same command — a duplicate
    /// proof, or a verdict that lost a race to the timeout or a cull — finds
    /// nothing and settles nothing, keeping the exactly-once guarantee the inline
    /// `settle_by_*` paths get by removing under the same borrow as the verify. A
    /// `SendRequest` row concludes by request id, never here, so it is skipped.
    pub fn settle_resolved(&mut self, command_id: CommandId) -> Option<ProvenReceipt> {
        let index = (0..self.columns.len()).find(|index| {
            self.columns.command_ids().get(*index) == Some(&command_id)
                && self.columns.kinds().get(*index) != Some(&ReceiptKind::SendRequest)
        })?;
        let proven = ProvenReceipt {
            command_id,
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

    /// Non-removing peek for the resource accept gate: RNS 1.3.1 Link.py:1074
    /// accepts a response resource only when it names a request we actually
    /// sent. The settle itself rides [`Self::settle_by_request_id`] at conclusion.
    pub fn has_pending_request(&self, request_id: &[u8; 16]) -> bool {
        (0..self.columns.len()).any(|index| {
            self.columns.kinds().get(index) == Some(&ReceiptKind::SendRequest)
                && self
                    .columns
                    .packet_hashes()
                    .get(index)
                    .is_some_and(|hash| &hash.as_bytes()[..16] == request_id)
        })
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

    fn row_signature_valid(&mut self, index: usize, signature: &Ed25519Signature) -> bool {
        let (Some(packet_hash), Some(signing_key)) = (
            self.columns.packet_hashes().get(index).copied(),
            self.columns.signing_keys().get(index).copied(),
        ) else {
            return false;
        };
        let key = *signing_key.as_ed25519();
        let memo_holds_key = matches!(&self.verifier_memo, Some(memo) if memo.public_key() == &key);
        if !memo_holds_key {
            let Ok(fresh) = Ed25519Verifier::new(&key) else {
                return false;
            };
            self.verifier_memo = Some(fresh);
        }
        let Some(verifier) = &self.verifier_memo else {
            return false;
        };
        verifier.verify(packet_hash.as_bytes(), signature).is_ok()
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
    fn a_fresh_table_is_empty_and_a_tracked_receipt_fills_it() {
        let (_, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        assert!(receipts.is_empty());
        assert_eq!(receipts.len(), 0);
        assert_eq!(receipts.track(outstanding(1, 1, key, 100, 7_000)), None);
        assert!(!receipts.is_empty());
        assert_eq!(receipts.len(), 1);
    }

    #[test]
    fn deferred_resolve_settles_the_same_receipt_the_inline_implicit_proof_would() {
        let (secret, key) = signer(0x33);
        let signature = ed25519_sign(&secret, &[0x44u8; 32]);

        let mut inline = TestReceipts::default();
        inline.track(outstanding(0x44, 7, key, 100, 7_000));
        let proven = inline
            .settle_by_implicit_proof(&signature)
            .expect("the inline path settles the valid implicit proof");

        let mut deferred = TestReceipts::default();
        deferred.track(outstanding(0x44, 7, key, 100, 7_000));
        let resolved = deferred
            .resolve_proof_by_destination(&PacketHash::new([0x44; 32]).proof_destination())
            .expect("the deferred path resolves the same candidate");

        assert_eq!(
            resolved.proven, proven,
            "deferred resolve yields the settlement the inline verify would have",
        );
        assert_eq!(resolved.packet_hash, PacketHash::new([0x44; 32]));
        assert!(
            Ed25519Verifier::new(resolved.signing_key.as_ed25519())
                .expect("the stored key decompresses")
                .verify(resolved.packet_hash.as_bytes(), &signature)
                .is_ok(),
            "the returned materials are exactly what the pool needs to verify",
        );
        assert_eq!(
            deferred.len(),
            1,
            "resolution identifies the receipt but leaves it outstanding until a valid verdict settles it",
        );
        assert_eq!(
            deferred
                .settle_resolved(resolved.proven.command_id)
                .as_ref(),
            Some(&proven),
            "a valid verdict settles exactly the resolved receipt",
        );
        assert!(
            deferred.is_empty(),
            "the settled receipt is gone, freeing the window slot",
        );
    }

    #[test]
    fn a_resolved_receipt_survives_a_failed_verify_and_still_times_out() {
        let (_, key) = signer(0x55);
        let destination = PacketHash::new([0x55; 32]).proof_destination();
        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x55, 9, key, 100, 7_000));

        let resolved = receipts
            .resolve_proof_by_destination(&destination)
            .expect("the destination identifies the outstanding receipt");
        assert_eq!(resolved.proven.command_id, CommandId(9));
        assert_eq!(
            receipts.len(),
            1,
            "a deferred resolution the pool has not yet confirmed leaves the row in place",
        );

        assert_eq!(
            receipts
                .pop_expired(InstantMillis(8_000))
                .map(|r| r.command_id),
            Some(CommandId(9)),
            "a forged proof whose verify fails never calls settle_resolved, so its receipt is never evicted and still expires on schedule",
        );
    }

    #[test]
    fn settle_resolved_removes_the_resolved_receipt_exactly_once() {
        let (_, key) = signer(0x66);
        let destination = PacketHash::new([0x66; 32]).proof_destination();
        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x66, 4, key, 100, 7_000));
        receipts.track(outstanding(0x77, 5, key, 200, 7_000));

        receipts
            .resolve_proof_by_destination(&destination)
            .expect("the destination identifies its receipt");

        let proven = receipts
            .settle_resolved(CommandId(4))
            .expect("a valid verdict settles the resolved receipt");
        assert_eq!(proven.command_id, CommandId(4));
        assert_eq!(receipts.len(), 1, "only the settled receipt is removed");

        assert!(
            receipts.settle_resolved(CommandId(4)).is_none(),
            "a second verdict for the same command settles nothing, exactly once",
        );
        assert_eq!(
            receipts.len(),
            1,
            "the duplicate verdict removes no other receipt",
        );
    }

    #[test]
    fn deferred_resolution_picks_the_receipt_the_proof_settles_not_the_oldest() {
        let (_, key_a) = signer(0x11);
        let (secret_b, key_b) = signer(0x22);
        let proof_for_b = ed25519_sign(&secret_b, &[0x22u8; 32]);
        let b_destination = PacketHash::new([0x22; 32]).proof_destination();

        let mut inline = TestReceipts::default();
        inline.track(outstanding(0x11, 1, key_a, 100, 7_000));
        inline.track(outstanding(0x22, 2, key_b, 200, 7_000));
        let truth = inline
            .settle_by_implicit_proof(&proof_for_b)
            .expect("the inline trial-verify settles the receipt the proof is for");
        assert_eq!(truth.command_id, CommandId(2));

        let mut deferred = TestReceipts::default();
        deferred.track(outstanding(0x11, 1, key_a, 100, 7_000));
        deferred.track(outstanding(0x22, 2, key_b, 200, 7_000));
        let resolved = deferred
            .resolve_proof_by_destination(&b_destination)
            .expect("the proof's destination identifies its receipt");
        assert_eq!(
            resolved.proven, truth,
            "deferred resolution must settle the receipt the proof is for, never just the oldest",
        );
    }

    #[test]
    fn deferred_resolution_rejects_a_proof_that_matches_no_outstanding_receipt() {
        let (_, key_a) = signer(0x11);
        let (_, key_b) = signer(0x22);
        let stray_destination = PacketHash::new([0x99; 32]).proof_destination();

        let mut deferred = TestReceipts::default();
        deferred.track(outstanding(0x11, 1, key_a, 100, 7_000));
        deferred.track(outstanding(0x22, 2, key_b, 200, 7_000));

        assert!(
            deferred
                .resolve_proof_by_destination(&stray_destination)
                .is_none(),
            "a proof addressed to no tracked send must not settle anything",
        );
        assert_eq!(deferred.len(), 2, "a non-matching proof removes no receipt");
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
    fn alternating_peers_settle_and_the_cached_key_never_cross_authenticates() {
        let (first_secret, first_key) = signer(0x21);
        let (second_secret, second_key) = signer(0x42);
        let mut receipts = TestReceipts::default();
        assert_eq!(
            receipts.track(outstanding(1, 1, first_key, 100, 9_000)),
            None
        );
        assert_eq!(
            receipts.track(outstanding(2, 2, second_key, 200, 9_000)),
            None
        );
        assert_eq!(
            receipts.track(outstanding(3, 3, first_key, 300, 9_000)),
            None
        );

        let first_named = PacketHash::new([1; 32]);
        assert!(receipts
            .settle_by_explicit_proof(
                &first_named,
                &ed25519_sign(&first_secret, first_named.as_bytes()),
            )
            .is_some());

        let cross_named = PacketHash::new([2; 32]);
        assert_eq!(
            receipts.settle_by_explicit_proof(
                &cross_named,
                &ed25519_sign(&first_secret, cross_named.as_bytes()),
            ),
            None,
            "the first peer's freshly cached key must not authenticate the second peer's row",
        );
        assert!(receipts
            .settle_by_explicit_proof(
                &cross_named,
                &ed25519_sign(&second_secret, cross_named.as_bytes()),
            )
            .is_some());

        let last_named = PacketHash::new([3; 32]);
        assert!(receipts
            .settle_by_explicit_proof(
                &last_named,
                &ed25519_sign(&first_secret, last_named.as_bytes()),
            )
            .is_some());
        assert!(receipts.is_empty());
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
