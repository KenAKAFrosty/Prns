mod announce;
mod classification;
mod dispatch;
mod forward;
mod links;
mod outcome;
mod path_requests;
#[cfg(test)]
pub(super) mod testkit;
mod upstream_delivery;

pub use announce::{AcceptedAnnounce, AnnounceIngest, AnnounceVerifyOwed, RebroadcastDecision};
pub use classification::{ClassifiedInboundPacket, DataPacket, Ingress};
use forward::ForwardingArrival;
pub use forward::PacketToForward;
pub use links::ForwardedLinkRequestBody;
use links::{LinkRequestArrival, RelayOutcome};
pub(crate) use outcome::{AcceptedAnnounceEffect, IngestEffects};
pub use outcome::{
    DeferredCrypto, IgnoreReason, IngestPacketOutcome, LinkRttOwed,
    NON_TRANSPORTED_DATA_MAX_RECEIVED_HOPS,
};
use upstream_delivery::UpstreamDeliveryOutcome;
pub use upstream_delivery::{
    DecryptOwed, RatchetDecryptOwed, MAX_POOLED_RATCHETS, MAX_RATCHET_DECRYPT_PAYLOAD_LEN,
    MAX_SINGLE_TOKEN_LEN,
};

use crate::crypto::token_open_in_place;
use crate::crypto::{X25519PublicKey, X25519SecretKey};
use crate::engine::EngineState;
use crate::engine::InstantMillis;
use crate::engine::LinkClosedReason;
use crate::engine::PacketReceiptDelivered;
use crate::engine::MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN;
use crate::engine::PATH_REQUEST_DESTINATION;
use crate::identity::IdentityHash;
use crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{InterfaceCommonPolicy, InterfaceId, InterfaceKind, InterfaceMode};
use crate::routing::announce::defaults::{
    jitter_offset, DEFAULT_REBROADCAST_JITTER_WINDOW_MS, MAX_PEER_EMISSIONS, PATH_REQUEST_GRACE_MS,
    PATH_REQUEST_ROAMING_GRACE_MS,
};
use crate::routing::announce::destination_announce_limit::DestinationAnnounceVerdict;
use crate::routing::announce::held::{HeldDropCause, HoldOutcome};
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::AnnounceArrival;
use crate::routing::announce::{
    determine_acceptance, AnnounceAcceptanceDecision, AnnounceAcceptanceInput,
};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::send_single::DEFAULT_PER_HOP_TIMEOUT_MS;
use crate::routing::delivery::{
    Delivery, GroupDelivery, LinkDelivery, PlainDelivery, SingleDelivery,
};
use crate::routing::links::channel::parse_envelope;
use crate::routing::links::channel::table::ChannelTable;
use crate::routing::links::handshake::{
    link_proof_from, link_proof_parse, link_request_from, link_rtt_from, signalling_bytes_from,
    AcceptedLinkRequest, LinkProofVerifyOwed, LinkRequest, LinkRttError, LINK_REQUEST_KEYS_LEN,
    SIGNALLED_LINK_REQUEST_LEN,
};
use crate::routing::links::identify::peer_identity_from;
use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
use crate::routing::links::request::{
    parse_request_plaintext, parse_response_plaintext, RequestId,
};
use crate::routing::links::resources::send::ResourceProofClassification;
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::transported::{extra_link_proof_timeout_ms, TransportedLink};
use crate::routing::links::LinkId;
use crate::routing::path_requests::recursive::{
    RecursiveOutcome, RECURSIVE_PATH_REQUEST_TIMEOUT_MS,
};
use crate::routing::path_requests::seen::{PathRequestIdBytes, PathRequestNovelty};
use crate::routing::proof::{LinkProofOwed, ProofIngest, ProofObligation, ProofOwed};
use crate::routing::reverse_routes::{ReverseRouteEntry, DEFAULT_REVERSE_ROUTE_TIMEOUT_MS};
use crate::routing::tunnel::{
    parse_synthesize_payload, TunnelTransition, TUNNEL_SYNTHESIZE_DESTINATION, TUNNEL_TIMEOUT_MS,
};
use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use crate::routing::NextHop;
use crate::routing::{DropCause, RemovedRoute, RouteResponsiveness, UpsertRouteOutcome};
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::units::RttMillis;
use crate::wire::{ContextFlag, IfacFlag, PropagationType};
use crate::wire::{
    DestinationHash, DestinationType, PacketType, TransportId, WireAddress, WireContext, WireError,
    WirePacketHeader, BROADCAST_MTU, MAX_HOP_COUNT, TRUNCATED_HASH_BYTE_LEN,
};
use heapless::Vec as HeaplessVec;
