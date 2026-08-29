use super::announce::{AnnounceIngest, AnnounceVerifyOwed};
use super::forward::PacketToForward;
use super::links::ForwardedLinkRequestBody;
use super::upstream_delivery::{DecryptOwed, RatchetDecryptOwed};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::engine::{
    CommandId, InstantMillis, LinkClosedReason, PacketReceiptDelivered, WakeSchedule,
};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::announce::schedule::ScheduleRejection;
use crate::routing::announce::{AnnounceObservation, AnnounceRateAccounting};
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::{ChannelSequence, MessageType};
use crate::routing::links::handshake::{
    AcceptedLinkRequest, LinkProofSignOwed, LinkProofVerifyOwed, LinkRttError,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::table::AcceptedResource;
use crate::routing::links::resources::{
    ResourceCorrelation, ResourceFailureCause, ResourceHash, ResourcePartRequest,
};
use crate::routing::links::LinkId;
use crate::routing::path_requests::seen::PathRequestIdBytes;
use crate::routing::proof::{ProofIngest, ProofObligation};
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, WirePacketHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IngestEffects<'a> {
    pub destination_identity_expiry: Option<InstantMillis>,
    pub accepted_announce: Option<AcceptedAnnounceEffect<'a>>,
    pub held_announce_release: WakeSchedule,
}

impl Default for IngestEffects<'_> {
    fn default() -> Self {
        Self {
            destination_identity_expiry: None,
            accepted_announce: None,
            held_announce_release: WakeSchedule::Unchanged,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedAnnounceEffect<'a> {
    pub observation: AnnounceObservation<'a>,
    pub rate_accounting: AnnounceRateAccounting,
}

impl IngestEffects<'_> {
    pub(crate) fn note_destination_identity_expiry(&mut self, expiry: Option<InstantMillis>) {
        if let Some(expiry) = expiry {
            self.destination_identity_expiry = Some(
                self.destination_identity_expiry
                    .map_or(expiry, |current| current.min(expiry)),
            );
        }
    }
}

/// RNS 1.4.2 `Transport.packet_filter` drops PLAIN and GROUP data received more than one hop out.
pub const NON_TRANSPORTED_DATA_MAX_RECEIVED_HOPS: u8 = 1;

#[derive(Default)]
#[allow(clippy::large_enum_variant)]
pub enum DeferredCrypto {
    #[default]
    Empty,
    Decrypt(DecryptOwed),
    RatchetDecrypt(RatchetDecryptOwed),
    LinkProofVerify(LinkProofVerifyOwed),
    LinkProofSign(LinkProofSignOwed),
    AnnounceVerify(AnnounceVerifyOwed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRttOwed {
    pub link_id: LinkId,
    pub received_hops: u8,
    pub responder_encryption: X25519PublicKey,
    pub responder_signing: Ed25519PublicKey,
    pub command_id: CommandId,
    pub arrived_at: InstantMillis,
    pub rtt: RttMillis,
    pub mtu: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    Consumed,
    Malformed,
    UnhandledContext,
    Duplicate,
    Superseded,
    NotForUs,
    NoRoute,
    HopLimitReached,
    LoopPrevented,
    RouteUnresponsive,
    OtherInstance,
    UnknownLink,
    LinkPhaseMismatch,
    LinkRttError(LinkRttError),
    DecryptFailed,
    ProofInvalid,
    UnknownIdentity,
    /// Link requests are disabled for the destination.
    LinkRequestsRefused,
    PermissionDenied,
    RateLimited,
    CapacityExhausted,
    RequestTooLarge,
    /// The app's declared acceptance policy declined an offer that was well-formed and deliverable.
    StrategyDeclined,
    /// A resource response advertisement has no matching pending request.
    UnmatchedResponse,
    IfacRefused,
}

/// A receive outcome that violates an interface-framing or RNS protocol rule. These are narrower
/// than all ignored packets: duplicates, packets for another node, policy refusals, IFAC
/// authentication failures, and resource pressure are intentionally not protocol violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolViolationKind {
    Malformed,
    InvalidIfacEnvelope,
    UnhandledContext,
    RelayedNonTransportedData,
    InvalidLinkPhase,
    InvalidLinkRtt,
    InvalidProof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestPacketOutcome<'p> {
    Announce(AnnounceIngest),
    Delivery {
        delivery: Delivery<'p>,
        proof: ProofObligation,
    },
    OwesDecrypt,
    OwesRatchetDecrypt,
    OwesAnnounceVerify,
    Proof(ProofIngest),
    Forward(PacketToForward<'p>),
    AnswerPathRequest {
        destination: DestinationHash,
    },
    ScheduledPathResponse {
        destination: DestinationHash,
    },
    PathResponseScheduleRejected {
        destination: DestinationHash,
        rejection: ScheduleRejection,
    },
    /// RNS `DISCOVER_PATHS_FOR`.
    ForwardRecursivePathRequest {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    ForwardBoundaryPathRequest {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    ForwardLocalClientPathRequest {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    RelayPathRequestToLocalClients {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    RequestReceived {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'p [u8],
    },
    ResponseSettled {
        id: CommandId,
        delivered: PacketReceiptDelivered,
        link_id: LinkId,
        request_id: RequestId,
        data: &'p [u8],
    },
    ResponseTooLarge {
        id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
    },
    ChannelDataReceived {
        link_id: LinkId,
        message_type: MessageType,
        sequence: ChannelSequence,
        payload: &'p [u8],
        packet_hash: PacketHash,
    },
    OwesResourceParts(ResourcePartRequest<'p>),

    ResourceDelivered {
        id: CommandId,
        link_id: LinkId,
        correlation: ResourceCorrelation,
        last_segment: bool,
    },
    OwesResourcePull {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// RNS 1.4.2 `ACCEPT_APP` callback point.
    ResourceOffered {
        link_id: LinkId,
        original_hash: ResourceHash,
        accepted: AcceptedResource<'p>,
    },
    ResourceTooLarge {
        link_id: LinkId,
        hash: ResourceHash,
        settled_request: Option<CommandId>,
    },
    /// A validated and policy-approved advertisement is waiting for an
    /// incoming Resource row, or a retry coalesced into that existing wait.
    ResourceAdmissionPending,
    /// The offer cannot wait: it can never fit, this target has no pending
    /// queue, or the bounded queue is full.
    ResourceCapacityRejected {
        link_id: LinkId,
        hash: ResourceHash,
        settled_request: Option<CommandId>,
    },
    OwesResourceAssembly {
        link_id: LinkId,
        hash: ResourceHash,
    },
    ResourceDeadlineAdvanced,
    IncomingResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
        settled_request: Option<CommandId>,
    },
    ResourceRejectedByPeer {
        id: CommandId,
        link_id: LinkId,
        correlation: ResourceCorrelation,
    },
    TransportedLinkRequest {
        header: WirePacketHeader,
        body: ForwardedLinkRequestBody,
        fire_on: InterfaceId,
    },
    OwesLinkProof(AcceptedLinkRequest),
    OwesLinkRtt(LinkRttOwed),
    OwesLinkProofVerify,
    LinkActivated {
        link_id: LinkId,
        rtt_millis: u64,
    },
    OwesKeepaliveEcho {
        link_id: LinkId,
    },
    LinkClosedByPeer {
        link_id: LinkId,
    },
    OwesLinkClose {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    TunnelObserved {
        expires: InstantMillis,
    },
    Ignored(IgnoreReason),
}

impl ProtocolViolationKind {
    #[must_use]
    pub const fn of_outcome(outcome: &IngestPacketOutcome<'_>) -> Option<Self> {
        match outcome {
            IngestPacketOutcome::Ignored(reason) => match reason {
                IgnoreReason::Malformed => Some(Self::Malformed),
                IgnoreReason::UnhandledContext => Some(Self::UnhandledContext),
                IgnoreReason::HopLimitReached => Some(Self::RelayedNonTransportedData),
                IgnoreReason::LinkPhaseMismatch => Some(Self::InvalidLinkPhase),
                IgnoreReason::LinkRttError(_) => Some(Self::InvalidLinkRtt),
                IgnoreReason::ProofInvalid => Some(Self::InvalidProof),
                IgnoreReason::Consumed
                | IgnoreReason::Duplicate
                | IgnoreReason::Superseded
                | IgnoreReason::NotForUs
                | IgnoreReason::NoRoute
                | IgnoreReason::LoopPrevented
                | IgnoreReason::RouteUnresponsive
                | IgnoreReason::OtherInstance
                | IgnoreReason::UnknownLink
                | IgnoreReason::DecryptFailed
                | IgnoreReason::UnknownIdentity
                | IgnoreReason::LinkRequestsRefused
                | IgnoreReason::PermissionDenied
                | IgnoreReason::RateLimited
                | IgnoreReason::CapacityExhausted
                | IgnoreReason::RequestTooLarge
                | IgnoreReason::StrategyDeclined
                | IgnoreReason::UnmatchedResponse
                | IgnoreReason::IfacRefused => None,
            },
            IngestPacketOutcome::OwesLinkClose {
                reason: LinkClosedReason::MalformedRtt,
                ..
            } => Some(Self::InvalidLinkRtt),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_malformed(self) -> bool {
        matches!(self, Self::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::LinkId;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    #[test]
    fn protocol_violations_are_the_wire_rule_subset_of_ignored_packets() {
        for (reason, expected) in [
            (IgnoreReason::Malformed, ProtocolViolationKind::Malformed),
            (
                IgnoreReason::UnhandledContext,
                ProtocolViolationKind::UnhandledContext,
            ),
            (
                IgnoreReason::HopLimitReached,
                ProtocolViolationKind::RelayedNonTransportedData,
            ),
            (
                IgnoreReason::LinkPhaseMismatch,
                ProtocolViolationKind::InvalidLinkPhase,
            ),
            (
                IgnoreReason::LinkRttError(LinkRttError::InvalidToken),
                ProtocolViolationKind::InvalidLinkRtt,
            ),
            (
                IgnoreReason::ProofInvalid,
                ProtocolViolationKind::InvalidProof,
            ),
        ] {
            assert_eq!(
                ProtocolViolationKind::of_outcome(&IngestPacketOutcome::Ignored(reason)),
                Some(expected),
            );
        }

        for reason in [
            IgnoreReason::Duplicate,
            IgnoreReason::NotForUs,
            IgnoreReason::DecryptFailed,
            IgnoreReason::CapacityExhausted,
            IgnoreReason::IfacRefused,
        ] {
            assert_eq!(
                ProtocolViolationKind::of_outcome(&IngestPacketOutcome::Ignored(reason)),
                None,
            );
        }
    }

    #[test]
    fn malformed_link_rtt_close_is_a_protocol_violation() {
        let malformed = IngestPacketOutcome::OwesLinkClose {
            link_id: LinkId::new([0; TRUNCATED_HASH_BYTE_LEN]),
            reason: LinkClosedReason::MalformedRtt,
        };
        assert_eq!(
            ProtocolViolationKind::of_outcome(&malformed),
            Some(ProtocolViolationKind::InvalidLinkRtt),
        );

        let timeout = IngestPacketOutcome::OwesLinkClose {
            link_id: LinkId::new([0; TRUNCATED_HASH_BYTE_LEN]),
            reason: LinkClosedReason::Timeout,
        };
        assert_eq!(ProtocolViolationKind::of_outcome(&timeout), None);
    }
}
