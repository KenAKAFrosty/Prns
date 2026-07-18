use super::announce::{AnnounceIngest, AnnounceVerifyOwed};
use super::forward::PacketToForward;
use super::links::ForwardedLinkRequestBody;
use super::upstream_delivery::{DecryptOwed, RatchetDecryptOwed};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::engine::{CommandId, InstantMillis, LinkClosedReason, PacketReceiptDelivered};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::announce::{AnnounceObservation, AnnounceRateAccounting};
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::{ChannelSequence, MessageType};
use crate::routing::links::handshake::{
    AcceptedLinkRequest, LinkProofSignOwed, LinkProofVerifyOwed, LinkRttError,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::table::AcceptedResource;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash, ResourcePartRequest};
use crate::routing::links::LinkId;
use crate::routing::path_requests::seen::PathRequestIdBytes;
use crate::routing::proof::{ProofIngest, ProofObligation};
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, WirePacketHeader};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IngestEffects<'a> {
    pub destination_identity_expiry: Option<InstantMillis>,
    pub accepted_announce: Option<AcceptedAnnounceEffect<'a>>,
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

/// RNS 1.3.5 `Transport.packet_filter` drops PLAIN and GROUP data received more than one hop out.
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
    pub responder_encryption: X25519PublicKey,
    pub responder_signing: Ed25519PublicKey,
    pub command_id: CommandId,
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
    /// RNS 1.3.5 `Destination.accept_link_requests` is off: the destination announces but answers no `LINKREQUEST`.
    LinkRequestsRefused,
    PermissionDenied,
    RateLimited,
    CapacityExhausted,
    /// The app's declared acceptance policy declined an offer that was well-formed and deliverable.
    StrategyDeclined,
    /// RNS 1.3.5 drops response advertisements with no matching pending request.
    UnmatchedResponse,
    IfacRefused,
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
    /// RNS `DISCOVER_PATHS_FOR`.
    ForwardRecursivePathRequest {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// RNS `Transport.path_request`.
    RelayPathRequestToLocalClients {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// RNS `remote_identified`.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    RequestReceived {
        link_id: LinkId,
        request_id: RequestId,
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
    },
    OwesResourcePull {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// RNS 1.3.5 `ACCEPT_APP` callback point.
    ResourceOffered {
        link_id: LinkId,
        original_hash: ResourceHash,
        accepted: AcceptedResource<'p>,
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
        /// The pending request already removed from receipts.
        settled_request: Option<CommandId>,
    },
    ResourceRejectedByPeer {
        id: CommandId,
        link_id: LinkId,
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
        rtt_ms: u64,
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
