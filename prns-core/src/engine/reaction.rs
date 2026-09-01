#[cfg(feature = "runtime-metrics")]
use super::metrics::AnnounceOrigin;
use crate::engine::InstantMillis;
use crate::engine::{CommandId, LinkEstablished, Settlement};
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::announce::held::HeldDropCause;
use crate::routing::announce::{AnnounceObservation, AnnounceRateAccounting};
use crate::routing::delivery::send_single::EncryptOwed;
use crate::routing::delivery::Delivery;
use crate::routing::ingress::{AnnounceVerifyOwed, DecryptOwed, RatchetDecryptOwed};
use crate::routing::links::channel::MessageType;
use crate::routing::links::handshake::{LinkProofSignOwed, LinkProofVerifyOwed};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::send::ResourceBuildOwed;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash};
use crate::routing::links::LinkId;
use crate::routing::proof::{DeferredLinkReceiptSign, DeferredProofSign};
use crate::routing::request_handlers::RequestPathHash;
use crate::routing::RouteRemovalCause;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

// repr(C) on this enum, Journaled, and Directive: they cross the dual-core channel; see the layout note on [`PrnsCommand`].
#[repr(C)]
// Reactions cross no-alloc runtimes by value. Boxing the journal would add an allocation to every
// engine step and is unavailable on the embedded core this representation serves.
#[allow(clippy::large_enum_variant)]
pub enum EngineReaction<'a, Work = NoOwedWork> {
    /// A notice that something has just happened within the engine.
    Journaled(Journaled<'a>),

    /// An order for something that must now happen outside the engine.
    Directive(Directive<'a, Work>),
}

/// The work channel of an engine entry point that cannot request external fulfillment.
/// Its uninhabited shape lets runtimes route those reactions without an impossible fallback.
pub enum NoOwedWork {}

impl<'a, Work> EngineReaction<'a, Work> {
    /// Changes only the externally fulfilled-work channel while preserving the reaction itself.
    ///
    /// This is especially useful when a narrower engine entry point returns
    /// [`NoOwedWork`]: the exhaustive `match never {}` widens that reaction for a manifold that
    /// routes the complete [`OwedWork`] contract without inventing an impossible fallback.
    pub fn map_work<Mapped>(self, map: impl FnOnce(Work) -> Mapped) -> EngineReaction<'a, Mapped> {
        match self {
            Self::Journaled(journaled) => EngineReaction::Journaled(journaled),
            Self::Directive(directive) => EngineReaction::Directive(directive.map_work(map)),
        }
    }
}

/// Work the engine has fully authorized but asks its surrounding manifold to fulfill.
///
/// The enum names protocol work, never a scheduling decision. A runtime may fulfill a variant
/// inline, move it into a worker job, or use a platform accelerator without changing the engine
/// transition that requested it.
#[repr(C)]
// Owed work owns continuation material so runtimes can move it without copying packet payloads.
// Indirection belongs in a runtime's job envelope, never in the no-alloc engine contract.
#[allow(clippy::large_enum_variant)]
pub enum OwedWork<'a> {
    Crypto(CryptoOwed),
    ResourceBuild(ResourceBuildOwed<'a>),
    ResourceDecompression(ResourceDecompressionOwed<'a>),
}

/// Cryptographic work whose policy inputs are complete and whose pure operation may run outside
/// the engine. Every variant owns its continuation material so a runtime can move it directly
/// into a worker envelope without another packet-buffer copy.
#[repr(C)]
#[allow(clippy::large_enum_variant)]
pub enum CryptoOwed {
    Encrypt(EncryptOwed),
    Decrypt(DecryptOwed),
    RatchetDecrypt(RatchetDecryptOwed),
    LinkProofVerify(LinkProofVerifyOwed),
    LinkProofSign(LinkProofSignOwed),
    ProofSign(DeferredProofSign),
    LinkReceiptSign(DeferredLinkReceiptSign),
    AnnounceVerify(AnnounceVerifyOwed),
    RemoteControlPairingAvailabilityVerify(
        crate::remote_control::RemoteControlPairingAvailabilityVerifyOwed,
    ),
}

impl<'a> From<CryptoOwed> for OwedWork<'a> {
    fn from(owed: CryptoOwed) -> Self {
        Self::Crypto(owed)
    }
}

/// A compressed resource stream the engine has authenticated and asks its runtime to inflate.
/// The stream remains in the incoming-resource row; a worker runtime explicitly materializes an
/// owned job while this borrow is live, while an inline runtime may consume the view directly.
#[repr(C)]
pub struct ResourceDecompressionOwed<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub stream: &'a [u8],
    pub uncompressed_data_bytes: u64,
}

/// A runtime's completed resource inflate, submitted as a later engine input.
#[repr(C)]
pub struct ResourceDecompressionCompleted<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    /// Empty means the runtime could not produce a valid bounded inflate.
    pub plaintext: &'a [u8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushCause {
    Startup,
    Interval,
    RouteChange,
    RatchetRotation,
    Shutdown,
}

impl PersistenceFlushCause {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Interval => "interval",
            Self::RouteChange => "route_change",
            Self::RatchetRotation => "ratchet_rotation",
            Self::Shutdown => "shutdown",
        }
    }
}

/// The independently stored half of a persistence flush.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushTarget {
    RoutingState,
    Ratchets,
}

impl PersistenceFlushTarget {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RoutingState => "routing_state",
            Self::Ratchets => "ratchets",
        }
    }
}

#[repr(C)]
pub enum Journaled<'a> {
    /// RNS 1.4.2's announce-handler `received_announce` callback as data.
    AnnounceHeard {
        observation: AnnounceObservation<'a>,
        rate_accounting: AnnounceRateAccounting,
    },

    SelfRatchetRotated {
        destination: DestinationHash,
    },

    /// A host persistence worker injected an ordered save notice into the engine journal.
    /// The engine itself performs no storage IO.
    PersistenceFlushed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },

    /// A host persistence worker injected an ordered save-failure notice into the engine
    /// journal. Storage-specific error detail stays in the host log.
    PersistenceFlushFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
    AnnounceHeldDropped {
        destination: DestinationHash,
        source_interface: InterfaceId,
        cause: HeldDropCause,
    },
    /// RNS 1.4.2's destination `set_packet_callback` delivery as data.
    ///
    /// Emitted synchronously before a corresponding [`ProofStrategy::ProveIf`](crate::routing::ProofStrategy::ProveIf)
    /// decision is requested and before any proof directive reaches egress. A host that
    /// durably records this callback in-stack therefore lands the inbound delivery before
    /// acknowledging it.
    Delivered(Delivery<'a>),

    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },

    RemoteControlPairingExpired {
        endpoint: crate::remote_control::RemoteControlPairingEndpoint,
    },

    RemoteControlPairingExpiryFailed {
        endpoint: crate::remote_control::RemoteControlPairingEndpoint,
        failure: crate::engine::CloseRemoteControlPairingFailure,
    },

    RemoteControlPairingAvailabilityObserved(
        crate::remote_control::RemoteControlPairingAvailabilityObservation<'a>,
    ),

    RemoteControlTargetPairingConfirmationRequired(
        crate::remote_control::RemoteControlTargetPairingAttemptView<'a>,
    ),

    RemoteControlTargetPairingControllerCommitted {
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    },

    RemoteControlTargetPairingAuthorizationRequired {
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
        grant: crate::remote_control::RemoteControlControllerGrant,
    },

    RemoteControlControllerPairingConfirmationRequired(
        crate::remote_control::RemoteControlControllerPairingAttemptView<'a>,
    ),

    RemoteControlControllerPairingPersistenceRequired(
        crate::remote_control::RemoteControlControllerPairingPersistenceView<'a>,
    ),

    RemoteControlControllerPairingExpired {
        aborted: crate::remote_control::RemoteControlControllerPairingAborted,
    },

    RemoteControlControllerPairingLinkClosed {
        aborted: crate::remote_control::RemoteControlControllerPairingAborted,
    },

    RemoteControlTargetPairingExpired {
        aborted: crate::remote_control::RemoteControlTargetPairingAborted,
    },

    RemoteControlTargetPairingLinkClosed {
        aborted: crate::remote_control::RemoteControlTargetPairingAborted,
    },

    RemoteControlTargetPairingCompletionRetentionExpired {
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    },

    RemoteControlTargetPairingCompletionLinkClosed {
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    },

    /// RNS 1.4.2's `set_link_established_callback` as data.
    LinkEstablished(LinkEstablished),

    /// RNS 1.4.2's `remote_identified` callback as data.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },

    /// RNS 1.4.2's request handler callback as data.
    RequestReceived {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    },

    /// RNS 1.4.2's request `response_callback` as data.
    ResponseReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },

    /// One verified segment of a split response resource; the receive gate refuses out-of-order chains, so these concatenate in arrival order.
    /// The request settles as `Settlement::SendRequest` when the final segment assembles, not through a [`Journaled::ResponseReceived`].
    ResponseSegmentReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        segment_index: u64,
        total_segments: u64,
        data: &'a [u8],
    },

    /// RNS 1.4.2 `Channel._receive`'s callback as data.
    ChannelMessageReceived {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },

    /// RNS 1.4.2's `set_link_closed_callback` as data.
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },

    /// RNS 1.4.2 `Link.receive`: a packet for an active link arrived on an interface other than the link's own, dropped unprocessed as a possible manipulation attempt.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },

    /// RNS 1.4.2's `resource_concluded` callback as data.
    /// `metadata` is the transfer's packed metadata, stripped from the stream head, opaque to the engine.
    ResourceReceived {
        link_id: LinkId,
        hash: ResourceHash,
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },

    /// The failure half of RNS 1.4.2's `resource_concluded` callback, with the cause the reference never names.
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
    },

    /// One segment of a split resource landed / progress toward [`Journaled::ResourceAssembled`].
    /// `metadata` rides segment one only, stripped from the stream head like the single-segment delivery.
    ResourceSegmentReceived {
        link_id: LinkId,
        original_hash: ResourceHash,
        segment_index: u64,
        total_segments: u64,
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },

    ResourceAssembled {
        link_id: LinkId,
        original_hash: ResourceHash,
        total_size_bytes: u64,
    },

    RouteRemoved {
        destination: DestinationHash,
        cause: RouteRemovalCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    MalformedRtt,
    LocallyClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanTarget {
    All,
    Only(InterfaceId),
    AllExcept(InterfaceId),
}

/// An order for something that must now happen outside the engine.
#[repr(C)]
pub enum Directive<'a, Work = NoOwedWork> {
    Fulfill(Work),

    Send {
        target: InterfaceId,
        bytes: &'a [u8],
    },
    SendIfOnline {
        target: InterfaceId,
        bytes: &'a [u8],
        on_send: &'a mut dyn FnMut(),
    },

    SendAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
        hops: u8,
        #[cfg(feature = "runtime-metrics")]
        origin: AnnounceOrigin,
    },
    SendToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },

    SendAnnounceToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
        hops: u8,
        #[cfg(feature = "runtime-metrics")]
        origin: AnnounceOrigin,
    },
    /// The driver calls `fill` exactly once, with at least `size_hint` bytes, even on a full lane (its own scratch). The engine's bookkeeping runs inside `fill`.
    EmitFrame {
        target: InterfaceId,
        size_hint: usize,
        fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    },

    #[cfg(feature = "runtime-metrics")]
    SendMeasuredLocalAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
    },

    #[cfg(feature = "runtime-metrics")]
    SendMeasuredLocalAnnounceToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },
}

impl<'a, Work> Directive<'a, Work> {
    /// Changes only [`Directive::Fulfill`]'s work type and leaves every routed directive intact.
    pub fn map_work<Mapped>(self, map: impl FnOnce(Work) -> Mapped) -> Directive<'a, Mapped> {
        match self {
            Self::Fulfill(work) => Directive::Fulfill(map(work)),
            Self::Send { target, bytes } => Directive::Send { target, bytes },
            Self::SendIfOnline {
                target,
                bytes,
                on_send,
            } => Directive::SendIfOnline {
                target,
                bytes,
                on_send,
            },
            Self::SendAnnounce {
                target,
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
            } => Directive::SendAnnounce {
                target,
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
            },
            Self::SendToFleet {
                supervisor,
                fan,
                bytes,
            } => Directive::SendToFleet {
                supervisor,
                fan,
                bytes,
            },
            Self::SendAnnounceToFleet {
                supervisor,
                fan,
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
            } => Directive::SendAnnounceToFleet {
                supervisor,
                fan,
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
            },
            Self::EmitFrame {
                target,
                size_hint,
                fill,
            } => Directive::EmitFrame {
                target,
                size_hint,
                fill,
            },
            #[cfg(feature = "runtime-metrics")]
            Self::SendMeasuredLocalAnnounce { target, bytes } => {
                Directive::SendMeasuredLocalAnnounce { target, bytes }
            }
            #[cfg(feature = "runtime-metrics")]
            Self::SendMeasuredLocalAnnounceToFleet {
                supervisor,
                fan,
                bytes,
            } => Directive::SendMeasuredLocalAnnounceToFleet {
                supervisor,
                fan,
                bytes,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PersistenceFlushCause, PersistenceFlushTarget};

    #[test]
    fn persistence_names_cover_the_wire_stable_vocabulary() {
        assert_eq!(
            [
                PersistenceFlushCause::Startup.name(),
                PersistenceFlushCause::Interval.name(),
                PersistenceFlushCause::RouteChange.name(),
                PersistenceFlushCause::RatchetRotation.name(),
                PersistenceFlushCause::Shutdown.name(),
            ],
            [
                "startup",
                "interval",
                "route_change",
                "ratchet_rotation",
                "shutdown",
            ],
        );
        assert_eq!(
            [
                PersistenceFlushTarget::RoutingState.name(),
                PersistenceFlushTarget::Ratchets.name(),
            ],
            ["routing_state", "ratchets"],
        );
    }
}
