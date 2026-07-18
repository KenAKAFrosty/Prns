mod announce;
mod classification;
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

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn hops_across_local_boundary(
        &self,
        received_hops: u8,
        source_interface: InterfaceId,
        outbound_interface: InterfaceId,
    ) -> u8 {
        let crosses_local_boundary = source_interface.kind() == Some(InterfaceKind::LocalClient)
            && outbound_interface.kind() != Some(InterfaceKind::LocalClient);
        self.protocol
            .local_hop_count_override
            .apply(received_hops, crosses_local_boundary)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn ingest_packet_with<'p>(
        &mut self,
        packet: crate::interfaces::InboundPacket<'p>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
        deferred: Option<&mut DeferredCrypto>,
    ) -> IngestPacketOutcome<'p> {
        let (_, ingress) = ClassifiedInboundPacket::classify(packet).into_parts();
        self.ingest_classified_with(ingress, fill_entropy, interfaces, on_removed, deferred)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn ingest_classified_with<'p>(
        &mut self,
        ingress: Ingress<'p>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
        deferred: Option<&mut DeferredCrypto>,
    ) -> IngestPacketOutcome<'p> {
        self.ingest_classified_with_effects(
            ingress,
            fill_entropy,
            interfaces,
            on_removed,
            deferred,
            &mut IngestEffects::default(),
        )
    }

    pub(crate) fn ingest_classified_with_effects<'p>(
        &mut self,
        ingress: Ingress<'p>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
        deferred: Option<&mut DeferredCrypto>,
        effects: &mut IngestEffects<'p>,
    ) -> IngestPacketOutcome<'p> {
        self.ingested_packet_count = self.ingested_packet_count.saturating_add(1);

        match ingress {
            Ingress::Announce {
                packet_hash: _,
                identity_hash,
                announce,
                payload,
                header,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
                is_path_response,
            } => {
                if self.identity_blackholes.is_blackholed(&identity_hash) {
                    return IngestPacketOutcome::Announce(AnnounceIngest::Blackholed);
                }
                if received_hops > MAX_HOP_COUNT {
                    return IngestPacketOutcome::Announce(AnnounceIngest::Ignored);
                }

                let unknown_route = !self.routing_table.has_route(&announce.destination);

                let satisfies_pending_path_request =
                    self.pending_path_requests.contains(&announce.destination)
                        || self.recursive_path_requests.contains(&announce.destination);

                let ingress_policy = interfaces.descriptor_for(source_interface).map_or(
                    InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
                    |descriptor| descriptor.common.ingress_control,
                );

                let should_hold_for_ingress_burst = unknown_route
                    && !satisfies_pending_path_request
                    && self.interface_announce_limits.should_limit_with_policy(
                        source_interface,
                        arrived_at,
                        ingress_policy,
                    );

                if should_hold_for_ingress_burst {
                    if !announce.signature_is_valid() {
                        return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
                    }
                    self.interface_announce_limits
                        .record(source_interface, arrived_at);
                    let held = match self.held_announces.hold_with_limit(
                        received_hops,
                        source_interface,
                        next_hop,
                        is_path_response,
                        &announce,
                        ingress_policy.max_held_announces,
                    ) {
                        HoldOutcome::Held | HoldOutcome::Replaced | HoldOutcome::StaleKept => {
                            AnnounceIngest::Held
                        }
                        HoldOutcome::NewcomerDropped(cause) => AnnounceIngest::HeldDropped {
                            destination: announce.destination,
                            cause,
                        },
                    };
                    return IngestPacketOutcome::Announce(held);
                }

                if let Some(deferred) = deferred {
                    let mut owned = HeaplessVec::new();
                    if owned.extend_from_slice(payload).is_err() {
                        return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted);
                    }
                    *deferred = DeferredCrypto::AnnounceVerify(AnnounceVerifyOwed {
                        payload: owned,
                        header,
                        received_hops,
                        source_interface,
                        arrived_at,
                        next_hop,
                        is_path_response,
                    });
                    return IngestPacketOutcome::OwesAnnounceVerify;
                }

                if !announce.signature_is_valid() {
                    return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
                }
                self.interface_announce_limits
                    .record(source_interface, arrived_at);
                let arrival = AnnounceArrival {
                    announce,
                    hops: received_hops,
                    arrived_at,
                    receiving_interface: source_interface,
                    next_hop,
                    is_path_response,
                };
                IngestPacketOutcome::Announce(self.ingest_announce(
                    identity_hash,
                    &arrival,
                    &mut *fill_entropy,
                    interfaces,
                    on_removed,
                    effects,
                ))
            }

            Ingress::Data {
                packet_hash,
                data,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                if data.header.destination_type == DestinationType::Link {
                    return self.ingest_link_addressed(
                        data,
                        packet_hash,
                        received_hops,
                        source_interface,
                        arrived_at,
                    );
                }
                if data.header.destination_type == DestinationType::Plain {
                    let address = DestinationHash::from_address(data.header.address);
                    if address == PATH_REQUEST_DESTINATION {
                        return self.ingest_path_request(
                            &data,
                            source_interface,
                            arrived_at,
                            interfaces,
                        );
                    }
                    if address == TUNNEL_SYNTHESIZE_DESTINATION {
                        return self.ingest_tunnel_synthesize(&data, source_interface, arrived_at);
                    }
                }

                if let Some(transport_id) = data.header.transport_id {
                    if self.transport_id() != Some(transport_id) {
                        return IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance);
                    }
                }

                if matches!(
                    data.header.destination_type,
                    DestinationType::Plain | DestinationType::Group
                ) && received_hops > NON_TRANSPORTED_DATA_MAX_RECEIVED_HOPS
                {
                    return IngestPacketOutcome::Ignored(IgnoreReason::HopLimitReached);
                }

                if data.header.destination_type == DestinationType::Single
                    && self.packet_hash_history.remember(packet_hash)
                        == RememberPacketOutcome::AlreadyKnown
                {
                    return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate);
                }

                let is_not_for_upstream_app = self
                    .upstream_app_destinations
                    .lookup(
                        &DestinationHash::from_address(data.header.address),
                        data.header.destination_type,
                    )
                    .is_none();

                let is_in_transport_through_us = self.network_transport_enabled()
                    && data.header.transport_id == self.transport_id()
                    && is_not_for_upstream_app;

                let is_shared_client_transit = is_not_for_upstream_app
                    && data.header.destination_type == DestinationType::Single
                    && (source_interface.kind() == Some(InterfaceKind::LocalClient)
                        || self.routes_via_local_client(&DestinationHash::from_address(
                            data.header.address,
                        )));

                if is_in_transport_through_us || is_shared_client_transit {
                    let DataPacket { header, payload } = data;
                    return match self.maybe_forward(
                        header,
                        payload,
                        packet_hash,
                        received_hops,
                        ForwardingArrival {
                            source_interface,
                            arrived_at,
                            interfaces,
                        },
                    ) {
                        Some(forward) => IngestPacketOutcome::Forward(forward),
                        None => IngestPacketOutcome::Ignored(IgnoreReason::NoRoute),
                    };
                }
                match self.maybe_upstream_delivery(
                    data,
                    packet_hash,
                    source_interface,
                    arrived_at,
                    deferred,
                ) {
                    UpstreamDeliveryOutcome::Delivered(delivery, proof) => {
                        IngestPacketOutcome::Delivery { delivery, proof }
                    }
                    UpstreamDeliveryOutcome::OwesDecrypt => IngestPacketOutcome::OwesDecrypt,
                    UpstreamDeliveryOutcome::OwesRatchetDecrypt => {
                        IngestPacketOutcome::OwesRatchetDecrypt
                    }
                    UpstreamDeliveryOutcome::NotForUs => {
                        IngestPacketOutcome::Ignored(IgnoreReason::NotForUs)
                    }
                }
            }

            Ingress::Proof {
                packet_hash: _,
                payload,
                address,
                context,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                let link_id = LinkId::from_address(address);
                if context == WireContext::LinkRequestProof {
                    return self.ingest_link_proof(
                        link_id,
                        payload,
                        received_hops,
                        source_interface,
                        arrived_at,
                        deferred,
                    );
                }
                if context == WireContext::ResourceProof {
                    match self.ingest_resource_proof(link_id, payload, arrived_at) {
                        ResourceProofClassification::Resolved(outcome) => return outcome,
                        ResourceProofClassification::NotALocalLink => {}
                    }
                }

                match self.relay_if_transported(
                    address,
                    context,
                    PacketType::Proof,
                    received_hops,
                    source_interface,
                    arrived_at,
                ) {
                    RelayOutcome::Forward { header, fire_on } => {
                        return IngestPacketOutcome::Forward(PacketToForward {
                            header,
                            payload,
                            fire_on,
                        });
                    }
                    RelayOutcome::NotTransportedByUs => {}
                }

                if let Some(reverse) = self
                    .reverse_routes
                    .take(&DestinationHash::from_address(address), arrived_at)
                {
                    if reverse.outbound_interface != source_interface {
                        return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs);
                    }
                    return IngestPacketOutcome::Forward(PacketToForward {
                        header: WirePacketHeader {
                            ifac_flag: IfacFlag::Open,
                            context_flag: ContextFlag::Unset,
                            propagation: PropagationType::Broadcast,
                            destination_type: DestinationType::Single,
                            packet_type: PacketType::Proof,
                            hops: self.hops_across_local_boundary(
                                received_hops,
                                source_interface,
                                reverse.received_interface,
                            ),
                            transport_id: None,
                            address,
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
                let outcome = self.settle_receipt_proof(payload, arrived_at);
                if matches!(outcome, ProofIngest::SendToLinkDelivered { .. }) {
                    self.links.note_inbound(&link_id, arrived_at);
                }
                IngestPacketOutcome::Proof(outcome)
            }

            Ingress::LinkRequest {
                packet_hash,
                payload,
                header,
                received_hops,
                source_interface,
                arrived_at,
            } => self.ingest_link_request(
                &header,
                payload,
                LinkRequestArrival {
                    packet_hash,
                    received_hops,
                    source_interface,
                    arrived_at,
                    interfaces,
                },
            ),
            Ingress::Malformed => IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
            Ingress::IfacRefused => IngestPacketOutcome::Ignored(IgnoreReason::IfacRefused),
        }
    }

    fn ingest_tunnel_synthesize<'p>(
        &mut self,
        data: &DataPacket<'_>,
        source_interface: InterfaceId,
        now: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let Some(verified) = parse_synthesize_payload(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
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
        self.routing_table.invalidate_route_expiries();
        IngestPacketOutcome::TunnelObserved { expires }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::interfaces::InboundPacket;

    #[test]
    fn ingest_counts_each_packet_without_a_clock() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();

        let mut first_bytes = [1, 2, 3];
        let first = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(10),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut first_bytes,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        let mut second_bytes = [4];
        let second = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(20),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut second_bytes,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );

        assert_eq!(first, IngestPacketOutcome::Ignored(IgnoreReason::Malformed));
        assert_eq!(
            second,
            IngestPacketOutcome::Ignored(IgnoreReason::Malformed)
        );
        assert_eq!(state.ingested_packet_count(), 2);
    }

    #[test]
    fn ingest_processes_but_does_not_accept_non_announce_bytes() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let junk = InboundPacket {
            arrived_at: InstantMillis(1),
            source_interface: InterfaceId::new([0u8; 8]),
            bytes: &mut [0x00, 0x00, 0x01, 0x02, 0x03],
        };
        let out = state.ingest_packet_with(
            junk,
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(out, IngestPacketOutcome::Ignored(IgnoreReason::Malformed));
        assert_eq!(state.route_count(), 0);
    }

    #[test]
    fn an_ifac_flagged_packet_is_dropped_on_an_open_interface() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        raw[0] |= 0x80;
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(out, IngestPacketOutcome::Ignored(IgnoreReason::IfacRefused));
        assert_eq!(state.route_count(), 0);
    }
}
