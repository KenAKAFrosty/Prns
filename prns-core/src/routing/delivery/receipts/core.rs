use crate::engine::{CommandId, InstantMillis, SendRequestIntent};
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
        link_id: LinkId,
        response: RequestReceiptPolicy,
    },
}

const _: () = {
    assert!(core::mem::size_of::<ReceiptKind>() == 32);
};

impl ReceiptKind {
    const fn is_request(self) -> bool {
        matches!(self, Self::SendRequest { .. })
    }

    pub(crate) const fn request(
        link_id: LinkId,
        maximum_response_bytes: ByteLimit,
        intent: SendRequestIntent,
    ) -> Self {
        Self::SendRequest {
            link_id,
            response: RequestReceiptPolicy::new(maximum_response_bytes, intent),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestReceiptPolicy {
    ApplicationUnlimited,
    ApplicationMaximum(u64),
    RemoteControlControllerPairingUnlimited,
    RemoteControlControllerPairingMaximum(u64),
}

impl RequestReceiptPolicy {
    pub(crate) const fn new(limit: ByteLimit, intent: SendRequestIntent) -> Self {
        match (intent, limit) {
            (SendRequestIntent::Application, ByteLimit::Unlimited) => Self::ApplicationUnlimited,
            (SendRequestIntent::Application, ByteLimit::Maximum(maximum)) => {
                Self::ApplicationMaximum(maximum)
            }
            (SendRequestIntent::RemoteControlControllerPairing, ByteLimit::Unlimited) => {
                Self::RemoteControlControllerPairingUnlimited
            }
            (SendRequestIntent::RemoteControlControllerPairing, ByteLimit::Maximum(maximum)) => {
                Self::RemoteControlControllerPairingMaximum(maximum)
            }
        }
    }

    pub const fn intent(self) -> SendRequestIntent {
        match self {
            Self::ApplicationUnlimited | Self::ApplicationMaximum(_) => {
                SendRequestIntent::Application
            }
            Self::RemoteControlControllerPairingUnlimited
            | Self::RemoteControlControllerPairingMaximum(_) => {
                SendRequestIntent::RemoteControlControllerPairing
            }
        }
    }

    pub const fn maximum_response_bytes(self) -> ByteLimit {
        match self {
            Self::ApplicationUnlimited | Self::RemoteControlControllerPairingUnlimited => {
                ByteLimit::Unlimited
            }
            Self::ApplicationMaximum(maximum)
            | Self::RemoteControlControllerPairingMaximum(maximum) => ByteLimit::Maximum(maximum),
        }
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
pub struct ProvenRequestReceipt {
    pub command_id: CommandId,
    pub intent: SendRequestIntent,
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
pub(crate) struct ReceiptProofCandidate {
    pub(crate) command_id: CommandId,
    pub(crate) kind: ReceiptKind,
    pub(crate) sent_at: InstantMillis,
    pub(crate) packet_hash: PacketHash,
    pub(crate) signing_key: IdentitySigningPublicKey,
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
pub enum LinkOwnedReceiptKind {
    SendToLink,
    SendRequest(SendRequestIntent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkOwnedReceipt {
    pub command_id: CommandId,
    pub kind: LinkOwnedReceiptKind,
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
    /// Removal preserves insertion order (shift, not swap), matching the reference's append-only
    /// receipt list and keeping deterministic resolution if truncated proof destinations collide.
    fn remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct Receipts<C: ReceiptTable> {
    table: C,
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

    pub fn pop_for_link(&mut self, link_id: &LinkId) -> Option<LinkOwnedReceipt> {
        let (index, kind) = self
            .table
            .kinds()
            .iter()
            .enumerate()
            .find_map(|(index, kind)| match kind {
                ReceiptKind::SendToLink(candidate) if candidate == link_id => {
                    Some((index, LinkOwnedReceiptKind::SendToLink))
                }
                ReceiptKind::SendRequest {
                    link_id: candidate,
                    response,
                } if candidate == link_id => {
                    Some((index, LinkOwnedReceiptKind::SendRequest(response.intent())))
                }
                ReceiptKind::SendSinglePacket { .. }
                | ReceiptKind::SendToLink(_)
                | ReceiptKind::SendRequest { .. } => None,
            })?;
        let receipt = LinkOwnedReceipt {
            command_id: *self.table.command_ids().get(index)?,
            kind,
        };
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(receipt)
    }

    /// Read, not removed: the row stays outstanding until the engine resumes a valid external
    /// verdict and takes the exact matching row through [`Self::take_matching_proof_receipt`].
    pub(crate) fn resolve_explicit_for_verification(
        &self,
        proof_hash: &PacketHash,
    ) -> Option<ReceiptProofCandidate> {
        let index = (0..self.table.len()).find(|index| {
            self.table
                .kinds()
                .get(*index)
                .is_some_and(|kind| !kind.is_request())
                && self.table.packet_hashes().get(*index) == Some(proof_hash)
        })?;
        self.read_proof_candidate(index)
    }

    /// An implicit proof is addressed to its packet hash's [`PacketHash::proof_destination`], which names the exact receipt deterministically. The row stays outstanding until a valid verdict is resumed: a forged signature can neither remove nor evict it, and the timeout still owns it.
    pub(crate) fn resolve_proof_by_destination(
        &self,
        proof_destination: &DestinationHash,
    ) -> Option<ReceiptProofCandidate> {
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
        self.read_proof_candidate(index)
    }

    fn read_proof_candidate(&self, index: usize) -> Option<ReceiptProofCandidate> {
        let candidate = ReceiptProofCandidate {
            command_id: *self.table.command_ids().get(index)?,
            kind: *self.table.kinds().get(index)?,
            sent_at: *self.table.sent_ats().get(index)?,
            packet_hash: *self.table.packet_hashes().get(index)?,
            signing_key: *self.table.signing_keys().get(index)?,
        };
        Some(candidate)
    }

    /// Keyed by command id and packet hash: a duplicate proof, a verdict that lost a race to the
    /// timeout or cull, or a command id later reused for another send takes no row.
    /// A `SendRequest` row concludes by request id, never here.
    pub(crate) fn take_matching_proof_receipt(
        &mut self,
        command_id: CommandId,
        packet_hash: &PacketHash,
    ) -> Option<ReceiptKind> {
        let index = (0..self.table.len()).find(|index| {
            self.table.command_ids().get(*index) == Some(&command_id)
                && self.table.packet_hashes().get(*index) == Some(packet_hash)
                && self
                    .table
                    .kinds()
                    .get(*index)
                    .is_some_and(|kind| !kind.is_request())
        })?;
        let kind = *self.table.kinds().get(index)?;
        self.table.remove(index);
        self.refresh_earliest_timeout();
        Some(kind)
    }

    /// A response names its request by the truncated hash of the request packet; the session key authenticated it, so no signature gates this.
    pub fn settle_by_request_id(&mut self, request_id: RequestId) -> Option<ProvenRequestReceipt> {
        let index = self.request_row_index(request_id)?;
        let intent = match *self.table.kinds().get(index)? {
            ReceiptKind::SendRequest { response, .. } => response.intent(),
            ReceiptKind::SendSinglePacket { .. } | ReceiptKind::SendToLink(_) => return None,
        };
        let proven = ProvenRequestReceipt {
            command_id: *self.table.command_ids().get(index)?,
            intent,
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
            ReceiptKind::SendRequest { response, .. } => Some(response.maximum_response_bytes()),
            ReceiptKind::SendSinglePacket { .. } | ReceiptKind::SendToLink(_) => None,
        }
    }

    pub fn pending_request_intent(&self, request_id: RequestId) -> Option<SendRequestIntent> {
        let index = self.request_row_index(request_id)?;
        match self.table.kinds().get(index)? {
            ReceiptKind::SendRequest { response, .. } => Some(response.intent()),
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
    use crate::crypto::{ed25519_public_key, ed25519_sign, ed25519_verify, Ed25519SecretKey};

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
    fn proof_candidate_carries_exact_verification_material_and_settles_once() {
        let (secret, key) = signer(0x33);
        let packet_hash = PacketHash::new([0x44; 32]);
        let signature = ed25519_sign(&secret, packet_hash.as_bytes());
        let expected_kind = ReceiptKind::SendSinglePacket {
            route_evidence: None,
        };

        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x44, 7, key, 100, 7_000));
        let candidate = receipts
            .resolve_proof_by_destination(&packet_hash.proof_destination())
            .expect("the proof destination resolves its candidate");

        assert_eq!(candidate.command_id, CommandId(7));
        assert_eq!(candidate.kind, expected_kind);
        assert_eq!(candidate.sent_at, InstantMillis(100));
        assert_eq!(candidate.packet_hash, packet_hash);
        assert!(
            ed25519_verify(
                candidate.signing_key.as_ed25519(),
                candidate.packet_hash.as_bytes(),
                &signature,
            )
            .is_ok(),
            "the candidate carries exactly the material an external fulfiller verifies",
        );
        assert_eq!(
            receipts.len(),
            1,
            "resolution identifies the receipt but leaves it outstanding until a valid verdict settles it",
        );
        assert_eq!(
            receipts
                .take_matching_proof_receipt(candidate.command_id, &candidate.packet_hash)
                .as_ref(),
            Some(&expected_kind),
            "a valid verdict settles exactly the resolved receipt",
        );
        assert!(
            receipts.is_empty(),
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
        assert_eq!(resolved.command_id, CommandId(9));
        assert_eq!(
            receipts.len(),
            1,
            "candidate resolution alone leaves the row in place",
        );

        assert_eq!(
            receipts
                .pop_expired(InstantMillis(8_000))
                .map(|r| r.command_id),
            Some(CommandId(9)),
            "a forged proof whose verify fails never takes the matching row, so its receipt still expires on schedule",
        );
    }

    #[test]
    fn taking_a_matching_proof_receipt_removes_it_exactly_once() {
        let (_, key) = signer(0x66);
        let destination = PacketHash::new([0x66; 32]).proof_destination();
        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x66, 4, key, 100, 7_000));
        receipts.track(outstanding(0x77, 5, key, 200, 7_000));

        receipts
            .resolve_proof_by_destination(&destination)
            .expect("the destination identifies its receipt");

        let kind = receipts
            .take_matching_proof_receipt(CommandId(4), &PacketHash::new([0x66; 32]))
            .expect("a valid verdict settles the resolved receipt");
        assert_eq!(
            kind,
            ReceiptKind::SendSinglePacket {
                route_evidence: None,
            }
        );
        assert_eq!(receipts.len(), 1, "only the settled receipt is removed");

        assert!(
            receipts
                .take_matching_proof_receipt(CommandId(4), &PacketHash::new([0x66; 32]))
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
                .take_matching_proof_receipt(CommandId(4), &stale_hash)
                .is_none(),
            "the old verdict must match both command id and packet hash",
        );
        assert_eq!(receipts.len(), 1, "the reused command remains outstanding");
        assert!(
            receipts
                .take_matching_proof_receipt(CommandId(4), &replacement_hash)
                .is_some(),
            "the replacement's own proof can still settle it",
        );
    }

    #[test]
    fn proof_destination_resolves_its_receipt_not_the_oldest() {
        let (_, key_a) = signer(0x11);
        let (_, key_b) = signer(0x22);
        let b_destination = PacketHash::new([0x22; 32]).proof_destination();

        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x11, 1, key_a, 100, 7_000));
        receipts.track(outstanding(0x22, 2, key_b, 200, 7_000));
        let candidate = receipts
            .resolve_proof_by_destination(&b_destination)
            .expect("the proof's destination identifies its receipt");
        assert_eq!(
            candidate.command_id,
            CommandId(2),
            "resolution selects the addressed receipt without trial-verifying older rows",
        );
    }

    #[test]
    fn proof_destination_rejects_a_proof_that_matches_no_outstanding_receipt() {
        let (_, key_a) = signer(0x11);
        let (_, key_b) = signer(0x22);
        let stray_destination = PacketHash::new([0x99; 32]).proof_destination();

        let mut receipts = TestReceipts::default();
        receipts.track(outstanding(0x11, 1, key_a, 100, 7_000));
        receipts.track(outstanding(0x22, 2, key_b, 200, 7_000));

        assert!(
            receipts
                .resolve_proof_by_destination(&stray_destination)
                .is_none(),
            "a proof addressed to no tracked send must not settle anything",
        );
        assert_eq!(receipts.len(), 2, "a non-matching proof removes no receipt");
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
                link_id: LinkId::new([0x2A; 16]),
                response: RequestReceiptPolicy::ApplicationUnlimited,
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
    fn link_retirement_reclaims_even_a_request_claimed_by_a_resource() {
        let (_, key) = signer(0x41);
        let link_id = LinkId::new([0x42; 16]);
        let packet_hash = PacketHash::new([0x43; 32]);
        let request_id = RequestId::of_packet(&packet_hash);
        let mut receipts = TestReceipts::default();
        receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: CommandId(43),
            kind: ReceiptKind::SendRequest {
                link_id,
                response: RequestReceiptPolicy::ApplicationUnlimited,
            },
            peer_signing_key: key,
            sent_at: InstantMillis(100),
            timeout_at: InstantMillis(7_000),
        });
        receipts.track(OutstandingReceipt {
            packet_hash: PacketHash::new([0x44; 32]),
            command_id: CommandId(44),
            kind: ReceiptKind::SendToLink(link_id),
            peer_signing_key: key,
            sent_at: InstantMillis(200),
            timeout_at: InstantMillis(8_000),
        });
        receipts.track(outstanding(0x45, 45, key, 300, 9_000));
        receipts.claim_request_for_transfer(request_id);

        assert_eq!(
            receipts.pop_for_link(&link_id),
            Some(LinkOwnedReceipt {
                command_id: CommandId(43),
                kind: LinkOwnedReceiptKind::SendRequest(SendRequestIntent::Application),
            }),
        );
        assert_eq!(
            receipts.pop_for_link(&link_id),
            Some(LinkOwnedReceipt {
                command_id: CommandId(44),
                kind: LinkOwnedReceiptKind::SendToLink,
            }),
        );
        assert_eq!(receipts.pop_for_link(&link_id), None);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts.earliest_timeout_at(), Some(InstantMillis(9_000)));
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
        let candidate = receipts
            .resolve_explicit_for_verification(&named)
            .expect("the named packet hash resolves its candidate");
        assert!(ed25519_verify(
            candidate.signing_key.as_ed25519(),
            candidate.packet_hash.as_bytes(),
            &signature,
        )
        .is_ok());
        assert_eq!(
            receipts.take_matching_proof_receipt(candidate.command_id, &candidate.packet_hash),
            Some(ReceiptKind::SendSinglePacket {
                route_evidence: None,
            }),
        );
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts.resolve_explicit_for_verification(&named),
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
        let candidate = receipts
            .resolve_explicit_for_verification(&named)
            .expect("the packet hash still resolves its outstanding receipt");
        assert!(ed25519_verify(
            candidate.signing_key.as_ed25519(),
            candidate.packet_hash.as_bytes(),
            &forged,
        )
        .is_err());
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts
                .pop_expired(InstantMillis(9_000))
                .map(|receipt| receipt.command_id),
            Some(CommandId(1)),
            "an invalid external verdict leaves timeout ownership intact",
        );
    }
}
