mod blackhole;
mod command_execution;
mod commands;
mod deadlines;
mod destination_identity;
mod introspection;
mod management;
mod node_egress;
mod node_ingress;
mod proof;
mod reaction;
mod registration;
mod route_evidence;
mod settlement;
mod state;
mod tunnel;
mod wake;

pub use blackhole::{
    BlackholeIdentityEffect, BlackholeSeedEffect, BlackholeSeedReport, UnblackholeIdentityEffect,
};
pub(crate) use destination_identity::RememberAnnouncedDestinationIdentityOutcome;
pub use destination_identity::{
    DestinationIdentityRetentionEffect, DestinationIdentitySeedOutcome, MarkDestinationUsedEffect,
    ReleaseDestinationEffect, RetainDestinationEffect, RetainIdentityEffect,
};
pub use management::{
    DropRouteEffect, DropRouteOutcome, DropRoutesViaEffect, DropRoutesViaOutcome,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "runtime-metrics")] {
        mod metrics;

        pub use metrics::{
            AnnounceCommandCounts, AnnounceCommandOutcome, AnnounceIngressCounts,
            AnnounceIngressOutcome, AnnounceOrigin, AnnounceSourceKind,
            EngineAnnounceMetricsSnapshot, EngineMetricsSnapshot, EnginePathRequestMetricsSnapshot,
            EngineResourceMetricsSnapshot, IgnoreReasonCounts, IgnoreReasonKind,
            InterfaceAnnounceMetricsSnapshot, InterfaceKindCounts, PathRequestIngressCounts,
            PathRequestIngressOutcome, PathRequestRelayCounts, PathRequestRelayOutcome,
            ResourceAdmissionEvent, ResourceAdmissionEventCounts, ResourceDirectionMetricsSnapshot,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(test, feature = "test-support"))] {
        pub mod test_support;
    }
}

pub use crate::routing::announce::{
    write_announce_wire_packet, write_path_response_announce_wire_packet,
    write_relayed_path_response_wire_packet, write_retransmitted_announce_wire_packet,
};
pub use crate::routing::path_requests::{
    write_path_request_wire_packet, PATH_REQUEST_DESTINATION, PATH_REQUEST_PAYLOAD_LEN,
};
pub use crate::routing::proof::{
    write_explicit_proof_wire_packet, write_implicit_proof_wire_packet,
    write_link_proof_wire_packet,
};
pub use crate::routing::RouteRemovalCause;
pub use crate::wire::WireError as EgressSerializeError;
pub use commands::*;
pub use introspection::{AnnounceRateState, RouteSnapshot};
pub use node_egress::ReemitAnnounce;
pub use node_ingress::{IngestIo, IngestPacketReport};
pub use proof::ResolvedReceiptSettlement;
pub use reaction::{
    Directive, EngineReaction, FanTarget, Journaled, LinkClosedReason, PersistenceFlushCause,
    PersistenceFlushTarget,
};
pub use registration::{
    PersistedRoutePreflightError, PersistedRouteSignaturePending, PersistedRouteVerificationError,
    RouteSeedOutcome, SetTransportIdentityError, VerifiedPersistedRoute,
};
pub use state::{
    EngineProtocolPolicy, EngineState, LinkMtuDiscovery, LocalHopCountOverride,
    LocalOriginHopCount, ProofForm, RecursivePathRequestDefault,
};
pub use tunnel::WriteTunnelSynthesizeError;
pub use wake::{NextWake, WakeReason, WakeSchedule, WakeSchedules};

pub use crate::routing::warmth::{Departure, DEPARTED_INTERFACE_GRACE_MS};

pub use crate::crypto::ratchets::{RatchetEntropy, RatchetPolicy};
pub use crate::routing::announce::emit::{
    AnnounceAppDataBytes, AnnounceRejection, AnnounceWriteError, AnnounceWriteFailure,
    CommandedAnnounceWriteOutcome, PathResponseWriteOutcome,
};
pub use crate::routing::announce::held::HeldDropCause;
pub use crate::routing::announce::AnnounceObservation;
pub use crate::routing::delivery::send_group::{SendGroupEntropy, SendGroupWriteError};
pub use crate::routing::delivery::send_plain::SendPlainPacketWriteError;
pub use crate::routing::delivery::send_single::{
    EncryptOwed, FinishSendSinglePacketOutcome, SendSinglePacketDispatch, SendSinglePacketEntropy,
    SendSinglePacketPrepared, SendSinglePacketWriteError, SendSinglePacketWriteOutcome,
    SendSinglePacketWriteRejection,
};
pub use crate::routing::ingress::{
    AcceptedAnnounce, AnnounceIngest, AnnounceVerifyOwed, ClassifiedInboundPacket, DataPacket,
    DecryptOwed, DeferredCrypto, IgnoreReason, IngestPacketOutcome, Ingress, LinkRttOwed,
    PacketToForward, ProtocolViolationKind, RatchetDecryptOwed, RebroadcastDecision,
};
pub use crate::routing::links::data::{
    link_mdu, LinkDataError, SendToLinkDispatch, SendToLinkWriteError, LINK_MDU,
};
pub use crate::routing::links::establish::{
    EstablishLinkEntropy, EstablishLinkWriteOutcome, LinkRequestDispatch,
    WriteEstablishLinkRejection, WriteLinkProofError, WriteLinkRttError, LINK_KEEPALIVE_MS,
};
pub use crate::routing::links::maintenance::{
    keepalive_ms_from, stale_ms_from, write_keepalive, write_link_close, LinkCloseDispatch,
    WriteLinkCloseError, KEEPALIVE_ECHO, KEEPALIVE_REQUEST,
};
pub use crate::routing::path_requests::pending::{
    CulledPathRequest, ExpiredPathRequest, SettledPathRequest, PATH_REQUEST_TIMEOUT_MS,
};
pub use crate::routing::path_requests::request_path::PathRequestWriteOutcome;
pub use crate::routing::path_requests::seen::PathRequestIdBytes;
pub use crate::routing::proof::{
    DeferredProofSign, ProofIngest, ProofObligation, ProofOwed, ProofRequest, WriteProofError,
};
pub use crate::units::InstantMillis;
