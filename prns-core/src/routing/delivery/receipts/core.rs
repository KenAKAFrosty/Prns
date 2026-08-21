use crate::crypto::{Ed25519Signature, Ed25519Verifier};
use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::routes::RouteEvidenceHandle;
use crate::units::ByteLimit;
use crate::wire::DestinationHash;

/// One table for every send kind, as RNS 1.4.2 keeps every `PacketReceipt` in the one `Transport.receipts` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptKind {
    SendSinglePacket {
        route_evidence: Option<RouteEvidenceHandle>,
    },
    SendToLink(LinkId),
    SendRequest {
        maximum_response_bytes: ByteLimit,
    },
}

const _: () = {
    assert!(core::mem::size_of::<ReceiptKind>() == 24);
};

impl ReceiptKind {
    const fn is_request(self) -> bool {
        matches!(self, Self::SendRequest { .. })
    }
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

/// When the row's timeout fires — or why it will not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptDeadline {
    Due(InstantMillis),
    /// RNS 1.4.2 `RequestReceipt.RECEIVING`: an accepted response resource owns failure for its request, so the row stops expiring.
    /// Every exit of that transfer settles the row — delivery, the resource watchdog, or the re-armed between-segments deadline.
    ClaimedByTransfer,
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

pub trait ReceiptTable {
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
    fn deadlines(&self) -> &[ReceiptDeadline];
    fn set_deadline(&mut self, index: usize, deadline: ReceiptDeadline);

    fn push(&mut self, receipt: OutstandingReceipt) -> Result<usize, TrackReceiptError>;
    /// Removal must preserve insertion order (shift, not swap): index order IS the implicit-proof trial order, and proofs return in send order over a FIFO wire.
    /// A swap-remove was measured paying ~5 Ed25519 verifies per proof where one suffices; the reference holds the same invariant free (an append-only list).
    fn remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct Receipts<C: ReceiptTable> {
    table: C,
    /// One-slot cache of the last decompressed signing key; decompressing per trial verify was a measured ~8% of a firehose initiator's CPU.
    verifier_memo: Option<Ed25519Verifier>,
    earliest_timeout: Option<InstantMillis>,
}

impl<C: ReceiptTable> Receipts<C> {
    /// A full table culls its stalest receipt, as RNS 1.4.2 `Transport.jobs()` does past `MAX_RECEIPTS`, always favoring the new send; the culled command still settles, typed.
    pub fn track(&mut self, receipt: OutstandingReceipt) -> Option<CulledReceipt> {
        let mut culled = None;
        if self.table.len() >= self.table.capacity() {
            culled = self.cull_stalest();
        }
        let pushed = self.table.push(receipt);
        self.refresh_earliest_timeout();
        match pushed {
            Ok(_) => culled,
            Err(TrackReceiptError::TableFull) => Some(CulledReceipt {
                command_id: receipt.command_id,
                kind: receipt.kind,
            }),
        }
    }

    fn cull_stalest(&mut self) -> Option<CulledReceipt> {
        let index = self
            .table
            .sent_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, sent_at)| **sent_at)
            .map(|(index, _)| index)?;
        let culled = CulledReceipt {
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
        };
        self.table.remove(index);
        Some(culled)
    }

    fn refresh_earliest_timeout(&mut self) {
        self.earliest_timeout = due_minimum(self.table.deadlines());
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_timeout,
            due_minimum(self.table.deadlines()),
            "earliest_timeout cache desynced from the deadlines column"
        );
        self.earliest_timeout
    }

    pub fn pop_expired(&mut self, now: InstantMillis) -> Option<ExpiredReceipt> {
        let index = self
            .table
            .deadlines()
            .iter()
            .position(|deadline| matches!(deadline, ReceiptDeadline::Due(at) if *at <= now))?;
        let expired = ExpiredReceipt {
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(expired)
    }

    /// RNS 1.4.2 explicit proof: match the row by full packet hash, then verify. A failed signature leaves the row outstanding (reference parity; the timeout still owns it).
    pub fn settle_by_explicit_proof(
        &mut self,
        proof_hash: &PacketHash,
        signature: &Ed25519Signature,
    ) -> Option<ProvenReceipt> {
        let index = (0..self.table.len()).find(|index| {
            self.table
                .kinds()
                .get(*index)
                .is_some_and(|kind| !kind.is_request())
                && self.table.packet_hashes().get(*index) == Some(proof_hash)
        })?;
        self.settle_verified(index, signature)
    }

    /// RNS 1.4.2 implicit proof: a bare signature, trial-verified against every outstanding row in insertion order (Packet.py); the ordering invariant makes that send order, so a FIFO wire's proofs match on the first trial.
    pub fn settle_by_implicit_proof(
        &mut self,
        signature: &Ed25519Signature,
    ) -> Option<ProvenReceipt> {
        let mut matched = None;
        for index in 0..self.table.len() {
            if self
                .table
                .kinds()
                .get(index)
                .is_some_and(|kind| !kind.is_request())
                && self.row_signature_valid(index, signature)
            {
                matched = Some(index);
                break;
            }
        }
        let index = matched?;
        let proven = ProvenReceipt {
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(proven)
    }

    /// Read, not removed: the row stays outstanding until the pool's verdict settles it through [`Self::settle_resolved`].
    pub fn resolve_explicit_for_deferred_verify(
        &mut self,
        proof_hash: &PacketHash,
    ) -> Option<DeferredVerify> {
        let index = (0..self.table.len()).find(|index| {
            self.table
                .kinds()
                .get(*index)
                .is_some_and(|kind| !kind.is_request())
                && self.table.packet_hashes().get(*index) == Some(proof_hash)
        })?;
        self.read_for_deferred_verify(index)
    }

    /// An implicit proof is addressed to its packet hash's [`PacketHash::proof_destination`], which names the exact receipt deterministically. The row is left outstanding until a valid verdict settles it: a forged signature can neither settle nor evict it, and the timeout still owns it.
    pub fn resolve_proof_by_destination(
        &mut self,
        proof_destination: &DestinationHash,
    ) -> Option<DeferredVerify> {
        let index = (0..self.table.len()).find(|index| {
            self.table
                .kinds()
                .get(*index)
                .is_some_and(|kind| !kind.is_request())
                && self
                    .table
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
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
        };
        let packet_hash = *self.table.packet_hashes().get(index)?;
        let signing_key = *self.table.signing_keys().get(index)?;
        Some(DeferredVerify {
            proven,
            packet_hash,
            signing_key,
        })
    }

    /// Keyed by command id and packet hash: a duplicate proof, a verdict that lost a race to the
    /// timeout or cull, or a command id later reused for another send settles nothing.
    /// A `SendRequest` row concludes by request id, never here.
    pub fn settle_resolved(
        &mut self,
        command_id: CommandId,
        packet_hash: &PacketHash,
    ) -> Option<ProvenReceipt> {
        let index = (0..self.table.len()).find(|index| {
            self.table.command_ids().get(*index) == Some(&command_id)
                && self.table.packet_hashes().get(*index) == Some(packet_hash)
                && self
                    .table
                    .kinds()
                    .get(*index)
                    .is_some_and(|kind| !kind.is_request())
        })?;
        let proven = ProvenReceipt {
            command_id,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(proven)
    }

    /// A response names its request by the truncated hash of the request packet; the session key authenticated it, so no signature gates this.
    pub fn settle_by_request_id(&mut self, request_id: RequestId) -> Option<ProvenReceipt> {
        let index = self.request_row_index(request_id)?;
        let proven = ProvenReceipt {
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(proven)
    }

    /// Non-removing peek for the resource accept gate: RNS 1.4.2 `Link.receive` accepts a response resource only when it names a request we actually sent.
    pub fn has_pending_request(&self, request_id: RequestId) -> bool {
        self.request_row_index(request_id).is_some()
    }

    /// Non-removing peek so a mid-chain response segment can name the command it answers.
    pub fn pending_request_command(&self, request_id: RequestId) -> Option<CommandId> {
        let index = self.request_row_index(request_id)?;
        self.table.command_ids().get(index).copied()
    }

    pub fn pending_request_response_limit(&self, request_id: RequestId) -> Option<ByteLimit> {
        let index = self.request_row_index(request_id)?;
        match self.table.kinds().get(index)? {
            ReceiptKind::SendRequest {
                maximum_response_bytes,
            } => Some(*maximum_response_bytes),
            ReceiptKind::SendSinglePacket { .. } | ReceiptKind::SendToLink(_) => None,
        }
    }

    /// The request's still-live response deadline before a Resource claims it.
    /// Pending Resource offers use this as a hard ceiling on their shorter
    /// admission wait.
    pub fn pending_request_deadline(&self, request_id: RequestId) -> Option<InstantMillis> {
        let index = self.request_row_index(request_id)?;
        match self.table.deadlines().get(index)? {
            ReceiptDeadline::Due(at) => Some(*at),
            ReceiptDeadline::ClaimedByTransfer => None,
        }
    }

    /// RNS 1.4.2 `RequestReceipt.response_resource_progress`: accepting a response resource flips the request to `RECEIVING` and its own timeout stops.
    /// The transfer settles the row through every exit, so a claimed row cannot leak.
    pub fn claim_request_for_transfer(&mut self, request_id: RequestId) {
        if let Some(index) = self.request_row_index(request_id) {
            self.table
                .set_deadline(index, ReceiptDeadline::ClaimedByTransfer);
            self.refresh_earliest_timeout();
        }
    }

    /// Hand the timeout back after a non-final segment concludes: the next segment's advertisement must land before `at` or the row expires.
    /// Our seam — the reference's `RECEIVING` requests wait forever on a chain that stalls between segments.
    pub fn arm_request_timeout(&mut self, request_id: RequestId, at: InstantMillis) {
        if let Some(index) = self.request_row_index(request_id) {
            self.table.set_deadline(index, ReceiptDeadline::Due(at));
            self.refresh_earliest_timeout();
        }
    }

    fn request_row_index(&self, request_id: RequestId) -> Option<usize> {
        (0..self.table.len()).find(|index| {
            self.table
                .kinds()
                .get(*index)
                .is_some_and(|kind| kind.is_request())
                && self
                    .table
                    .packet_hashes()
                    .get(*index)
                    .is_some_and(|hash| &hash.as_bytes()[..16] == request_id.as_bytes())
        })
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
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
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(proven)
    }

    fn row_signature_valid(&mut self, index: usize, signature: &Ed25519Signature) -> bool {
        let (Some(packet_hash), Some(signing_key)) = (
            self.table.packet_hashes().get(index).copied(),
            self.table.signing_keys().get(index).copied(),
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

fn due_minimum(deadlines: &[ReceiptDeadline]) -> Option<InstantMillis> {
    deadlines
        .iter()
        .filter_map(|deadline| match deadline {
            ReceiptDeadline::Due(at) => Some(*at),
            ReceiptDeadline::ClaimedByTransfer => None,
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    type TestReceipts = Receipts<FixedReceiptTable<3>>;

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
            kind: ReceiptKind::SendSinglePacket {
                route_evidence: None,
            },
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
                kind: ReceiptKind::SendSinglePacket {
                    route_evidence: None,
                },
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
                .settle_resolved(resolved.proven.command_id, &resolved.packet_hash)
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
            .settle_resolved(CommandId(4), &PacketHash::new([0x66; 32]))
            .expect("a valid verdict settles the resolved receipt");
        assert_eq!(proven.command_id, CommandId(4));
        assert_eq!(receipts.len(), 1, "only the settled receipt is removed");

        assert!(
            receipts
                .settle_resolved(CommandId(4), &PacketHash::new([0x66; 32]))
                .is_none(),
            "a second verdict for the same command settles nothing, exactly once",
        );
        assert_eq!(
            receipts.len(),
            1,
            "the duplicate verdict removes no other receipt",
        );
    }

    #[test]
    fn a_stale_verdict_cannot_settle_a_reused_command_id() {
        let (_, key) = signer(0x66);
        let stale_hash = PacketHash::new([0x66; 32]);
        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x66, 4, key, 100, 7_000));

        receipts
            .resolve_proof_by_destination(&stale_hash.proof_destination())
            .expect("the old worker resolves the original receipt");
        assert_eq!(
            receipts
                .pop_expired(InstantMillis(7_000))
                .map(|receipt| receipt.command_id),
            Some(CommandId(4)),
            "the original receipt can time out while verification is in flight",
        );

        let replacement_hash = PacketHash::new([0x77; 32]);
        receipts.track(outstanding(0x77, 4, key, 8_000, 15_000));
        assert!(
            receipts
                .settle_resolved(CommandId(4), &stale_hash)
                .is_none(),
            "the old verdict must match both command id and packet hash",
        );
        assert_eq!(receipts.len(), 1, "the reused command remains outstanding");
        assert!(
            receipts
                .settle_resolved(CommandId(4), &replacement_hash)
                .is_some(),
            "the replacement's own proof can still settle it",
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
    fn a_claimed_request_neither_expires_nor_drives_the_wakeup_until_rearmed() {
        let (_, key) = signer(0x21);
        let mut receipts = TestReceipts::default();
        let packet_hash = PacketHash::new([0x2A; 32]);
        let request_id = RequestId::of_packet(&packet_hash);
        receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: CommandId(4),
            kind: ReceiptKind::SendRequest {
                maximum_response_bytes: ByteLimit::Unlimited,
            },
            peer_signing_key: key,
            sent_at: InstantMillis(100),
            timeout_at: InstantMillis(7_000),
        });

        receipts.claim_request_for_transfer(request_id);
        assert_eq!(receipts.earliest_timeout_at(), None);
        assert_eq!(receipts.pop_expired(InstantMillis(u64::MAX)), None);
        assert!(receipts.has_pending_request(request_id));
        assert_eq!(
            receipts.pending_request_command(request_id),
            Some(CommandId(4)),
        );

        receipts.arm_request_timeout(request_id, InstantMillis(9_000));
        assert_eq!(receipts.earliest_timeout_at(), Some(InstantMillis(9_000)));
        assert_eq!(receipts.pop_expired(InstantMillis(8_999)), None);
        assert_eq!(
            receipts
                .pop_expired(InstantMillis(9_000))
                .map(|receipt| receipt.command_id),
            Some(CommandId(4)),
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
                kind: ReceiptKind::SendSinglePacket {
                    route_evidence: None,
                },
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
                kind: ReceiptKind::SendSinglePacket {
                    route_evidence: None,
                },
                sent_at: InstantMillis(300),
            }),
        );
        assert_eq!(receipts.len(), 1);

        let stale = ed25519_sign(&first_secret, PacketHash::new([9; 32]).as_bytes());
        assert_eq!(receipts.settle_by_implicit_proof(&stale), None);
    }
}
