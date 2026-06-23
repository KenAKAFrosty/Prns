use crate::crypto::{token_open_in_place, TokenKey};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::engine::commands::CommandId;
use crate::engine::commands::Delivered;
use crate::engine::egress::PATH_REQUEST_DESTINATION;
use crate::engine::reaction::LinkClosedReason;
use crate::engine::EngineState;
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::{
    InboundPacket, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceMode,
};
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS, MAX_ANNOUNCE_REBROADCASTS,
    PATH_REQUEST_GRACE_MS, PATH_REQUEST_ROAMING_GRACE_MS,
};
use crate::routing::announce::rate_limit::AnnounceRateVerdict;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::Announce;
use crate::routing::announce::{AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::send_single::DEFAULT_PER_HOP_TIMEOUT_MS;
use crate::routing::delivery::{
    Delivery, GroupDelivery, LinkDelivery, PlainDelivery, SingleDelivery,
    PLAIN_DATA_MAX_RECEIVED_HOPS,
};
use crate::routing::links::channel::columns::ChannelColumns;
use crate::routing::links::channel::{parse_envelope, ChannelSequence, MessageType};
use crate::routing::links::handshake::{
    link_proof_from, link_request_from, link_rtt_from, signalling_bytes_from, LinkRequest,
    LinkRttError, LINK_REQUEST_KEYS_LEN, SIGNALLED_LINK_REQUEST_LEN,
};
use crate::routing::links::identify::peer_identity_from;
use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
use crate::routing::links::request::{
    parse_request_plaintext, parse_response_plaintext, RequestId,
};
use crate::routing::links::resources::{ResourceHash, MAP_HASH_LEN};
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::transported::{extra_link_proof_timeout_ms, TransportedLink};
use crate::routing::links::LinkId;
use crate::routing::path_requests::discovery::{
    DiscoveryOutcome, DISCOVERY_PATH_REQUEST_TIMEOUT_MS,
};
use crate::routing::path_requests::seen::{PathRequestIdBytes, PathRequestNovelty};
use crate::routing::proof::{LinkProofOwed, ProofIngest, ProofObligation, ProofOwed};
use crate::routing::request_handlers::RequestPathHash;
use crate::routing::reverse_routes::{ReverseRouteEntry, DEFAULT_REVERSE_ROUTE_TIMEOUT_MS};
use crate::routing::tunnel::{
    parse_synthesize_payload, TunnelTransition, TUNNEL_SYNTHESIZE_DESTINATION, TUNNEL_TIMEOUT_MS,
};
use crate::routing::upstream_app_destinations::{ProofStrategy, UpstreamAppDestinationKind};
use crate::routing::NextHop;
use crate::routing::{DropCause, RemovedRoute, RouteResponsiveness, UpsertRouteOutcome};
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::units::Rtt;
use crate::wire::{ContextFlag, IfacFlag, PropagationType};
use crate::wire::{
    DestinationHash, DestinationType, PacketType, TransportId, WireContext, WireError,
    WirePacketHeader, BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DataPacket<'a> {
    pub destination_type: DestinationType,
    pub destination: DestinationHash,
    pub context: WireContext,
    pub maybe_transport_id: Option<TransportId>,
    pub payload: &'a mut [u8],
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Ingress<'a> {
    Announce {
        announce: Announce<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        is_path_response: bool,
    },

    Data {
        data: DataPacket<'a>,
        header: WirePacketHeader,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    LinkRequest {
        payload: &'a [u8],
        header: WirePacketHeader,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    Proof {
        payload: &'a [u8],
        destination: DestinationHash,
        context: WireContext,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    Unparseable,
}

/// The hop count a packet arrives with, after the free local-instance transit is discounted. A
/// packet from a [`InterfaceKind::LocalClient`] (an app on this host sharing our instance) crossed
/// no real hop, so the per-interface increment is cancelled and the shared instance plus its apps
/// count as a single node — RNS `Transport.inbound`'s `hops -= 1` for local clients, applied at the
/// one place every packet type's hop count is set.
fn local_adjusted_hops(received_hops: u8, source: InterfaceId) -> u8 {
    if source.kind() == Some(InterfaceKind::LocalClient) {
        received_hops.saturating_sub(1)
    } else {
        received_hops
    }
}

impl<'a> Ingress<'a> {
    pub fn classify(packet: InboundPacket<'a>) -> Self {
        let InboundPacket {
            arrived_at,
            source_interface,
            bytes,
        } = packet;
        let (header, payload_offset) = match WirePacketHeader::parse(bytes) {
            Ok((header, payload)) => (header, bytes.len() - payload.len()),
            Err(_) => return Self::Unparseable,
        };
        if header.ifac_flag == IfacFlag::Authenticated {
            return Self::Unparseable;
        }
        let (_, payload) = bytes.split_at_mut(payload_offset);

        let received_hops = local_adjusted_hops(header.hops.saturating_add(1), source_interface);

        match header.packet_type {
            PacketType::Announce => {
                if header.destination_type != DestinationType::Single {
                    return Self::Unparseable;
                }

                // Shared so it stays `Copy`: `from_wire` lends `&'a` into the announce and
                // the debug round-trip reads payload again — a `&mut` would move on first use.
                let payload: &'a [u8] = payload;
                let Ok(announce) = Announce::from_wire(&header, payload) else {
                    return Self::Unparseable;
                };

                // Debug self-check: parse↔serialize round-trip on every
                // accepted announce. If `to_wire` ever drifts from
                // `from_wire`, the engine would silently re-emit a
                // signature-broken packet on rebroadcast. Cheap in
                // debug (one BROADCAST_MTU-sized scratch + compare), zero in
                // release.
                debug_assert!(
                    {
                        let mut scratch = [0u8; BROADCAST_MTU];
                        announce
                            .to_wire(&mut scratch)
                            .map(|n| &scratch[..n] == payload)
                            .unwrap_or(false)
                    },
                    "Announce::to_wire(from_wire(payload)) must equal payload"
                );

                Self::Announce {
                    announce,
                    received_hops,
                    source_interface,
                    arrived_at,
                    next_hop: header.transport_id.map_or(NextHop::Direct, NextHop::Via),
                    is_path_response: header.context == WireContext::PathResponse,
                }
            }
            PacketType::Data => Self::Data {
                data: DataPacket {
                    destination_type: header.destination_type,
                    destination: header.destination,
                    context: header.context,
                    maybe_transport_id: header.transport_id,
                    payload,
                },
                header,
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::LinkRequest => Self::LinkRequest {
                payload,
                header,
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::Proof => Self::Proof {
                payload,
                destination: header.destination,
                context: header.context,
                received_hops,
                source_interface,
                arrived_at,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceIngest {
    Accepted(AcceptedAnnounce),
    Ignored,
    /// The interface is bursting and this announce was for an unknown destination,
    /// so it was parked in the held queue to be drip-released once the burst subsides
    /// — RNS `Interface.hold_announce` (Interfaces/Interface.py:228).
    Held,
}

/// The route an accepted announce just took — what an app needs to discover
/// the peer behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedAnnounce {
    pub destination: DestinationHash,
    pub hops: u8,
    pub rebroadcast: RebroadcastDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebroadcastDecision {
    Scheduled,
    NotATransportNode,
    NoTransportInterfaces,
    /// A path response is learned but never re-flooded — the answer is for the
    /// requester, not the network (RNS Transport.py:1884).
    TerminalPathResponse,
    /// The route is learned, but the destination is announcing faster than the
    /// receiving interface's rate target allows, so its rebroadcast is suppressed
    /// for a penalty window (RNS Transport.py:1835-1887).
    RateBlocked,
}

/// One forwarded LINKREQUEST's payload, owned: at most the keys and the
/// (possibly clamped, possibly stripped) signalling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardedLinkRequestBody {
    pub bytes: [u8; SIGNALLED_LINK_REQUEST_LEN],
    pub len: usize,
}

impl ForwardedLinkRequestBody {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestPacketOutcome<'p> {
    Announce(AnnounceIngest),
    Delivery {
        delivery: Delivery<'p>,
        proof: ProofObligation,
    },
    Proof(ProofIngest),
    Forward(PacketToForward<'p>),
    /// A path request arrived for one of our own destinations — the runtime
    /// owes a path-response announce for it.
    AnswerPathRequest {
        destination: DestinationHash,
    },
    /// A path request arrived for a destination we relay but do not own — the
    /// cached announce is scheduled as a directed answer after the request grace,
    /// letting directly reachable peers respond first.
    ScheduledPathResponse {
        destination: DestinationHash,
    },
    /// A path request arrived for a destination we neither own nor hold a route to,
    /// either from a local client of our shared instance or on an interface whose
    /// mode discovers unknown paths (RNS `DISCOVER_PATHS_FOR`). We forward it on the
    /// requester's behalf on every other transport interface (RNS Transport.py:3004
    /// from a local client, :3013 recursive discovery); the asking interface is
    /// remembered so the answering announce can be steered straight back to it.
    ForwardPathRequestForDiscovery {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// A path request arrived from the wider network for a destination we do not
    /// hold, while apps share our instance. We offer it to those local clients only
    /// (RNS Transport.py:3041) in case one owns the destination, without recursing
    /// out across the network; the asking interface is remembered to steer the
    /// answer home.
    RelayPathRequestToLocalClients {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// The initiator of an active link revealed its identity, and the
    /// signature checked out — surfaced to the app, RNS 1.3.1's
    /// `remote_identified` callback.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    /// A sealed request passed the registry's allow gate — the app owes the
    /// response, answered back with a `Respond` command naming `request_id`.
    RequestReceived {
        link_id: LinkId,
        request_id: RequestId,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: Rtt,
        data: &'p [u8],
    },
    /// A sealed response named an outstanding request's id — the command
    /// settles Delivered and the bytes ride the journal.
    ResponseSettled {
        id: CommandId,
        delivered: Delivered,
        link_id: LinkId,
        request_id: RequestId,
        data: &'p [u8],
    },
    /// A decrypted channel envelope arrived on an active link. The engine owes
    /// the unconditional ack (`packet_hash` names it) and, once the receive
    /// algorithm runs, the in-order messages it unblocks. `payload` is the
    /// envelope body, borrowed from the arriving packet.
    ChannelDataReceived {
        link_id: LinkId,
        message_type: MessageType,
        sequence: ChannelSequence,
        payload: &'p [u8],
        packet_hash: PacketHash,
    },
    /// A part request named one of our outgoing transfers — the engine owes
    /// the requested parts raw from the register, and a hashmap update when
    /// the receiver's names ran dry.
    OwesResourceParts {
        link_id: LinkId,
        hash: ResourceHash,
        requested: &'p [u8],
        exhausted_at: Option<[u8; MAP_HASH_LEN]>,
    },

    ResourceDelivered {
        id: CommandId,
    },
    /// An advertisement passed the strategy and capacity gates and its
    /// transfer is registered. The engine now owes the first part request.
    OwesResourcePull {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// Every part of an inbound transfer has landed.
    /// The engine owes the assembly: open, verify, prove, journal.
    OwesResourceAssembly {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// A part landed mid-window — the transfer advanced and its watchdog moved
    /// to the next part-round deadline, but no part request or assembly is owed
    /// yet. Nothing leaves for the peer; the resource lane must still resync to
    /// the freshly-set deadline, which `Ignored` would silently strand later.
    ResourceProgressed,
    ResourceConcludedFailed {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// The receiver refused an offered transfer with `RESOURCE_RCL` — the
    /// send settles rejected-by-peer; the register row is already gone.
    ResourceRejectedByPeer {
        id: CommandId,
    },
    /// A link request in transport booked a transported row — the rewritten
    /// request (re-headered, MTU signalling clamped to this path segment) is
    /// owed to the next hop.
    TransportedLinkRequest {
        header: WirePacketHeader,
        body: ForwardedLinkRequestBody,
        fire_on: InterfaceId,
    },
    /// A link request arrived for one of our own destinations — the engine
    /// owes the signed LRPROOF that brings the link up.
    OwesLinkProof {
        request: LinkRequest,
        identity: IdentityHash,
        proof_strategy: ProofStrategy,
        received_hops: u8,
        arrived_at: InstantMillis,
    },
    /// The LRPROOF for a link we initiated validated against the announced
    /// identity — the engine owes the encrypted LRRTT that activates both ends.
    OwesLinkRtt {
        link_id: LinkId,
        responder_encryption: X25519PublicKey,
        responder_signing: Ed25519PublicKey,
        command_id: CommandId,
        rtt: Rtt,
        mtu: usize,
    },
    /// The LRRTT for a handshake we answered opened under the session key —
    /// the link is ACTIVE.
    LinkActivated {
        link_id: LinkId,
        rtt_ms: u64,
    },
    /// A keepalive request arrived on a link we answer for — the engine owes
    /// the echo back on the arrival lane.
    OwesKeepaliveEcho {
        link_id: LinkId,
    },
    /// The peer closed the link with its sealed LINKCLOSE; the row is gone.
    LinkClosedByPeer {
        link_id: LinkId,
    },
    /// The engine owes the peer a sealed LINKCLOSE for a link it is dropping.
    OwesLinkClose {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    /// RNS 1.3.1 `Link.receive` (Link.py:975): a packet for an active link arrived on an
    /// interface other than the one the link is attached to. The reference treats this as a
    /// possible manipulation attempt — the packet is dropped, never processed; we surface the
    /// mismatch rather than swallowing it silently.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    TunnelObserved {
        expires: InstantMillis,
    },
    Ignored,
}

/// RNS 1.3.1 `Transport.packet_filter`'s duplicate-filter exemptions, as a
/// switching relay must honor them: these contexts retry byte-identically by
/// design — a re-sent resource part is the same raw slice, a keepalive is
/// the same single byte — so deduplicating them severs every retry that
/// crosses the relay.
fn switch_exempt_from_duplicate_filter(context: WireContext) -> bool {
    matches!(
        context,
        WireContext::KeepAlive
            | WireContext::Resource
            | WireContext::ResourceRequest
            | WireContext::ResourceProof
            | WireContext::CacheRequest
            | WireContext::Channel
    )
}

/// A packet in transport, re-framed and owed to another interface — RNS 1.3.1
/// Transport.py:1556-1580 (data riding the path table onward) and :2254 (a
/// proof riding the reverse table home).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketToForward<'p> {
    pub header: WirePacketHeader,
    pub payload: &'p [u8],
    pub fire_on: InterfaceId,
}

impl PacketToForward<'_> {
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, WireError> {
        let header_len = self.header.write(buf)?;
        let total_len = header_len + self.payload.len();
        if buf.len() < total_len {
            return Err(WireError::BufferTooShort);
        }
        buf[header_len..total_len].copy_from_slice(self.payload);
        Ok(total_len)
    }
}

/// A parsed path-request payload (RNS 1.3.1 `Transport.path_request_handler`):
/// the requested destination, the id the network dedups on, and — only in the
/// transport form — the requester's transport id.
struct PathRequest {
    destination: DestinationHash,
    requester_transport_id: Option<TransportId>,
    id: PathRequestIdBytes,
}

/// Why a payload is not an answerable path request — the reference's two
/// non-answering cases (Transport.py:2864), distinct because one is malformed
/// and the other is well-formed policy.
#[derive(Debug, PartialEq, Eq)]
enum PathRequestError {
    /// Too short to carry a destination hash — not a path request at all.
    NoDestination,
    /// A destination with no id; recognized, but never answered.
    NoId,
}

impl PathRequest {
    fn parse(payload: &[u8]) -> Result<Self, PathRequestError> {
        let destination = payload
            .get(..TRUNCATED_HASH_BYTE_LEN)
            .and_then(DestinationHash::from_slice)
            .ok_or(PathRequestError::NoDestination)?;
        let (requester_transport_id, id_region) = if payload.len() > TRUNCATED_HASH_BYTE_LEN * 2 {
            (
                TransportId::from_slice(
                    &payload[TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN * 2],
                ),
                &payload[TRUNCATED_HASH_BYTE_LEN * 2..],
            )
        } else if payload.len() > TRUNCATED_HASH_BYTE_LEN {
            (None, &payload[TRUNCATED_HASH_BYTE_LEN..])
        } else {
            return Err(PathRequestError::NoId);
        };
        let used = id_region.len().min(TRUNCATED_HASH_BYTE_LEN);
        let mut id = PathRequestIdBytes::default();
        id[..used].copy_from_slice(&id_region[..used]);
        Ok(Self {
            destination,
            requester_transport_id,
            id,
        })
    }

    /// RNS 1.3.1 `Transport.path_request`: the path loops if the requester is the
    /// very next hop we would answer with.
    fn loops_back_through_requester(&self, next_hop: NextHop) -> bool {
        matches!((next_hop, self.requester_transport_id), (NextHop::Via(via), Some(id)) if via == id)
    }
}

fn path_response_grace_ms(source_interface: InterfaceId, view: &[InterfaceConfig]) -> u64 {
    let roaming = view
        .iter()
        .find(|config| config.id == source_interface)
        .is_some_and(|config| config.mode == InterfaceMode::Roaming);
    if roaming {
        PATH_REQUEST_GRACE_MS + PATH_REQUEST_ROAMING_GRACE_MS
    } else {
        PATH_REQUEST_GRACE_MS
    }
}

fn request_echoes_into_its_own_roaming_segment(
    route_learned_on: InterfaceId,
    source_interface: InterfaceId,
    view: &[InterfaceConfig],
) -> bool {
    route_learned_on == source_interface
        && iface_config(view, source_interface)
            .is_some_and(|config| config.mode == InterfaceMode::Roaming)
}

impl<S: StorageLayout> EngineState<S> {
    #[must_use]
    pub fn ingest_packet<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'p> {
        self.ingest_packet_with(packet, jitter, interfaces, &mut |_| {})
    }

    #[must_use]
    pub(crate) fn ingest_packet_with<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> IngestPacketOutcome<'p> {
        self.ingested_packet_count = self.ingested_packet_count.saturating_add(1);

        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
                is_path_response,
            } => {
                self.interface_announce_limits
                    .record(source_interface, arrived_at);
                let unknown = !self.routing_table.has_route(&announce.destination);
                let awaiting = self.pending_path_requests.contains(&announce.destination)
                    || self.discovery_path_requests.contains(&announce.destination);
                if unknown
                    && !awaiting
                    && self
                        .interface_announce_limits
                        .should_limit(source_interface, arrived_at)
                {
                    self.held_announces.hold(
                        received_hops,
                        source_interface,
                        next_hop,
                        is_path_response,
                        &announce,
                    );
                    IngestPacketOutcome::Announce(AnnounceIngest::Held)
                } else {
                    IngestPacketOutcome::Announce(self.ingest_announce(
                        announce,
                        received_hops,
                        source_interface,
                        arrived_at,
                        next_hop,
                        is_path_response,
                        jitter,
                        interfaces,
                        on_removed,
                    ))
                }
            }

            Ingress::Data {
                data,
                header,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                if data.destination_type == DestinationType::Link {
                    let link_id = LinkId::new(*data.destination.as_bytes());
                    if self.links.phase_for(&link_id).is_none()
                        && data.context != WireContext::LinkRequestProof
                    {
                        if let Ok(switch) = self.transported_links.switch(
                            &link_id,
                            source_interface,
                            received_hops,
                            arrived_at,
                        ) {
                            if !switch_exempt_from_duplicate_filter(data.context) {
                                let packet_hash = PacketHash::of_fields(
                                    DestinationType::Link,
                                    PacketType::Data,
                                    &data.destination,
                                    data.context,
                                    data.payload,
                                );
                                match self.packet_hash_history.remember(packet_hash) {
                                    RememberPacketOutcome::AlreadyKnown => {
                                        return IngestPacketOutcome::Ignored
                                    }
                                    RememberPacketOutcome::StoredFresh
                                    | RememberPacketOutcome::StoredAfterRotation => {}
                                }
                            }
                            let forward = IngestPacketOutcome::Forward(PacketToForward {
                                header: WirePacketHeader {
                                    ifac_flag: IfacFlag::Open,
                                    context_flag: ContextFlag::Unset,
                                    propagation: PropagationType::Broadcast,
                                    destination_type: DestinationType::Link,
                                    packet_type: PacketType::Data,
                                    hops: received_hops,
                                    transport_id: None,
                                    destination: data.destination,
                                    context: data.context,
                                },
                                payload: data.payload,
                                fire_on: switch.fire_on,
                            });
                            return forward;
                        }
                    }
                    if let Some(LinkPhase::Active {
                        attached_interface, ..
                    }) = self.links.phase_for(&link_id)
                    {
                        if *attached_interface != source_interface {
                            return IngestPacketOutcome::LinkInterfaceMismatch {
                                link_id,
                                attached_interface: *attached_interface,
                                arrived_on: source_interface,
                            };
                        }
                    }
                    return match data.context {
                        WireContext::LinkRtt => self.classify_link_rtt(
                            &data.destination,
                            data.payload,
                            source_interface,
                            arrived_at,
                        ),
                        WireContext::None => {
                            self.classify_link_data(data, source_interface, arrived_at)
                        }
                        WireContext::KeepAlive => {
                            self.classify_keepalive(&data.destination, data.payload, arrived_at)
                        }
                        WireContext::LinkClose => self.classify_link_close(data),
                        WireContext::LinkIdentify => self.classify_link_identify(data, arrived_at),
                        WireContext::Request => self.classify_request_over_link(data, arrived_at),
                        WireContext::Response => self.classify_response_over_link(data, arrived_at),
                        WireContext::ResourceRequest => {
                            self.classify_resource_request(data, arrived_at)
                        }
                        WireContext::ResourceAdvertisement => {
                            self.classify_resource_advertisement(data, arrived_at)
                        }
                        WireContext::Resource => self.classify_resource_part(data, arrived_at),
                        WireContext::ResourceHashUpdate => {
                            self.classify_resource_hashmap_update(data, arrived_at)
                        }
                        WireContext::ResourceInitiatorCancel => {
                            self.classify_resource_cancel(data, arrived_at)
                        }
                        WireContext::ResourceReceiverCancel => {
                            self.classify_resource_receiver_cancel(data, arrived_at)
                        }
                        WireContext::Channel => self.classify_channel_data(data, arrived_at),
                        _ => IngestPacketOutcome::Ignored,
                    };
                }
                if data.destination == PATH_REQUEST_DESTINATION
                    && data.destination_type == DestinationType::Plain
                {
                    return self.ingest_path_request(
                        &data,
                        source_interface,
                        arrived_at,
                        interfaces,
                    );
                }
                if data.destination == TUNNEL_SYNTHESIZE_DESTINATION
                    && data.destination_type == DestinationType::Plain
                {
                    return self.ingest_tunnel_synthesize(&data, source_interface, arrived_at);
                }
                let not_for_upstream_app = self
                    .upstream_app_destinations
                    .lookup(&data.destination, data.destination_type)
                    .is_none();
                let in_transport_through_us = self.transport_id.is_some()
                    && header.transport_id == self.transport_id
                    && not_for_upstream_app;
                let local_client_transit = not_for_upstream_app
                    && data.destination_type == DestinationType::Single
                    && (source_interface.kind() == Some(InterfaceKind::LocalClient)
                        || self.routes_via_local_client(&data.destination));
                if in_transport_through_us || local_client_transit {
                    return match self.maybe_forward(
                        header,
                        data.payload,
                        received_hops,
                        source_interface,
                        arrived_at,
                    ) {
                        Some(forward) => IngestPacketOutcome::Forward(forward),
                        None => IngestPacketOutcome::Ignored,
                    };
                }
                match self.maybe_upstream_delivery(
                    data,
                    received_hops,
                    source_interface,
                    arrived_at,
                ) {
                    Some((delivery, proof)) => IngestPacketOutcome::Delivery { delivery, proof },
                    None => IngestPacketOutcome::Ignored,
                }
            }

            Ingress::Proof {
                payload,
                destination,
                context,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                if context == WireContext::LinkRequestProof {
                    return self.classify_link_proof(
                        &destination,
                        payload,
                        received_hops,
                        source_interface,
                        arrived_at,
                    );
                }
                if context == WireContext::ResourceProof {
                    if let Some(outcome) =
                        self.classify_resource_proof(&destination, payload, arrived_at)
                    {
                        return outcome;
                    }
                }
                let link_id = LinkId::new(*destination.as_bytes());
                if self.links.phase_for(&link_id).is_none() {
                    if let Ok(switch) = self.transported_links.switch(
                        &link_id,
                        source_interface,
                        received_hops,
                        arrived_at,
                    ) {
                        if !switch_exempt_from_duplicate_filter(context) {
                            let packet_hash = PacketHash::of_fields(
                                DestinationType::Link,
                                PacketType::Proof,
                                &destination,
                                context,
                                payload,
                            );
                            match self.packet_hash_history.remember(packet_hash) {
                                RememberPacketOutcome::AlreadyKnown => {
                                    return IngestPacketOutcome::Ignored
                                }
                                RememberPacketOutcome::StoredFresh
                                | RememberPacketOutcome::StoredAfterRotation => {}
                            }
                        }
                        return IngestPacketOutcome::Forward(PacketToForward {
                            header: WirePacketHeader {
                                ifac_flag: IfacFlag::Open,
                                context_flag: ContextFlag::Unset,
                                propagation: PropagationType::Broadcast,
                                destination_type: DestinationType::Link,
                                packet_type: PacketType::Proof,
                                hops: received_hops,
                                transport_id: None,
                                destination,
                                context,
                            },
                            payload,
                            fire_on: switch.fire_on,
                        });
                    }
                }
                if let Some(reverse) = self.reverse_routes.take(&destination, arrived_at) {
                    // The proof must arrive back over the interface we forwarded
                    // toward; anything else is dropped (Transport.py:2256).
                    if reverse.outbound_interface != source_interface {
                        return IngestPacketOutcome::Ignored;
                    }
                    return IngestPacketOutcome::Forward(PacketToForward {
                        header: WirePacketHeader {
                            ifac_flag: IfacFlag::Open,
                            context_flag: ContextFlag::Unset,
                            propagation: PropagationType::Broadcast,
                            destination_type: DestinationType::Single,
                            packet_type: PacketType::Proof,
                            hops: received_hops,
                            transport_id: None,
                            destination,
                            context: WireContext::None,
                        },
                        payload,
                        fire_on: reverse.received_interface,
                    });
                }
                if let Some((id, delivered)) =
                    self.settle_channel_ack(&link_id, payload, arrived_at)
                {
                    // A channel send's proof is link traffic too — extend liveness.
                    self.links.note_inbound(&link_id, arrived_at);
                    return IngestPacketOutcome::Proof(ProofIngest::SendChannelDelivered {
                        id,
                        delivered,
                    });
                }
                let outcome = self.ingest_proof(payload, arrived_at);
                if matches!(outcome, ProofIngest::SendLinkDelivered { .. }) {
                    // The validated proof is link traffic: it extends the link's
                    // liveness exactly as RNS 1.3.1's `link.last_proof` does.
                    self.links
                        .note_inbound(&LinkId::new(*destination.as_bytes()), arrived_at);
                }
                IngestPacketOutcome::Proof(outcome)
            }

            Ingress::LinkRequest {
                payload,
                header,
                received_hops,
                source_interface,
                arrived_at,
            } => self.classify_link_request(
                &header,
                payload,
                received_hops,
                source_interface,
                arrived_at,
                interfaces,
            ),
            Ingress::Unparseable => IngestPacketOutcome::Ignored,
        }
    }

    /// RNS 1.3.1 Transport.inbound's LINKREQUEST-in-transport arm: a request
    /// addressed through us toward a routed destination books a transported row
    /// and forwards — re-headered for its remaining distance, its MTU
    /// signalling clamped to what this path segment can actually carry.
    /// RNS 1.3.1 Transport.inbound's LRPROOF arm: the relay validates the
    /// proof itself against the announced identity it holds for the
    /// destination — over the right side, at the right distance — then marks
    /// the row live and sends the proof on toward the initiator.
    fn classify_transported_link_proof<'p>(
        &mut self,
        link_id: &LinkId,
        payload: &'p [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let Some(entry) = self.transported_links.entry_for(link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let destination = entry.destination;
        let next_hop_interface = entry.next_hop_interface;
        let received_interface = entry.received_interface;
        let Some(retained) = self.routing_table.retained_announce_for(&destination) else {
            return IngestPacketOutcome::Ignored;
        };
        let responder_signing = *retained.announce.public_keys.signing.as_ed25519();
        if link_proof_from(link_id, payload, &responder_signing).is_err() {
            return IngestPacketOutcome::Ignored;
        }
        let Ok(switch) = self.transported_links.validate_by_proof(
            link_id,
            source_interface,
            received_hops,
            arrived_at,
        ) else {
            return IngestPacketOutcome::Ignored;
        };
        self.mark_interface_dirty(next_hop_interface);
        self.mark_interface_dirty(received_interface);
        self.routing_table
            .mark_responsiveness(&destination, RouteResponsiveness::Responsive);
        IngestPacketOutcome::Forward(PacketToForward {
            header: WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: DestinationType::Link,
                packet_type: PacketType::Proof,
                hops: received_hops,
                transport_id: None,
                destination: DestinationHash::new(*link_id.as_bytes()),
                context: WireContext::LinkRequestProof,
            },
            payload,
            fire_on: switch.fire_on,
        })
    }

    fn classify_transported_link_request(
        &mut self,
        header: &WirePacketHeader,
        request: &LinkRequest,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        view: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'static> {
        let addressed_through_us =
            self.transport_id.is_some() && header.transport_id == self.transport_id;
        let local_client_transit = source_interface.kind() == Some(InterfaceKind::LocalClient)
            || self.routes_via_local_client(&request.destination);
        if !addressed_through_us && !local_client_transit {
            return IngestPacketOutcome::Ignored;
        }
        let Some(route) = self
            .routing_table
            .forwarding_route_for(&request.destination)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let fire_on = route.receiving_interface;
        let forwarded_header = if route.hops.0 > 1 {
            let NextHop::Via(next) = route.next_hop else {
                return IngestPacketOutcome::Ignored;
            };
            WirePacketHeader {
                hops: received_hops,
                transport_id: Some(next),
                ..*header
            }
        } else {
            WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: header.destination_type,
                packet_type: header.packet_type,
                hops: received_hops,
                transport_id: None,
                destination: header.destination,
                context: header.context,
            }
        };

        let maybe_arrival_hw_mtu =
            iface_config(view, source_interface).and_then(|c| c.hardware_mtu);
        let maybe_outbound_hw_mtu = iface_config(view, fire_on).and_then(|c| c.hardware_mtu);
        let mut body = ForwardedLinkRequestBody {
            bytes: [0u8; SIGNALLED_LINK_REQUEST_LEN],
            len: LINK_REQUEST_KEYS_LEN,
        };
        body.bytes[..32].copy_from_slice(&request.initiator_encryption.0);
        body.bytes[32..LINK_REQUEST_KEYS_LEN].copy_from_slice(&request.initiator_signing.0);
        if request.signalled {
            match maybe_outbound_hw_mtu {
                None => {}
                Some(outbound_hw) => {
                    let clamped = request
                        .mtu
                        .min(outbound_hw)
                        .min(maybe_arrival_hw_mtu.unwrap_or(usize::MAX));
                    body.bytes[LINK_REQUEST_KEYS_LEN..SIGNALLED_LINK_REQUEST_LEN]
                        .copy_from_slice(&signalling_bytes_from(clamped, request.mode));
                    body.len = SIGNALLED_LINK_REQUEST_LEN;
                }
            }
        }

        let bitrate = iface_config(view, source_interface).and_then(|c| c.bitrate_bps);
        let proof_timeout = InstantMillis(
            arrived_at
                .0
                .saturating_add(extra_link_proof_timeout_ms(bitrate))
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(route.hops.0.max(1))),
                ),
        );
        if self
            .transported_links
            .track(TransportedLink {
                link_id: request.link_id,
                destination: request.destination,
                next_hop: match route.next_hop {
                    NextHop::Via(next) => Some(next),
                    NextHop::Direct => None,
                },
                next_hop_interface: fire_on,
                received_interface: source_interface,
                taken_hops: received_hops,
                remaining_hops: route.hops.0,
                validated: false,
                last_active: arrived_at,
                proof_timeout,
            })
            .is_err()
        {
            return IngestPacketOutcome::Ignored;
        }
        IngestPacketOutcome::TransportedLinkRequest {
            header: forwarded_header,
            body,
            fire_on,
        }
    }

    fn classify_link_proof<'p>(
        &mut self,
        destination: &DestinationHash,
        payload: &'p [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*destination.as_bytes());
        let Some(LinkPhase::Pending {
            destination: link_destination,
            requested_at,
            command_id,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return self.classify_transported_link_proof(
                &link_id,
                payload,
                received_hops,
                source_interface,
                arrived_at,
            );
        };
        let Some(retained) = self.routing_table.retained_announce_for(link_destination) else {
            return IngestPacketOutcome::Ignored;
        };
        let responder_signing = *retained.announce.public_keys.signing.as_ed25519();
        let Ok(proof) = link_proof_from(&link_id, payload, &responder_signing) else {
            return IngestPacketOutcome::Ignored;
        };
        IngestPacketOutcome::OwesLinkRtt {
            link_id,
            responder_encryption: proof.responder_encryption,
            responder_signing,
            command_id: *command_id,
            rtt: Rtt::measured_between(*requested_at, arrived_at),
            mtu: if proof.mtu == 0 {
                BROADCAST_MTU
            } else {
                proof.mtu
            },
        }
    }

    fn classify_link_rtt(
        &mut self,
        destination: &DestinationHash,
        payload: &[u8],
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*destination.as_bytes());
        let Some(LinkPhase::Handshake {
            key, requested_at, ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let reported = match link_rtt_from(&link_id, payload, key) {
            Ok(reported) => reported,
            Err(LinkRttError::Malformed) => {
                return IngestPacketOutcome::OwesLinkClose {
                    link_id,
                    reason: LinkClosedReason::Protocol,
                };
            }
            Err(_) => return IngestPacketOutcome::Ignored,
        };
        let measured = Rtt::measured_between(*requested_at, arrived_at);
        let rtt = measured.max(reported.rtt);
        if self
            .links
            .activate_responding(&link_id, rtt, source_interface, arrived_at)
            .is_err()
        {
            return IngestPacketOutcome::Ignored;
        }
        self.mark_interface_dirty(source_interface);
        let responder_destination = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                role: LinkRole::Responder { destination, .. },
                ..
            }) => Some(*destination),
            _ => None,
        };
        if let Some(destination) = responder_destination {
            let default_strategy = self
                .upstream_app_destinations
                .default_resource_strategy(&destination);
            let _ = self.links.set_resource_strategy(&link_id, default_strategy);
        }
        IngestPacketOutcome::LinkActivated {
            link_id,
            rtt_ms: rtt.millis(),
        }
    }

    fn classify_link_data<'p>(
        &mut self,
        data: DataPacket<'p>,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        if !matches!(
            self.links.phase_for(&link_id),
            Some(LinkPhase::Active { .. }),
        ) {
            return IngestPacketOutcome::Ignored;
        }

        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        let Some(LinkPhase::Active { key, role, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let owed = match role {
            LinkRole::Initiator { .. } => None,
            LinkRole::Responder {
                destination,
                identity,
                proof_strategy,
            } => Some((
                *proof_strategy,
                LinkProofOwed {
                    link_id,
                    packet_hash,
                    identity: *identity,
                    destination: *destination,
                },
            )),
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::Delivery {
            delivery: Delivery::Link(LinkDelivery {
                link_id,
                plaintext,
                arrived_at,
                source_interface,
            }),
            proof: match owed {
                Some((ProofStrategy::ProveAll, owed)) => ProofObligation::OwedOverLink(owed),
                Some((ProofStrategy::ProveIf, owed)) => ProofObligation::OwedIfAppOverLink(owed),
                Some((ProofStrategy::ProveNone, _)) | None => ProofObligation::None,
            },
        }
    }

    /// RNS 1.3.1 `Link.receive`'s CHANNEL branch: an encrypted channel packet on
    /// an open (active) link. Unlike app link data, channel packets carry the
    /// protocol's own sequence dedup, so the packet-hash duplicate filter is
    /// skipped here (a byte-identical retransmit must reach the receive algorithm
    /// to be re-acked, exactly as RNS exempts CHANNEL from `packet_filter`). The
    /// hash is still taken — over the ciphertext, before the in-place open — for
    /// the ack the arrival unconditionally owes.
    fn classify_channel_data<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok(envelope) = parse_envelope(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ChannelDataReceived {
            link_id,
            message_type: envelope.message_type,
            sequence: envelope.sequence,
            payload: envelope.payload,
            packet_hash,
        }
    }

    fn classify_link_identify(
        &mut self,
        data: DataPacket<'_>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let Some(LinkPhase::Active {
            key,
            role: LinkRole::Responder { .. },
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(identity) = peer_identity_from(&link_id, plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_identified(&link_id, identity);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::PeerIdentified { link_id, identity }
    }

    fn classify_request_over_link<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Some(LinkPhase::Active {
            key,
            role: LinkRole::Responder { destination, .. },
            remote_identity,
            rtt,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let destination = *destination;
        let remote_identity = *remote_identity;
        let request_rtt = *rtt;
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok(parsed) = parse_request_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        if !self
            .request_handlers
            .permits(&destination, &parsed.path_hash, remote_identity.as_ref())
        {
            return IngestPacketOutcome::Ignored;
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::RequestReceived {
            link_id,
            request_id: RequestId::of_packet(&packet_hash),
            path_hash: parsed.path_hash,
            requested_at: parsed.requested_at,
            rtt: request_rtt,
            data: parsed.data,
        }
    }

    fn classify_response_over_link<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok((request_id, response_data)) = parse_response_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(proven) = self.receipts.settle_by_request_id(request_id.as_bytes()) else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResponseSettled {
            id: proven.command_id,
            delivered: Delivered {
                rtt: Rtt::measured_between(proven.sent_at, arrived_at),
            },
            link_id,
            request_id,
            data: response_data,
        }
    }

    fn classify_keepalive(
        &mut self,
        destination: &DestinationHash,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*destination.as_bytes());
        let &[byte] = payload else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(LinkPhase::Active { role, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        match (role, byte) {
            (LinkRole::Responder { .. }, KEEPALIVE_REQUEST) => {
                self.links.note_inbound(&link_id, arrived_at);
                IngestPacketOutcome::OwesKeepaliveEcho { link_id }
            }
            (LinkRole::Initiator { .. } | LinkRole::Responder { .. }, KEEPALIVE_ECHO) => {
                self.links.note_inbound(&link_id, arrived_at);
                IngestPacketOutcome::Ignored
            }
            _ => IngestPacketOutcome::Ignored,
        }
    }

    fn classify_link_close(&mut self, data: DataPacket<'_>) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let (key, attached_interface) = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                key,
                attached_interface,
                ..
            }) => (key, Some(*attached_interface)),
            Some(LinkPhase::Handshake { key, .. }) => (key, None),
            Some(LinkPhase::Pending { .. }) | None => return IngestPacketOutcome::Ignored,
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        if plaintext != link_id.as_bytes() {
            return IngestPacketOutcome::Ignored;
        }
        self.links.remove(&link_id);
        self.channels.close(&link_id);
        self.incoming_assemblies.clear(&link_id);
        self.outgoing_assemblies.clear(&link_id);
        if let Some(interface) = attached_interface {
            self.mark_interface_dirty(interface);
        }
        IngestPacketOutcome::LinkClosedByPeer { link_id }
    }

    fn classify_link_request(
        &mut self,
        header: &WirePacketHeader,
        payload: &[u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        view: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'static> {
        if header.destination_type != DestinationType::Single {
            return IngestPacketOutcome::Ignored;
        }
        let Ok(request) = link_request_from(header, payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(registered) = self
            .upstream_app_destinations
            .lookup(&request.destination, DestinationType::Single)
        else {
            return self.classify_transported_link_request(
                header,
                &request,
                received_hops,
                source_interface,
                arrived_at,
                view,
            );
        };
        let UpstreamAppDestinationKind::Single {
            identity,
            proof_strategy,
            ..
        } = registered.kind
        else {
            return IngestPacketOutcome::Ignored;
        };
        if self.held_identities.get(&identity).is_none() {
            return IngestPacketOutcome::Ignored;
        }

        let packet_hash = PacketHash::of_fields(
            DestinationType::Single,
            PacketType::LinkRequest,
            &request.destination,
            header.context,
            payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        IngestPacketOutcome::OwesLinkProof {
            request,
            identity,
            proof_strategy,
            received_hops,
            arrived_at,
        }
    }

    fn maybe_upstream_delivery<'p>(
        &mut self,
        data: DataPacket<'p>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> Option<(Delivery<'p>, ProofObligation)> {
        if let Some(transport_id) = data.maybe_transport_id {
            if self.transport_id != Some(transport_id) {
                return None;
            }
        }

        match data.destination_type {
            DestinationType::Plain => {
                if received_hops > PLAIN_DATA_MAX_RECEIVED_HOPS {
                    return None;
                }
                self.upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Plain)?;
                Some((
                    Delivery::Plain(PlainDelivery {
                        destination: data.destination,
                        context: data.context,
                        payload: data.payload,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                ))
            }
            DestinationType::Single => {
                let registered = self
                    .upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Single)?;
                let UpstreamAppDestinationKind::Single {
                    identity,
                    proof_strategy,
                    ..
                } = registered.kind
                else {
                    return None;
                };
                let held = self.held_identities.get(&identity)?;

                let packet_hash = PacketHash::of_data_fields(
                    DestinationType::Single,
                    &data.destination,
                    data.context,
                    data.payload,
                );
                match self.packet_hash_history.remember(packet_hash) {
                    RememberPacketOutcome::AlreadyKnown => return None,
                    RememberPacketOutcome::StoredFresh
                    | RememberPacketOutcome::StoredAfterRotation => {}
                }

                let ratchet_secrets = self.self_ratchets.secrets_newest_first(&data.destination);
                let plaintext = held
                    .decrypt_in_place_with_ratchets(ratchet_secrets, data.payload)
                    .ok()?;

                let proof = match proof_strategy {
                    ProofStrategy::ProveAll => ProofObligation::Owed(ProofOwed {
                        packet_hash,
                        identity,
                    }),
                    ProofStrategy::ProveNone => ProofObligation::None,
                    ProofStrategy::ProveIf => ProofObligation::OwedIfApp(ProofOwed {
                        packet_hash,
                        identity,
                    }),
                };
                Some((
                    Delivery::Single(SingleDelivery {
                        destination: data.destination,
                        context: data.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    proof,
                ))
            }
            DestinationType::Group => {
                self.upstream_app_destinations
                    .lookup(&data.destination, DestinationType::Group)?;

                let packet_hash = PacketHash::of_data_fields(
                    DestinationType::Group,
                    &data.destination,
                    data.context,
                    data.payload,
                );
                match self.packet_hash_history.remember(packet_hash) {
                    RememberPacketOutcome::AlreadyKnown => return None,
                    RememberPacketOutcome::StoredFresh
                    | RememberPacketOutcome::StoredAfterRotation => {}
                }

                let key = self.group_keys.key_for(&data.destination)?;
                let token_key = TokenKey::from_derived(key).ok()?;
                let plaintext = token_open_in_place(&token_key, data.payload).ok()?;
                Some((
                    Delivery::Group(GroupDelivery {
                        destination: data.destination,
                        context: data.context,
                        plaintext,
                        arrived_at,
                        source_interface,
                    }),
                    ProofObligation::None,
                ))
            }
            DestinationType::Link => None,
        }
    }

    fn ingest_tunnel_synthesize<'p>(
        &mut self,
        data: &DataPacket<'_>,
        source_interface: InterfaceId,
        now: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let Some(verified) = parse_synthesize_payload(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let expires = InstantMillis(now.0.saturating_add(TUNNEL_TIMEOUT_MS));
        match self
            .tunnels
            .observe_synthesize(verified.tunnel_id, source_interface, expires)
        {
            TunnelTransition::Established | TunnelTransition::Refreshed => {}
            TunnelTransition::Reappeared { previous_interface } => {
                self.routing_table
                    .repoint_routes(previous_interface, source_interface, now);
                self.mark_interface_dirty(previous_interface);
                self.mark_interface_dirty(source_interface);
            }
        }
        IngestPacketOutcome::TunnelObserved { expires }
    }

    fn ingest_path_request<'p>(
        &mut self,
        data: &DataPacket<'_>,
        source_interface: InterfaceId,
        now: InstantMillis,
        view: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'p> {
        let Ok(request) = PathRequest::parse(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };

        // A request we have already seen (same destination and id) is a loop or
        // a re-arrival — drop it before answering or forwarding again.
        if self
            .seen_path_requests
            .observe(request.destination, request.id)
            == PathRequestNovelty::Duplicate
        {
            return IngestPacketOutcome::Ignored;
        }

        if self
            .upstream_app_destinations
            .lookup(&request.destination, DestinationType::Single)
            .is_some()
        {
            return IngestPacketOutcome::AnswerPathRequest {
                destination: request.destination,
            };
        }

        let held_route = self.transport_id.and(
            self.routing_table
                .forwarding_route_for(&request.destination),
        );
        let Some(route) = held_route else {
            // No route held. A shared instance still relays the request so its
            // local clients take part: a client's own request fans out to the wider
            // network, a network request is offered to the local clients that might
            // own the destination, and a discover-eligible transport interface keeps
            // its recursive discovery. Otherwise we stay silent.
            if self.transport_id.is_none() {
                return IngestPacketOutcome::Ignored;
            }
            let from_local_client = source_interface.kind() == Some(InterfaceKind::LocalClient);
            let discovers = iface_config(view, source_interface)
                .is_some_and(|config| config.mode.discovers_unknown_paths());
            if discovers
                && self
                    .interface_path_request_limits
                    .record_and_should_limit(source_interface, now)
            {
                return IngestPacketOutcome::Ignored;
            }
            let has_local_client = view
                .iter()
                .any(|config| config.id.kind() == Some(InterfaceKind::LocalClient));
            let outcome = if from_local_client || discovers {
                IngestPacketOutcome::ForwardPathRequestForDiscovery {
                    destination: request.destination,
                    id: request.id,
                }
            } else if has_local_client {
                IngestPacketOutcome::RelayPathRequestToLocalClients {
                    destination: request.destination,
                    id: request.id,
                }
            } else {
                return IngestPacketOutcome::Ignored;
            };
            let expires_at = InstantMillis(now.0.saturating_add(DISCOVERY_PATH_REQUEST_TIMEOUT_MS));
            match self.discovery_path_requests.begin(
                request.destination,
                source_interface,
                expires_at,
            ) {
                DiscoveryOutcome::AlreadyInFlight => return IngestPacketOutcome::Ignored,
                DiscoveryOutcome::Opened => {}
            }
            return outcome;
        };

        if request_echoes_into_its_own_roaming_segment(
            route.receiving_interface,
            source_interface,
            view,
        ) {
            return IngestPacketOutcome::Ignored;
        }

        // A held route whose next hop is the requester itself is suppressed, not
        // discovered onward — we already know the way, it just loops back.
        if request.loops_back_through_requester(route.next_hop) {
            return IngestPacketOutcome::Ignored;
        }

        if self.routing_table.responsiveness_of(&request.destination)
            == Some(RouteResponsiveness::Unresponsive)
        {
            return IngestPacketOutcome::Ignored;
        }

        let due_at = InstantMillis(now.0 + path_response_grace_ms(source_interface, view));
        self.scheduled_announces.schedule_directed(
            request.destination,
            due_at,
            source_interface,
            route.hops.0,
        );
        IngestPacketOutcome::ScheduledPathResponse {
            destination: request.destination,
        }
    }

    /// True when the live forwarding route for `destination` leaves over a
    /// [`InterfaceKind::LocalClient`] interface: RNS Transport.py's
    /// `for_local_client`, an app sharing our instance for whom we carry traffic
    /// inward regardless of whether the arriving packet was addressed through us.
    fn routes_via_local_client(&self, destination: &DestinationHash) -> bool {
        self.routing_table
            .forwarding_route_for(destination)
            .is_some_and(|route| {
                route.receiving_interface.kind() == Some(InterfaceKind::LocalClient)
            })
    }

    /// RNS 1.3.1 Transport.py:1556-1580: a transport-addressed packet rides the
    /// path table onward. It's re-addressed at the next relay while more than one
    /// hop remains, stripped back to a plain broadcast for the final hop. It also
    /// leaves a reverse-table row so its proof can ride home.
    fn maybe_forward<'p>(
        &mut self,
        header: WirePacketHeader,
        payload: &'p mut [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> Option<PacketToForward<'p>> {
        if header.destination_type != DestinationType::Single
            || header.packet_type != PacketType::Data
        {
            return None;
        }
        let route = self
            .routing_table
            .forwarding_route_for(&header.destination)?;

        let packet_hash = PacketHash::of_data_fields(
            header.destination_type,
            &header.destination,
            header.context,
            payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return None,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        let forwarded_header = if route.hops.0 > 1 {
            let NextHop::Via(next) = route.next_hop else {
                return None;
            };
            WirePacketHeader {
                hops: received_hops,
                transport_id: Some(next),
                ..header
            }
        } else {
            WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: header.destination_type,
                packet_type: header.packet_type,
                hops: received_hops,
                transport_id: None,
                destination: header.destination,
                context: header.context,
            }
        };

        self.reverse_routes.remember(
            ReverseRouteEntry {
                proof_destination: packet_hash.proof_destination(),
                received_interface: source_interface,
                outbound_interface: route.receiving_interface,
                expires_at: InstantMillis(
                    arrived_at
                        .0
                        .saturating_add(DEFAULT_REVERSE_ROUTE_TIMEOUT_MS),
                ),
            },
            arrived_at,
        );
        self.routing_table
            .note_relayed(&header.destination, arrived_at);

        Some(PacketToForward {
            header: forwarded_header,
            payload,
            fire_on: route.receiving_interface,
        })
    }

    /// Off (false) when the interface sets no target, which is the reference default (RNS Transport.py:1836).
    fn announce_rate_blocks_rebroadcast(
        &mut self,
        source_interface: InterfaceId,
        destination: DestinationHash,
        now: InstantMillis,
        interfaces: &[InterfaceConfig],
    ) -> bool {
        let Some(limit) = interfaces
            .iter()
            .find(|descriptor| descriptor.id == source_interface)
            .and_then(|descriptor| descriptor.announce_rate_limit)
        else {
            return false;
        };
        self.announce_rates.observe(destination, now, limit) == AnnounceRateVerdict::Blocked
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ingest_announce(
        &mut self,
        announce: Announce<'_>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        is_path_response: bool,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> AnnounceIngest {
        if self.transport_id.is_some() {
            self.scheduled_announces.absorb_echo(
                &announce.destination,
                received_hops,
                arrived_at,
                MAX_ANNOUNCE_REBROADCASTS,
            );
        }

        let decision = AnnounceAcceptanceInput {
            packet_hops: received_hops,
            announce_id: announce.announce_id,
            destination_is_self_or_upstream: self
                .upstream_app_destinations
                .lookup(&announce.destination, DestinationType::Single)
                .is_some(),
            existing_route: self
                .routing_table
                .existing_route_for(&announce.destination, interfaces),
            arrived_at,
        }
        .determine_acceptance();

        if !matches!(decision, AnnounceAcceptanceDecision::Accept(_)) {
            return AnnounceIngest::Ignored;
        }

        let previous_interface = self
            .routing_table
            .path_row(&announce.destination)
            .map(|entry| entry.receiving_interface);
        let dirty = &mut self.dirty_interfaces;
        let outcome = self.routing_table.upsert_route_with_tunnels(
            received_hops,
            arrived_at,
            source_interface,
            interfaces,
            &self.tunnels,
            next_hop,
            &announce,
            &mut |removed| {
                dirty.mark(removed.receiving_interface);
                on_removed(removed);
            },
        );
        match outcome {
            UpsertRouteOutcome::Inserted | UpsertRouteOutcome::Updated => {
                self.mark_interface_dirty(source_interface);
                if let Some(previous) = previous_interface {
                    self.mark_interface_dirty(previous);
                }
                // An announce that answers a discovery we forwarded on a stranger's
                // behalf is steered straight back to the interface that asked. A path
                // response is otherwise terminal at us, so without this the answer the
                // stranger is waiting for would never reach them.
                let discovery_answer = self.discovery_path_requests.take(&announce.destination);
                let rebroadcast = if is_path_response {
                    if let Some(requesting_interface) = discovery_answer {
                        self.scheduled_announces.schedule_directed(
                            announce.destination,
                            arrived_at,
                            requesting_interface,
                            received_hops,
                        );
                        RebroadcastDecision::Scheduled
                    } else {
                        RebroadcastDecision::TerminalPathResponse
                    }
                } else if self.transport_id.is_none() {
                    RebroadcastDecision::NotATransportNode
                } else if !interfaces
                    .iter()
                    .any(|descriptor| descriptor.capabilities.allows_transport())
                {
                    RebroadcastDecision::NoTransportInterfaces
                } else if self.announce_rate_blocks_rebroadcast(
                    source_interface,
                    announce.destination,
                    arrived_at,
                    interfaces,
                ) {
                    RebroadcastDecision::RateBlocked
                } else {
                    let offset = jitter_offset_for(
                        jitter,
                        &announce.destination,
                        DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
                    );
                    self.scheduled_announces.schedule(
                        announce.destination,
                        InstantMillis(arrived_at.0.saturating_add(offset)),
                        source_interface,
                        received_hops,
                    );
                    RebroadcastDecision::Scheduled
                };
                AnnounceIngest::Accepted(AcceptedAnnounce {
                    destination: announce.destination,
                    hops: received_hops,
                    rebroadcast,
                })
            }
            UpsertRouteOutcome::Dropped(
                DropCause::PayloadArenaFull | DropCause::RoutingTableFull,
            ) => AnnounceIngest::Ignored,
        }
    }
}

fn iface_config(view: &[InterfaceConfig], id: InterfaceId) -> Option<&InterfaceConfig> {
    view.iter().find(|config| config.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        ContextFlag, DestinationHash, IfacFlag, PropagationType, TransportId, WireContext,
        WirePacketHeader, HEADER_MIN_LEN,
    };

    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_local_client_transit_is_discounted_one_hop() {
        let local_client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app-1");
        let tcp = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"1.2.3.4:4242");
        assert_eq!(local_adjusted_hops(5, local_client), 4);
        assert_eq!(local_adjusted_hops(5, tcp), 5);
        assert_eq!(local_adjusted_hops(0, local_client), 0);
    }

    fn header_bytes(packet_type: PacketType) -> [u8; HEADER_MIN_LEN] {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new([0xA5; 16]),
            context: WireContext::None,
        };
        let mut bytes = [0u8; HEADER_MIN_LEN];
        assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
        bytes
    }

    #[test]
    fn path_request_parse_names_its_two_non_answering_cases() {
        let dest = [0x11; TRUNCATED_HASH_BYTE_LEN];
        let id = [0x55; TRUNCATED_HASH_BYTE_LEN];
        let tid = [0x7a; TRUNCATED_HASH_BYTE_LEN];

        assert_eq!(
            PathRequest::parse(&dest[..8]).err(),
            Some(PathRequestError::NoDestination),
        );
        assert_eq!(
            PathRequest::parse(&dest).err(),
            Some(PathRequestError::NoId)
        );

        let leaf = [&dest[..], &id[..]].concat();
        let parsed = PathRequest::parse(&leaf).unwrap();
        assert_eq!(parsed.requester_transport_id, None);
        assert_eq!(parsed.id, id);

        let transport = [&dest[..], &tid[..], &id[..]].concat();
        let parsed = PathRequest::parse(&transport).unwrap();
        assert_eq!(parsed.requester_transport_id, TransportId::from_slice(&tid));
        assert_eq!(parsed.id, id);
    }

    #[test]
    fn malformed_headers_are_unparseable() {
        let packet = InboundPacket {
            arrived_at: InstantMillis(7),
            source_interface: iface(0x01),
            bytes: &mut [0x01],
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
    }

    #[test]
    fn recognized_non_announce_packets_classify_from_the_header() {
        for packet_type in [PacketType::Data, PacketType::LinkRequest, PacketType::Proof] {
            let mut bytes = header_bytes(packet_type);
            let packet = InboundPacket {
                arrived_at: InstantMillis(9),
                source_interface: iface(0x02),
                bytes: &mut bytes,
            };

            let classified = Ingress::classify(packet);
            match packet_type {
                PacketType::Data => assert!(matches!(classified, Ingress::Data { .. })),
                PacketType::LinkRequest => {
                    assert!(matches!(classified, Ingress::LinkRequest { .. }))
                }
                PacketType::Proof => assert!(matches!(classified, Ingress::Proof { .. })),
                PacketType::Announce => unreachable!(),
            }
        }
    }

    #[test]
    fn data_packets_carry_their_typed_fields_through_classification() {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Transport,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 5,
            transport_id: Some(TransportId::new([0x11; 16])),
            destination: DestinationHash::new([0xA5; 16]),
            context: WireContext::Resource,
        };
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut expected_payload = payload;
        let mut bytes = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut bytes).unwrap();
        bytes[header_len..header_len + payload.len()].copy_from_slice(&payload);

        let packet = InboundPacket {
            arrived_at: InstantMillis(21),
            source_interface: iface(0x05),
            bytes: &mut bytes[..header_len + payload.len()],
        };

        let Ingress::Data {
            data,
            header: _,
            received_hops,
            source_interface,
            arrived_at,
        } = Ingress::classify(packet)
        else {
            panic!("a data packet should classify as data");
        };
        assert_eq!(
            data,
            DataPacket {
                destination_type: DestinationType::Plain,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::Resource,
                maybe_transport_id: Some(TransportId::new([0x11; 16])),
                payload: &mut expected_payload,
            }
        );
        assert_eq!(received_hops, 6);
        assert_eq!(source_interface, iface(0x05));
        assert_eq!(arrived_at, InstantMillis(21));
    }

    #[test]
    fn data_packets_classify_for_every_destination_type() {
        for destination_type in [
            DestinationType::Single,
            DestinationType::Group,
            DestinationType::Plain,
            DestinationType::Link,
        ] {
            let header = WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type,
                packet_type: PacketType::Data,
                hops: 0,
                transport_id: None,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::None,
            };
            let mut bytes = [0u8; HEADER_MIN_LEN];
            assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
            let packet = InboundPacket {
                arrived_at: InstantMillis(23),
                source_interface: iface(0x06),
                bytes: &mut bytes,
            };

            let Ingress::Data { data, .. } = Ingress::classify(packet) else {
                panic!("data packets to any destination type classify as data");
            };
            assert_eq!(data.destination_type, destination_type);
            assert!(data.payload.is_empty());
        }
    }

    #[test]
    fn announce_packets_must_target_a_single_destination() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[0] |= (DestinationType::Group as u8) << 2;
        let packet = InboundPacket {
            arrived_at: InstantMillis(11),
            source_interface: iface(0x03),
            bytes: &mut raw,
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
    }

    #[test]
    fn announce_received_hops_saturates_at_wire_max() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[1] = u8::MAX;
        let source_interface = iface(0x04);
        let arrived_at = InstantMillis(13);
        let packet = InboundPacket {
            arrived_at,
            source_interface,
            bytes: &mut raw,
        };

        let classified = Ingress::classify(packet);
        let Ingress::Announce {
            received_hops,
            source_interface: classified_source,
            arrived_at: classified_arrival,
            ..
        } = classified
        else {
            panic!("valid announce should classify as announce");
        };
        assert_eq!(received_hops, u8::MAX);
        assert_eq!(classified_source, source_interface);
        assert_eq!(classified_arrival, arrived_at);
    }

    use crate::engine::test_support::*;

    use crate::engine::{
        AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, EngineState,
        IngestPacketOutcome, RatchetEntropy, RatchetPolicy,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IdentitySigner;
    use crate::routing::announce::derive_destination_hash;
    use crate::routing::delivery::{Delivery, PlainDelivery, SingleDelivery};
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::storage::TestFixedStorage;

    #[test]
    fn a_path_request_for_a_local_destination_owes_an_answer() {
        let mut state = personal_node_announcer();
        let local = personal_node_destination();

        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(local, None, &[0x55; 16], &mut buf)
            .unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::AnswerPathRequest { destination: local },
        );
    }

    #[test]
    fn a_leaf_ignores_a_path_request_for_a_stranger() {
        // A non-transport node with no route to the destination has nothing to
        // answer and nothing to forward.
        let mut leaf: EngineState<Cap> = EngineState::<Cap>::default();
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(
            DestinationHash::new([0x44; 16]),
            None,
            &[0x55; 16],
            &mut buf,
        )
        .unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            leaf.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn write_path_response_announce_emits_a_path_response_a_peer_learns_as_a_route() {
        use crate::engine::PathResponseWriteOutcome;
        use crate::routing::announce::Announce;

        // B answers for its own destination with a PATH_RESPONSE announce.
        let mut b = personal_node_announcer();
        let local = personal_node_destination();
        let mut buf = [0u8; BROADCAST_MTU];
        let PathResponseWriteOutcome::Written { wire_len } = b.write_path_response_announce(
            &local,
            InstantMillis(500),
            TEST_ANNOUNCE_ENTROPY,
            &mut buf,
        ) else {
            panic!("a local destination is answerable");
        };

        let (header, payload) = WirePacketHeader::parse(&buf[..wire_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.context, WireContext::PathResponse);
        assert_eq!(header.destination, local);
        assert_eq!(
            Announce::from_wire(&header, payload).unwrap().destination,
            local
        );

        // A fresh peer accepts it as an ordinary announce — a learned route.
        let mut a: EngineState<Cap> = EngineState::<Cap>::default();
        let mut wire = buf[..wire_len].to_vec();
        assert!(matches!(
            a.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_200),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        assert_eq!(a.route_count(), 1);
    }

    #[test]
    fn a_path_response_for_a_destination_we_do_not_hold_is_refused() {
        use crate::engine::PathResponseWriteOutcome;
        let mut b = personal_node_announcer();
        let mut buf = [0u8; BROADCAST_MTU];
        assert!(matches!(
            b.write_path_response_announce(
                &DestinationHash::new([0x44; 16]),
                InstantMillis(500),
                TEST_ANNOUNCE_ENTROPY,
                &mut buf,
            ),
            PathResponseWriteOutcome::NotLocal,
        ));
    }

    fn relay_holding_a_cached_route() -> (EngineState<Cap>, DestinationHash) {
        let cached =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());
        let mut relay = transporting_node();
        let mut announce = hx(RAW_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xB2),
                    bytes: &mut announce,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        (relay, cached)
    }

    fn discovering_descriptor(id: InterfaceId, mode: InterfaceMode) -> InterfaceConfig {
        InterfaceConfig {
            mode,
            ..routable_descriptor(id)
        }
    }

    fn stranger_path_request(id: [u8; 16]) -> std::vec::Vec<u8> {
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::write_path_request_wire_packet(
            DestinationHash::new([0x44; 16]),
            None,
            &id,
            &mut buf,
        )
        .unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn a_transport_node_on_a_gateway_interface_forwards_an_unknown_path_request() {
        let stranger = DestinationHash::new([0x44; 16]);
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let view = [discovering_descriptor(source, InterfaceMode::Gateway)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::ForwardPathRequestForDiscovery {
                destination: stranger,
                id: [0x55; 16],
            },
        );
        // The forward is remembered, so a fresh discovery for the same stranger
        // cannot be opened while the first is still in flight.
        assert_eq!(
            relay
                .discovery_path_requests
                .begin(stranger, source, InstantMillis(2_000)),
            DiscoveryOutcome::AlreadyInFlight
        );
    }

    #[test]
    fn a_flooded_discover_interface_stops_forwarding_path_requests() {
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let view = [discovering_descriptor(source, InterfaceMode::Gateway)];
        let now = InstantMillis(1_000);

        let mut forwarded = 0;
        let mut dropped_after_forwarding = false;
        for dest_byte in 1..=8u8 {
            let mut buf = [0u8; BROADCAST_MTU];
            let n = crate::engine::write_path_request_wire_packet(
                DestinationHash::new([dest_byte; 16]),
                None,
                &[dest_byte; 16],
                &mut buf,
            )
            .unwrap();
            let mut wire = buf[..n].to_vec();
            match relay.ingest_packet(
                InboundPacket {
                    arrived_at: now,
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ) {
                IngestPacketOutcome::ForwardPathRequestForDiscovery { .. } => forwarded += 1,
                IngestPacketOutcome::Ignored if forwarded > 0 => dropped_after_forwarding = true,
                _ => {}
            }
        }

        assert!(
            forwarded >= 1,
            "the first unknown-destination requests are forwarded"
        );
        assert!(
            dropped_after_forwarding,
            "once the interface floods unknown-destination requests, the recursive forward is dropped",
        );
    }

    #[test]
    fn a_second_discovery_for_the_same_stranger_is_not_forwarded_again() {
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let view = [discovering_descriptor(source, InterfaceMode::Gateway)];

        let mut first = stranger_path_request([0x55; 16]);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::ForwardPathRequestForDiscovery { .. },
        ));

        // A second request carrying a different tag clears the tag-dedup but is
        // still suppressed by the per-destination discovery already in flight.
        let mut second = stranger_path_request([0x66; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_transport_node_does_not_discover_on_a_full_mode_interface() {
        // DISCOVER_PATHS_FOR is access-point/gateway/roaming only.
        let source = iface(0xA1);
        let mut relay = transporting_node();
        let view = [routable_descriptor(source)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_local_clients_unknown_path_request_fans_out_to_the_network() {
        let stranger = DestinationHash::new([0x44; 16]);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let uplink = iface(0xB2);
        let mut relay = transporting_node();
        let view = [routable_descriptor(app), routable_descriptor(uplink)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: app,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::ForwardPathRequestForDiscovery {
                destination: stranger,
                id: [0x55; 16],
            },
            "a local client's request for an unheard destination fans out so the network can answer",
        );
        assert_eq!(
            relay
                .discovery_path_requests
                .begin(stranger, app, InstantMillis(2_000)),
            DiscoveryOutcome::AlreadyInFlight,
            "the asking client is remembered so the answer is steered back to it",
        );
    }

    #[test]
    fn a_network_request_for_an_unheld_destination_is_offered_to_local_clients_only() {
        let stranger = DestinationHash::new([0x44; 16]);
        let uplink = iface(0xA1);
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let mut relay = transporting_node();
        let view = [routable_descriptor(uplink), routable_descriptor(app)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: uplink,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::RelayPathRequestToLocalClients {
                destination: stranger,
                id: [0x55; 16],
            },
            "a network request a plain shared instance can't answer is offered to its apps",
        );
        assert_eq!(
            relay
                .discovery_path_requests
                .begin(stranger, uplink, InstantMillis(2_000)),
            DiscoveryOutcome::AlreadyInFlight,
        );
    }

    #[test]
    fn a_full_mode_request_with_no_local_clients_is_still_ignored() {
        let uplink = iface(0xA1);
        let other = iface(0xB2);
        let mut relay = transporting_node();
        let view = [routable_descriptor(uplink), routable_descriptor(other)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: uplink,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::Ignored,
            "with no apps sharing the instance, an unanswerable full-mode request stays silent",
        );
    }

    #[test]
    fn a_leaf_does_not_discover_even_on_a_gateway_interface() {
        // Recursive discovery is a transport-node behavior; a leaf stays silent.
        let source = iface(0xA1);
        let mut leaf: EngineState<Cap> = EngineState::<Cap>::default();
        let view = [discovering_descriptor(source, InterfaceMode::AccessPoint)];

        let mut wire = stranger_path_request([0x55; 16]);
        assert_eq!(
            leaf.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_answering_path_response_is_steered_back_to_the_interface_that_asked() {
        use crate::engine::{Directive, EngineReaction, PathResponseWriteOutcome};

        // B answers for its own destination with a PATH_RESPONSE announce.
        let mut b = personal_node_announcer();
        let local = personal_node_destination();
        let mut buf = [0u8; BROADCAST_MTU];
        let PathResponseWriteOutcome::Written { wire_len } = b.write_path_response_announce(
            &local,
            InstantMillis(500),
            TEST_ANNOUNCE_ENTROPY,
            &mut buf,
        ) else {
            panic!("a local destination is answerable");
        };

        // A forwarded a discovery for `local` on behalf of interface 0xA1.
        let requester = iface(0xA1);
        let mut a = transporting_node();
        assert_eq!(
            a.discovery_path_requests
                .begin(local, requester, InstantMillis(60_000)),
            DiscoveryOutcome::Opened
        );

        // The answer arrives from elsewhere; A accepts the route.
        let mut wire = buf[..wire_len].to_vec();
        assert!(matches!(
            a.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_200),
                    source_interface: iface(0xB2),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));

        // A steers a directed answer to 0xA1 alone, not the flood fan-out.
        let view = [
            routable_descriptor(requester),
            routable_descriptor(iface(0xB2)),
        ];
        let mut targets = std::vec::Vec::new();
        a.fire_due_scheduled_announces(InstantMillis(1_200), &view, &mut |reaction| {
            if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction {
                targets.push(target);
            }
        });
        assert_eq!(targets, std::vec![requester]);
    }

    fn path_request_wire(destination: DestinationHash) -> std::vec::Vec<u8> {
        let mut buf = [0u8; BROADCAST_MTU];
        let n =
            crate::engine::write_path_request_wire_packet(destination, None, &[0x55; 16], &mut buf)
                .unwrap();
        buf[..n].to_vec()
    }

    fn path_request_wire_with(body: &[u8]) -> std::vec::Vec<u8> {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: PATH_REQUEST_DESTINATION,
            context: WireContext::None,
        };
        let mut wire = std::vec![0u8; HEADER_MIN_LEN];
        header.write(&mut wire).unwrap();
        wire.extend_from_slice(body);
        wire
    }

    #[test]
    fn a_transport_form_request_answers_and_dedups_on_the_id_not_the_transport_id() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let transport_id = [0x7a; 16];
        let id = [0x55; 16];
        let mut body = std::vec::Vec::new();
        body.extend_from_slice(cached.as_bytes());
        body.extend_from_slice(&transport_id);
        body.extend_from_slice(&id);

        let mut wire = path_request_wire_with(&body);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "the 48-byte transport form is parsed and answered",
        );

        let mut same_id_other_transport = std::vec::Vec::new();
        same_id_other_transport.extend_from_slice(cached.as_bytes());
        same_id_other_transport.extend_from_slice(&[0xCC; 16]);
        same_id_other_transport.extend_from_slice(&id);
        let mut wire = path_request_wire_with(&same_id_other_transport);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "a different transport id but the same id is the same request — deduped",
        );
    }

    #[test]
    fn an_unresponsive_route_is_withheld_then_vouched_for_again_once_it_recovers() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let transport_id = [0x7a; 16];

        relay
            .routing_table
            .mark_responsiveness(&cached, RouteResponsiveness::Unresponsive);

        let mut withheld = std::vec::Vec::new();
        withheld.extend_from_slice(cached.as_bytes());
        withheld.extend_from_slice(&transport_id);
        withheld.extend_from_slice(&[0x11; 16]);
        let mut wire = path_request_wire_with(&withheld);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "an unresponsive route is withheld so a node with a live path answers instead",
        );

        relay
            .routing_table
            .mark_responsiveness(&cached, RouteResponsiveness::Responsive);

        let mut recovered = std::vec::Vec::new();
        recovered.extend_from_slice(cached.as_bytes());
        recovered.extend_from_slice(&transport_id);
        recovered.extend_from_slice(&[0x22; 16]);
        let mut wire = path_request_wire_with(&recovered);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "once marked responsive again, we vouch for the route once more",
        );
    }

    #[test]
    fn a_request_whose_requester_is_our_next_hop_is_declined() {
        let cached =
            DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap());
        let mut relay = transporting_node();
        let mut announce = hx(RNS_1_3_1_RETRANSMITTED_ANNOUNCE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let request = |requester: [u8; 16], id: u8| {
            let mut body = std::vec::Vec::new();
            body.extend_from_slice(cached.as_bytes());
            body.extend_from_slice(&requester);
            body.extend_from_slice(&[id; 16]);
            path_request_wire_with(&body)
        };

        let mut loops_back = request([0x7a; 16], 0x01);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut loops_back,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "the requester is the via we'd route through — answering would loop",
        );

        let mut other_requester = request([0xCC; 16], 0x02);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xA1),
                    bytes: &mut other_requester,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "a different requester gets the cached path",
        );
    }

    #[test]
    fn an_idless_path_request_is_ignored() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire_with(cached.as_bytes());
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "a bare destination carries no id — the reference ignores it",
        );
    }

    #[test]
    fn a_transport_node_answers_a_path_request_from_its_cache() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
        );

        let scheduled = relay.scheduled_announces.iter().next().unwrap();
        assert_eq!(scheduled.destination, cached);
        assert_eq!(
            scheduled.due_at,
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS),
            "the cache answer waits out the grace before firing",
        );
        assert_eq!(
            scheduled.directed_to,
            Some(iface(0xA1)),
            "it is directed at the requester, not flooded",
        );
    }

    #[test]
    fn a_roaming_requester_earns_the_extra_grace() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let requester = iface(0xA1);
        let roaming_view = [InterfaceConfig {
            mode: InterfaceMode::Roaming,
            ..routable_descriptor(requester)
        }];
        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: requester,
                bytes: &mut wire,
            },
            TEST_ENTROPY,
            &roaming_view,
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().due_at,
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS + PATH_REQUEST_ROAMING_GRACE_MS),
        );
    }

    #[test]
    fn a_path_request_on_the_roaming_interface_the_route_lives_on_is_not_answered() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let learned_on = iface(0xB2);
        let roaming_view = [discovering_descriptor(learned_on, InterfaceMode::Roaming)];
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: learned_on,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &roaming_view,
            ),
            IngestPacketOutcome::Ignored,
            "a roaming interface does not answer for a route that lives on it",
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            None,
            "the suppressed request scheduled no directed answer; the flood rebroadcast stands",
        );
    }

    #[test]
    fn the_same_interface_answers_when_it_is_not_in_roaming_mode() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let learned_on = iface(0xB2);
        let full_view = [routable_descriptor(learned_on)];
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: learned_on,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &full_view,
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
            "the same-interface suppression is roaming-only; a Full interface still answers",
        );
    }

    #[test]
    fn a_flood_schedule_supersedes_a_directed_answer_for_the_same_destination() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut wire,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            Some(iface(0xA1)),
        );

        relay
            .scheduled_announces
            .schedule(cached, InstantMillis(1_100), iface(0xEE), 2);
        assert_eq!(relay.scheduled_announces.scheduled_count(), 1);
        assert_eq!(
            relay.scheduled_announces.iter().next().unwrap().directed_to,
            None,
            "a fresher announce reclaims the entry as a flood — the grace answer is cancelled",
        );
    }

    #[test]
    fn the_cache_answer_fires_to_the_requester_only_after_the_grace_deadline() {
        use crate::engine::{Directive, EngineReaction};
        use crate::wire::{PropagationType, WireContext};

        let (mut relay, cached) = relay_holding_a_cached_route();
        let requester = iface(0xA1);
        let view = [
            routable_descriptor(requester),
            routable_descriptor(iface(0xEE)),
        ];

        let mut wire = path_request_wire(cached);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: requester,
                bytes: &mut wire,
            },
            TEST_ENTROPY,
            &view,
        );

        let mut early = std::vec::Vec::new();
        relay.fire_due_scheduled_announces(
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS - 1),
            &view,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    early.push(target);
                }
            },
        );
        assert!(early.is_empty(), "nothing fires before the grace deadline");

        let mut fired = std::vec::Vec::new();
        relay.fire_due_scheduled_announces(
            InstantMillis(1_000 + PATH_REQUEST_GRACE_MS),
            &view,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    fired.push((target, bytes.to_vec()));
                }
            },
        );
        assert_eq!(fired.len(), 1, "exactly one answer, to the one requester");
        assert_eq!(fired[0].0, requester);
        let (header, _) = WirePacketHeader::parse(&fired[0].1).unwrap();
        assert_eq!(header.destination, cached);
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(
            header.propagation,
            PropagationType::Transport,
            "a transport retransmission of the cached announce, directed at the asker",
        );
        assert_eq!(
            header.context,
            WireContext::PathResponse,
            "the directed answer is tagged PATH_RESPONSE so the requester takes it \
             as a terminal path response instead of re-flooding it as a fresh announce",
        );
    }

    #[test]
    fn a_leaf_with_a_route_but_no_transport_role_does_not_answer_from_cache() {
        let cached =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());
        let mut leaf: EngineState<Cap> = EngineState::<Cap>::default();
        let mut announce = hx(RAW_ANNOUNCE);
        let _ = leaf.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut wire = path_request_wire(cached);
        assert_eq!(
            leaf.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "without a transport role a node never answers from cache, even holding the route",
        );
    }

    #[test]
    fn a_path_response_is_learned_as_a_route_but_never_rebroadcast() {
        let mut relay = transporting_node();
        let mut response = hx(RAW_ANNOUNCE);
        // Tag the announce as a path response by flipping its context byte.
        response[HEADER_MIN_LEN - 1] = WireContext::PathResponse.to_byte();

        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut response,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::TerminalPathResponse,
            })),
        );
        assert_eq!(relay.route_count(), 1, "the path response is learned");
        assert_eq!(
            relay.scheduled_announce_count(),
            0,
            "a path response is never re-flooded",
        );
    }

    #[test]
    fn the_same_announce_without_the_path_response_tag_is_scheduled() {
        let mut relay = transporting_node();
        let mut announce = hx(RAW_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut announce,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(relay.scheduled_announce_count(), 1);
    }

    #[test]
    fn a_destination_announcing_faster_than_the_interface_target_is_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;
        use crate::routing::announce::AnnounceEntropy;

        // A peer mints two distinct announces for its own destination.
        let mut announcer = personal_node_announcer();
        let destination = personal_node_destination();
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        let mut buf_a = [0u8; BROADCAST_MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                AnnounceEntropy::new([0x11; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                AnnounceEntropy::new([0x22; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        // The receiving interface caps a destination to one announce per 10s.
        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        // First sighting: learned and scheduled to rebroadcast.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(10_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        // A second announce 1s later — far under the 10s target — is learned but
        // its rebroadcast is suppressed.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(11_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::RateBlocked,
                ..
            })),
        ));
        assert_eq!(relay.route_count(), 1, "the route is still learned");
        assert_eq!(
            relay.scheduled_announce_count(),
            1,
            "only the first announce was scheduled to rebroadcast",
        );
    }

    #[test]
    fn a_destination_within_the_interface_target_is_not_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;
        use crate::routing::announce::AnnounceEntropy;

        let mut announcer = personal_node_announcer();
        let destination = personal_node_destination();
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        let mut buf_a = [0u8; BROADCAST_MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                AnnounceEntropy::new([0x11; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; BROADCAST_MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                AnnounceEntropy::new([0x22; AnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(10_000),
                source_interface: source,
                bytes: &mut first,
            },
            TEST_ENTROPY,
            &rate_limited,
        );
        // A second announce a full target window later stays under the limit and
        // is scheduled like any other.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(25_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(
            relay.scheduled_announce_count(),
            1,
            "one pending per destination — the second schedule replaces the first",
        );
    }

    #[test]
    fn a_transport_node_with_no_route_does_not_forward_the_request() {
        // Forwarding an unknown onward is opt-in recursive discovery (off by
        // default), so a relay that holds no route simply ignores the request.
        let mut relay = transporting_node();
        let mut wire = path_request_wire(DestinationHash::new([0x44; 16]));
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_duplicate_path_request_is_not_answered_twice() {
        // Dedup is always on: a relay answers once from cache, and a re-arrival
        // of the same (destination, id) is dropped.
        let (mut relay, cached) = relay_holding_a_cached_route();

        let mut first = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::ScheduledPathResponse {
                destination: cached
            },
        );

        let mut echo = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xB2),
                    bytes: &mut echo,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "the same (destination, id) is a duplicate, not answered again",
        );
    }

    #[test]
    fn ingest_counts_each_packet_without_a_clock() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();

        let mut first_bytes = [1, 2, 3];
        let first = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(10),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut first_bytes,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let mut second_bytes = [4];
        let second = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(20),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut second_bytes,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(first, IngestPacketOutcome::Ignored);
        assert_eq!(second, IngestPacketOutcome::Ignored);
        assert_eq!(state.ingested_packet_count(), 2);
    }
    #[test]
    fn a_single_sealed_for_the_announced_destination_is_delivered() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"hello-announced");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-announced",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_single_sealed_to_the_announced_ratchet_is_delivered() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = personal_node_destination();
        let mut raw = hx(RAW_SEALED_TO_RATCHET);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn an_earlier_announced_ratchet_still_opens_after_rotation() {
        let mut state = ratcheted_personal_node_announcer();
        let interval = 6 * 60 * 60 * 1000;
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(1_000 + interval),
                TEST_ANNOUNCE_ENTROPY,
                RatchetEntropy::new([0x77; RatchetEntropy::LEN]),
                &mut buf,
            )
            .written_len();

        let destination = personal_node_destination();
        let mut raw = hx(RAW_SEALED_TO_RATCHET);
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"ratchet-parity",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_ratcheted_destination_still_opens_identity_keyed_traffic() {
        let mut state = ratcheted_personal_node_announcer();
        let destination = personal_node_destination();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let mut raw = sealed_single_packet(&identity, destination, b"identity-keyed");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"identity-keyed",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";

    #[test]
    fn neighbor_plain_data_for_a_registered_destination_delivers_the_rns_1_3_1_payload() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let destination = state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Plain(PlainDelivery {
                    destination,
                    context: WireContext::None,
                    payload: b"hello-plain",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn relayed_plain_data_is_dropped_at_the_packet_filter() {
        let mut raw = hx(RAW_PLAIN_DATA);
        raw[1] = 1;
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_data_for_an_unregistered_destination_is_not_delivered() {
        let mut raw = hx(RAW_PLAIN_DATA);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["other"])
            .unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn plain_addressed_data_never_reaches_a_single_destination_with_that_hash() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        let single = state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: single,
            context: WireContext::None,
        };
        let mut raw = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut raw).unwrap();
        raw[header_len] = 0xFF;

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw[..header_len + 1]),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn in_transport_data_delivers_only_when_we_are_the_named_transport_instance() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw_for_us = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));
        let mut raw_for_other = hx(&format!(
            "4800{}{}00{}",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Plain(delivered),
            ..
        } = state.ingest_packet(
            plain_data_packet(&mut raw_for_us),
            TEST_ENTROPY,
            &transporting_view(),
        )
        else {
            panic!("in-transport data named to us must deliver plainly");
        };
        assert_eq!(delivered.payload, &[0xEE]);

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw_for_other),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_identity_less_relay_never_accepts_in_transport_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        state
            .register_plain_destination("personal", &["node"])
            .unwrap();

        let mut raw = hx(&format!(
            "4800{}{}00{}",
            "4cd0cc45a7405dbd5cf9b5be1ef92f10", "12f815e3e65add6ceb2fda0e7be33868", "ee"
        ));

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn single_data_decrypts_in_place_and_delivers_the_plaintext() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"hello-single");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_replayed_single_packet_is_ignored_by_the_dedup_history() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut first_copy = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut first_copy),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));

        let mut replayed_copy = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut replayed_copy),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_tampered_single_token_is_ignored_without_poisoning_the_real_packet() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let raw = sealed_single_packet(&identity, destination, b"hello-single");

        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut tampered),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );

        let mut genuine = raw.clone();
        assert!(matches!(
            state.ingest_packet(
                plain_data_packet(&mut genuine),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(_),
                ..
            },
        ));
    }

    #[test]
    fn each_single_destination_decrypts_only_under_its_own_held_identity() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity_a = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let identity_b = InMemoryNodeIdentity::from_secret_key_bytes(&second_secret_key());
        let held_a = state.hold_identity(fixed_secret_key()).unwrap();
        let held_b = state.hold_identity(second_secret_key()).unwrap();
        assert_eq!(held_a, identity_a.identity_hash());
        assert_eq!(held_b, identity_b.identity_hash());

        let dest_a = state
            .register_single_destination(
                &held_a,
                "personal",
                &["a"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let dest_b = state
            .register_single_destination(
                &held_b,
                "personal",
                &["b"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut to_a = sealed_single_packet(&identity_a, dest_a, b"for-a");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_a),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_a,
                    context: WireContext::None,
                    plaintext: b"for-a",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut to_b = sealed_single_packet(&identity_b, dest_b, b"for-b");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut to_b),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination: dest_b,
                    context: WireContext::None,
                    plaintext: b"for-b",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );

        let mut crossed = sealed_single_packet(&identity_b, dest_a, b"crossed");
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut crossed),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_held_app_identity_does_not_answer_transport_addressed_data() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let raw = sealed_single_packet_routed(
            &identity,
            Some(TransportId::new(*held.as_bytes())),
            destination,
            b"hello-single",
        );

        let mut as_app_only = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut as_app_only),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );

        state.set_transport_identity(&held).unwrap();
        let mut as_transport = raw.clone();
        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut as_transport),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"hello-single",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::None,
            },
        );
    }

    #[test]
    fn a_group_delivery_decrypts_with_the_shared_key_byte_for_byte_vs_rns_1_3_1() {
        // Vector minted live against Python RNS 1.3.1: a GROUP destination held
        // by identity 4cd0cc45… under the app name personal.group, carrying the
        // fixed AES-256 key below, encrypting b"group-hello".
        const GROUP_KEY: &str = "42424242424242424242424242424242424242424242424242424242424242422424242424242424242424242424242424242424242424242424242424242424";
        const GROUP_TOKEN: &str = "614e1126ead06d77c97bdb042c1445d74288ac0645f40cdcdc67a949a0bce8212a4f3524305a78ae9cf89e9a8c302aa2b276c3914b9c3b60d8c41226a22aefcf";

        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let destination = state
            .register_group_destination(
                &identity.identity_hash(),
                "personal",
                &["group"],
                &hx(GROUP_KEY),
            )
            .unwrap();
        assert_eq!(
            destination,
            DestinationHash::new(hx("4b31bea5e2b9b8f6ab79f8ae27a58319").try_into().unwrap()),
            "our GROUP address derivation matches RNS Destination.hash",
        );

        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination,
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        let token = hx(GROUP_TOKEN);
        wire[header_len..header_len + token.len()].copy_from_slice(&token);
        let mut raw = wire[..header_len + token.len()].to_vec();

        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Group(group),
            proof: ProofObligation::None,
        } = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0x07),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        )
        else {
            panic!("a GROUP packet for our registered group delivers, owing no proof");
        };
        assert_eq!(group.plaintext, b"group-hello");
        assert_eq!(group.destination, destination);
    }

    #[test]
    fn a_group_packet_for_an_unregistered_group_is_ignored() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Group,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new([0x99; 16]),
            context: WireContext::None,
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let header_len = header.write(&mut wire).unwrap();
        wire[header_len..header_len + 64].fill(0xAB);
        let mut raw = wire[..header_len + 64].to_vec();
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0x07),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_prove_all_delivery_carries_the_owed_proof() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-me");
        let packet_hash = PacketHash::of_wire_packet(&raw).unwrap();

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Delivery {
                delivery: Delivery::Single(SingleDelivery {
                    destination,
                    context: WireContext::None,
                    plaintext: b"prove-me",
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0x07; 8]),
                }),
                proof: ProofObligation::Owed(ProofOwed {
                    packet_hash,
                    identity: held,
                }),
            },
        );
    }

    #[test]
    fn single_data_for_an_unregistered_destination_is_ignored() {
        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let registered = state
            .register_single_destination(
                &identity.identity_hash(),
                "personal",
                &["other"],
                b"",
                ProofStrategy::ProveNone,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let unregistered = derive_destination_hash(
            &identity.identity_hash(),
            &crate::routing::announce::expand_name("personal", &["node"]).unwrap(),
        );
        assert_ne!(registered, unregistered);
        let mut raw = sealed_single_packet(&identity, unregistered, b"hello-single");

        assert_eq!(
            state.ingest_packet(
                plain_data_packet(&mut raw),
                TEST_ENTROPY,
                &transporting_view()
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn an_echo_of_our_own_announce_takes_no_route() {
        let mut state = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = state
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut relayed = announce_buf[..announce_len].to_vec();
        relayed[1] = 1;
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 8]),
                    bytes: &mut relayed,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "a transport echoing our announce back must not become a route to ourselves",
        );
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn a_node_without_transport_interfaces_learns_the_route_but_owes_no_rebroadcast() {
        use crate::interfaces::{EgressCapability, TransportCapability};

        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();
        let mut leaf = routable_descriptor(InterfaceId::new([0xEE; 8]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[leaf],
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NoTransportInterfaces,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.scheduled_announce_count(), 0);
    }

    #[test]
    fn a_final_hop_forward_strips_the_transport_header_back_to_the_direct_wire() {
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a one-hop route forwards, got {out:?}");
        };
        assert_eq!(forward.fire_on, InterfaceId::new([0xB2; 8]));
        let mut wire = [0u8; BROADCAST_MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = hx(RAW_SEALED_TO_RATCHET);
        expected[1] = 1;
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "the final hop strips transport framing: the destination hears the direct wire, one hop further",
        );

        let mut replay = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let again = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut replay,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            again,
            IngestPacketOutcome::Ignored,
            "a relay forwards each packet exactly once",
        );
    }

    #[test]
    fn relaying_a_packet_slides_the_carried_routes_expiry_forward() {
        let route_view = [routable_descriptor(InterfaceId::new([0xB2; 8]))];
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let learned_expiry = relay
            .routing_table
            .soonest_route_expiry(&route_view)
            .expect("the announce taught exactly one route");

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(120_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert!(
            matches!(out, IngestPacketOutcome::Forward(_)),
            "the transport-addressed packet forwards across the held route, got {out:?}",
        );

        let relayed_expiry = relay
            .routing_table
            .soonest_route_expiry(&route_view)
            .expect("the carried route survives the relay");
        assert_eq!(
            relayed_expiry.0,
            learned_expiry.0 + (120_000 - 500),
            "relaying slid the carried route's expiry forward by the gap since its announce, so it cannot age out mid-flow",
        );
    }

    #[test]
    fn a_mid_path_forward_swaps_the_transport_id_to_the_next_relay() {
        use crate::wire::PropagationType;

        let next_relay = TransportId::new([0xBB; 16]);
        let mut relay = transporting_node();

        let raw = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let relayed_header = WirePacketHeader {
            transport_id: Some(next_relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; BROADCAST_MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a multi-hop route forwards, got {out:?}");
        };
        let mut wire = [0u8; BROADCAST_MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        expected[1] = 1;
        expected[2..18].copy_from_slice(next_relay.as_bytes());
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "mid-path the only bytes that change are the hop count and the next relay's id",
        );
    }

    #[test]
    fn a_local_clients_direct_data_is_carried_out_to_its_route() {
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut direct = hx(RAW_SEALED_TO_RATCHET);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: app,
                bytes: &mut direct,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("an app sharing our instance has its direct data carried out, got {out:?}");
        };
        assert_eq!(
            forward.fire_on,
            iface(0xB2),
            "the local client's packet rides the route it could not reach itself",
        );
    }

    #[test]
    fn a_strangers_direct_data_to_a_routed_destination_is_still_dropped() {
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut direct = hx(RAW_SEALED_TO_RATCHET);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut direct,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Ignored,
            "carrying a stranger's direct data would make us an open relay; only the named \
             transport instance or a local-client app is carried",
        );
    }

    #[test]
    fn a_packet_for_a_destination_on_a_local_client_is_carried_inward() {
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"nomadnet");
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: app,
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: iface(0xA1),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a packet for an app on our instance is carried inward to it, got {out:?}");
        };
        assert_eq!(
            forward.fire_on, app,
            "the destination announced at zero hops is carried in over its own interface",
        );
    }

    #[test]
    fn a_proof_rides_the_reverse_route_home_exactly_once() {
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 8]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("the data leg must forward first");
        };
        let proof_destination = PacketHash::of_data_fields(
            forward.header.destination_type,
            &forward.header.destination,
            forward.header.context,
            forward.payload,
        )
        .proof_destination();

        let proof_header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: crate::wire::PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Proof,
            hops: 0,
            transport_id: None,
            destination: proof_destination,
            context: WireContext::None,
        };
        let mut proof_wire = [0u8; BROADCAST_MTU];
        let header_len = proof_header.write(&mut proof_wire).unwrap();
        proof_wire[header_len..header_len + 64].fill(0xAB);
        let proof_len = header_len + 64;

        let mut wrong_lane = proof_wire;
        let mut right_lane = proof_wire;

        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut right_lane[..proof_len],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let IngestPacketOutcome::Forward(returned) = out else {
            panic!("the proof must ride the reverse route, got {out:?}");
        };
        assert_eq!(
            returned.fire_on,
            InterfaceId::new([0xA1; 8]),
            "the proof leaves on the interface the data packet arrived from",
        );
        let mut wire = [0u8; BROADCAST_MTU];
        let n = returned.to_wire(&mut wire).unwrap();
        let mut expected = std::vec::Vec::new();
        expected.extend_from_slice(&proof_wire[..proof_len]);
        expected[1] = 1;
        assert_eq!(&wire[..n], expected.as_slice());

        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(3_000),
                source_interface: InterfaceId::new([0xB2; 8]),
                bytes: &mut wrong_lane[..proof_len],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            out,
            IngestPacketOutcome::Proof(crate::engine::ProofIngest::Ignored),
            "reverse rows pop on use: the second copy finds no path home",
        );
    }

    #[test]
    fn ingest_accepts_a_real_announce_then_rejects_its_replay() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = transporting_node();

        let first = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(first, raw_announce_accepted(1));
        assert_eq!(state.route_count(), 1);

        let second = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            second,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored)
        );
        assert_eq!(state.route_count(), 1);
    }

    #[test]
    fn received_hops_are_incremented_so_the_reach_boundary_matches_pathfinder_m() {
        let mut at_limit = hx(RAW_ANNOUNCE);
        at_limit[1] = 127;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut at_limit,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(128));

        let mut beyond = hx(RAW_ANNOUNCE);
        beyond[1] = 128;
        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut beyond,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, IngestPacketOutcome::Announce(AnnounceIngest::Ignored));
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_accepted_announce_is_retained_for_faithful_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let pristine = raw.clone();
        let (header, payload) = WirePacketHeader::parse(&pristine).unwrap();
        let destination =
            DestinationHash::from_slice(&pristine[2..18]).expect("16-byte destination hash");

        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(1));

        let retained = state
            .routing_table
            .retained_announce_for(&destination)
            .expect("the accepted announce is on hand");
        assert_eq!(retained.hops, header.hops + 1);
        let mut buf = [0u8; 500];
        let n = retained.announce.to_wire(&mut buf).unwrap();
        assert_eq!(&buf[..n], payload);
    }

    #[test]
    fn a_node_without_a_transport_id_learns_the_route_but_owes_no_rebroadcast() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap(),
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::NotATransportNode,
            })),
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(state.scheduled_announce_count(), 0);
    }

    #[test]
    fn a_relayed_announce_routes_via_its_transport_node_and_a_direct_one_routes_direct() {
        use crate::routing::NextHop;
        use crate::wire::PropagationType;

        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let destination = header.destination;
        let relay = TransportId::new([0xBB; 16]);

        let relayed_header = WirePacketHeader {
            transport_id: Some(relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; BROADCAST_MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);

        let mut state = transporting_node();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, raw_announce_accepted(2));
        assert_eq!(
            state
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Via(relay),
            "a relayed announce's next hop is the transport node that stamped it",
        );

        let mut direct = raw.clone();
        let mut fresh = transporting_node();
        let _ = fresh.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut direct,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            fresh
                .routing_table
                .retained_announce_for(&destination)
                .unwrap()
                .next_hop,
            NextHop::Direct,
            "an unrelayed announce is reachable directly",
        );
    }

    #[test]
    fn ingest_processes_but_does_not_accept_non_announce_bytes() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut [0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = state.ingest_packet(junk, TEST_ENTROPY, &transporting_view());
        assert_eq!(out, IngestPacketOutcome::Ignored);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_ifac_flagged_packet_is_dropped_on_an_open_interface() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[0] |= 0x80;
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(out, IngestPacketOutcome::Ignored);
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_announce_whose_app_data_can_never_fit_is_ignored() {
        let mut raw = hx(RAW_ANNOUNCE);
        let mut state = EngineState::<
            TestFixedStorage<4, 64, 8, 4, 512, 8, 8, 128, 8, 8, 8, 8, 16, 16>,
        >::default();

        let out = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "an app_data larger than the whole arena has no eviction that can admit it",
        );
        assert_eq!(state.route_count(), 0);
    }

    fn flood_announce(seed: u8, hops: u8) -> std::vec::Vec<u8> {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[seed.wrapping_add(1); 64]);
        let app = [seed; 4];
        let announce = Announce::build_signed(
            &signer,
            crate::routing::announce::DottedNameHash::new([0u8; 10]),
            crate::routing::announce::AnnounceId::from_wire([seed; 10]),
            None,
            &app,
        )
        .expect("a built announce");
        let mut buf = [0u8; BROADCAST_MTU];
        let n = crate::engine::egress::write_announce_wire_packet(&announce, hops, &mut buf)
            .expect("announce serializes");
        buf[..n].to_vec()
    }

    #[test]
    fn a_flood_of_unknown_announces_is_held_then_drip_released_lowest_hop_first() {
        use crate::engine::{EngineReaction, Journaled};

        let source = InterfaceId::new([0xEE; 8]);
        let view = transporting_view();
        let mut relay = transporting_node();

        let mut accepted = 0usize;
        let mut held = 0usize;
        for i in 0..8u8 {
            let mut wire = flood_announce(i, 10 - i);
            match relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000 + u64::from(i) * 5),
                    source_interface: source,
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &view,
            ) {
                IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)) => accepted += 1,
                IngestPacketOutcome::Announce(AnnounceIngest::Held) => held += 1,
                other => panic!("a valid announce is accepted or held, got {other:?}"),
            }
        }
        assert!(
            accepted >= 1,
            "the announces under the burst threshold are processed normally",
        );
        assert!(
            held >= 1,
            "the flood past the threshold is parked, not processed"
        );
        assert_eq!(relay.held_announces.len(), held);
        assert_eq!(
            relay.route_count(),
            accepted,
            "a held announce has not become a route yet",
        );

        let mut released_hops = std::vec::Vec::new();
        for step in 0..(held as u64 + 4) {
            if relay.held_announces.is_empty() {
                break;
            }
            let now = InstantMillis(1_000 + 15_000 + step * 5_000);
            relay.fire_due_held_announces(
                now,
                &view,
                &mut |bytes: &mut [u8]| bytes.fill(0xE7),
                &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::AnnounceHeard { hops, .. }) =
                        reaction
                    {
                        released_hops.push(hops);
                    }
                },
            );
        }

        assert!(
            relay.held_announces.is_empty(),
            "once the burst subsides every held announce drips out",
        );
        assert_eq!(released_hops.len(), held, "each is released exactly once");
        assert_eq!(
            relay.route_count(),
            accepted + held,
            "and each held announce becomes a route on release",
        );
        let mut ascending = released_hops.clone();
        ascending.sort_unstable();
        assert_eq!(
            released_hops, ascending,
            "they drip lowest-hop first, not in arrival order",
        );
    }
}
