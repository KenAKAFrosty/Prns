mod announce;
mod forward;
mod links;
mod path_requests;
#[cfg(test)]
pub(super) mod testkit;
mod upstream_delivery;

pub use announce::{AcceptedAnnounce, AnnounceIngest, AnnounceVerifyOwed, RebroadcastDecision};
pub use forward::PacketToForward;
pub use links::ForwardedLinkRequestBody;
pub use upstream_delivery::{
    DecryptOwed, RatchetDecryptOwed, MAX_POOLED_RATCHETS, MAX_RATCHET_DECRYPT_PAYLOAD_LEN,
    MAX_SINGLE_TOKEN_LEN,
};

use crate::crypto::{token_open_in_place, TokenKey};
use crate::crypto::{Ed25519PublicKey, X25519PublicKey, X25519SecretKey};
use crate::engine::CommandId;
use crate::engine::EngineState;
use crate::engine::InstantMillis;
use crate::engine::LinkClosedReason;
use crate::engine::PacketReceiptDelivered;
use crate::engine::MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN;
use crate::engine::PATH_REQUEST_DESTINATION;
use crate::identity::IdentityHash;
use crate::identity::{ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN, ENCRYPTION_IV_LEN};
use crate::interfaces::{
    InboundPacket, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceMode,
};
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS, MAX_ANNOUNCE_REBROADCASTS,
    PATH_REQUEST_GRACE_MS, PATH_REQUEST_ROAMING_GRACE_MS,
};
use crate::routing::announce::rate_limit::AnnounceRateVerdict;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::{Announce, AnnounceArrival};
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
    link_proof_from, link_proof_parse, link_request_from, link_rtt_from, signalling_bytes_from,
    AcceptedLinkRequest, LinkProofSignOwed, LinkProofVerifyOwed, LinkRequest, LinkRttError,
    LINK_REQUEST_KEYS_LEN, SIGNALLED_LINK_REQUEST_LEN,
};
use crate::routing::links::identify::peer_identity_from;
use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
use crate::routing::links::request::{
    parse_request_plaintext, parse_response_plaintext, RequestId,
};
use crate::routing::links::resources::{ResourceHash, ResourcePartRequest};
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
use heapless::Vec as HeaplessVec;

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
        payload: &'a [u8],
        header: WirePacketHeader,
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

/// RNS `Transport.inbound`'s `hops -= 1` for local clients: a LocalClient packet crossed
/// no real hop, so the shared instance plus its apps count as a single node.
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
                let Ok(announce) = Announce::from_wire_unverified(&header, payload) else {
                    return Self::Unparseable;
                };

                // Debug self-check: if `to_wire` ever drifts from `from_wire`, the engine
                // would silently re-emit a signature-broken packet on rebroadcast. Zero
                // cost in release.
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
                    payload,
                    header,
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

/// `Option<&mut DeferredCrypto>` at the ingest entries is the pool seam itself:
/// `None` runs every verify/decrypt/sign synchronously; `Some` arms the slots and
/// the caller drains whatever the packet deposited.
#[derive(Default)]
pub struct DeferredCrypto {
    pub decrypt: Option<DecryptOwed>,
    pub ratchet_decrypt: Option<RatchetDecryptOwed>,
    pub link_proof_verify: Option<LinkProofVerifyOwed>,
    pub link_proof_sign: Option<LinkProofSignOwed>,
    pub announce_verify: Option<AnnounceVerifyOwed>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRttOwed {
    pub link_id: LinkId,
    pub responder_encryption: X25519PublicKey,
    pub responder_signing: Ed25519PublicKey,
    pub command_id: CommandId,
    pub rtt: Rtt,
    pub mtu: usize,
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
    /// Answered after the request grace, letting directly reachable peers respond first.
    ScheduledPathResponse {
        destination: DestinationHash,
    },
    /// RNS `DISCOVER_PATHS_FOR`: forwarded on the requester's behalf on every other
    /// transport interface (Transport.py:3006 local client, :3015 recursive discovery);
    /// the asking interface is remembered to steer the answer back.
    ForwardPathRequestForDiscovery {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// Offered to local clients only (RNS Transport.py:3043), never recursed out; the
    /// asking interface is remembered to steer the answer home.
    RelayPathRequestToLocalClients {
        destination: DestinationHash,
        id: PathRequestIdBytes,
    },
    /// RNS 1.3.5's `remote_identified` callback.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    RequestReceived {
        link_id: LinkId,
        request_id: RequestId,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: Rtt,
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
    },
    OwesResourcePull {
        link_id: LinkId,
        hash: ResourceHash,
    },
    OwesResourceAssembly {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// No part request or assembly is owed, but the resource lane must still resync
    /// to the fresh deadline; `Ignored` would silently strand it.
    ResourceProgressed,
    ResourceConcludedFailed {
        link_id: LinkId,
        hash: ResourceHash,
    },
    ResourceRejectedByPeer {
        id: CommandId,
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
    /// RNS 1.3.5 `Link.receive` (Link.py:975): dropped as a possible manipulation
    /// attempt; we surface the mismatch rather than swallowing it.
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

/// RNS 1.3.5 `Transport.packet_filter`'s duplicate-filter exemptions: these contexts
/// retry byte-identically by design, so deduplicating them severs every retry that
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

fn iface_config(view: &[InterfaceConfig], id: InterfaceId) -> Option<&InterfaceConfig> {
    view.iter().find(|config| config.id == id)
}

impl<S: StorageLayout> EngineState<S> {
    #[must_use]
    pub fn ingest_packet<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'p> {
        self.ingest_packet_with(packet, jitter, interfaces, &mut |_| {}, None)
    }

    #[must_use]
    pub(crate) fn ingest_packet_with<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
        on_removed: &mut impl FnMut(RemovedRoute),
        mut deferred: Option<&mut DeferredCrypto>,
    ) -> IngestPacketOutcome<'p> {
        self.ingested_packet_count = self.ingested_packet_count.saturating_add(1);

        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                payload,
                header,
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
                    if !announce.signature_is_valid() {
                        return IngestPacketOutcome::Ignored;
                    }
                    self.held_announces.hold(
                        received_hops,
                        source_interface,
                        next_hop,
                        is_path_response,
                        &announce,
                    );
                    IngestPacketOutcome::Announce(AnnounceIngest::Held)
                } else if let Some(deferred) = deferred {
                    let mut owned = HeaplessVec::new();
                    if owned.extend_from_slice(payload).is_err() {
                        return IngestPacketOutcome::Ignored;
                    }
                    deferred.announce_verify = Some(AnnounceVerifyOwed {
                        payload: owned,
                        header,
                        received_hops,
                        source_interface,
                        arrived_at,
                        next_hop,
                        is_path_response,
                        jitter,
                    });
                    IngestPacketOutcome::OwesAnnounceVerify
                } else {
                    if !announce.signature_is_valid() {
                        return IngestPacketOutcome::Ignored;
                    }
                    let arrival = AnnounceArrival {
                        announce,
                        hops: received_hops,
                        arrived_at,
                        receiving_interface: source_interface,
                        next_hop,
                        is_path_response,
                    };
                    IngestPacketOutcome::Announce(
                        self.ingest_announce(&arrival, jitter, interfaces, on_removed),
                    )
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
                    deferred.as_deref_mut(),
                ) {
                    Some((delivery, proof)) => IngestPacketOutcome::Delivery { delivery, proof },
                    None => match deferred {
                        Some(deferred) if deferred.decrypt.is_some() => {
                            IngestPacketOutcome::OwesDecrypt
                        }
                        Some(deferred) if deferred.ratchet_decrypt.is_some() => {
                            IngestPacketOutcome::OwesRatchetDecrypt
                        }
                        _ => IngestPacketOutcome::Ignored,
                    },
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
                        deferred,
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
                    // toward; anything else is dropped (Transport.py:2258).
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
                    self.links.note_inbound(&link_id, arrived_at);
                    return IngestPacketOutcome::Proof(ProofIngest::SendToChannelDelivered {
                        id,
                        delivered,
                    });
                }
                let outcome = self.ingest_proof(payload, arrived_at);
                if matches!(outcome, ProofIngest::SendToLinkDelivered { .. }) {
                    // Extends the link's liveness exactly as RNS 1.3.5's `link.last_proof` does.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::routing::ingress::testkit::{header_bytes, iface};
    use crate::wire::HEADER_MIN_LEN;

    #[test]
    fn a_local_client_transit_is_discounted_one_hop() {
        let local_client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app-1");
        let tcp = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"1.2.3.4:4242");
        assert_eq!(local_adjusted_hops(5, local_client), 4);
        assert_eq!(local_adjusted_hops(5, tcp), 5);
        assert_eq!(local_adjusted_hops(0, local_client), 0);
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
}
