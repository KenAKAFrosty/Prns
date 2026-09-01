use super::announce::{AnnounceIngest, AnnounceVerifyOwed};
#[cfg(test)]
use super::classification::ClassifiedInboundPacket;
use super::classification::{DataPacket, Ingress};
use super::forward::{ForwardingArrival, PacketToForward};
use super::links::{LinkRequestArrival, RelayOutcome};
use super::outcome::{
    IgnoreReason, IngestEffects, IngestPacketOutcome, NON_TRANSPORTED_DATA_MAX_RECEIVED_HOPS,
};
use super::upstream_delivery::UpstreamDeliveryOutcome;
use crate::engine::{DeliveryProof, EngineState, InstantMillis};
use crate::interfaces::{AttachedInterfaces, InterfaceCommonPolicy, InterfaceId, InterfaceKind};
use crate::remote_control::RemoteControlPairingAvailabilityDestination;
use crate::routing::announce::held::HoldOutcome;
#[cfg(test)]
use crate::routing::announce::AnnounceArrival;
use crate::routing::dedup::{PacketHashHistory, RememberPacketOutcome};
use crate::routing::links::resources::send::ResourceProofClassification;
use crate::routing::links::LinkId;
use crate::routing::path_requests::PATH_REQUEST_DESTINATION;
use crate::routing::tunnel::{
    parse_synthesize_payload, TunnelTransition, TUNNEL_SYNTHESIZE_DESTINATION, TUNNEL_TIMEOUT_MS,
};
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, MAX_HOP_COUNT,
};
use heapless::Vec as HeaplessVec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IngressCryptoMode {
    Owed,
    #[cfg(test)]
    Inline,
}

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
    #[deprecated(
        note = "test-only inline compatibility manifold; use ingest_packet_step_with or drive Directive::Fulfill and resume_* explicitly"
    )]
    pub(crate) fn ingest_packet_with<'p>(
        &mut self,
        packet: crate::interfaces::InboundPacket<'p>,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'p> {
        let (_, ingress) = ClassifiedInboundPacket::classify(packet).into_parts();
        self.ingest_classified_with_mode(
            ingress,
            interfaces,
            IngressCryptoMode::Inline,
            &mut IngestEffects::default(),
        )
    }

    /// One raw ingress transition, stopping at an owed-work boundary.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn ingest_packet_step_with<'p>(
        &mut self,
        packet: crate::interfaces::InboundPacket<'p>,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'p> {
        let (_, ingress) = ClassifiedInboundPacket::classify(packet).into_parts();
        self.ingest_classified_with(ingress, interfaces)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn ingest_classified_with<'p>(
        &mut self,
        ingress: Ingress<'p>,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'p> {
        self.ingest_classified_with_mode(
            ingress,
            interfaces,
            IngressCryptoMode::Owed,
            &mut IngestEffects::default(),
        )
    }

    pub(crate) fn ingest_classified_with_effects<'p>(
        &mut self,
        ingress: Ingress<'p>,
        interfaces: AttachedInterfaces<'_>,
        effects: &mut IngestEffects<'p>,
    ) -> IngestPacketOutcome<'p> {
        self.ingest_classified_with_mode(ingress, interfaces, IngressCryptoMode::Owed, effects)
    }

    fn ingest_classified_with_mode<'p>(
        &mut self,
        ingress: Ingress<'p>,
        interfaces: AttachedInterfaces<'_>,
        crypto: IngressCryptoMode,
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
                    effects.held_announce_release = self.held_announce_release_wake();
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

                match crypto {
                    IngressCryptoMode::Owed => {
                        let mut owned = HeaplessVec::new();
                        if owned.extend_from_slice(payload).is_err() {
                            return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted);
                        }
                        IngestPacketOutcome::OwesAnnounceVerify(AnnounceVerifyOwed {
                            payload: owned,
                            header,
                            received_hops,
                            source_interface,
                            arrived_at,
                            next_hop,
                            is_path_response,
                        })
                    }
                    #[cfg(test)]
                    IngressCryptoMode::Inline => {
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
                            &mut |_| {},
                            interfaces,
                            &mut |_| {},
                            effects,
                        ))
                    }
                }
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
                    if address
                        == RemoteControlPairingAvailabilityDestination::canonical()
                            .destination_hash()
                    {
                        return self.ingest_remote_control_pairing_availability(
                            data,
                            super::RemoteControlPairingAvailabilityArrival {
                                received_hops,
                                source_interface,
                                arrived_at,
                            },
                            crypto,
                            interfaces,
                            effects,
                        );
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
                    crypto,
                ) {
                    UpstreamDeliveryOutcome::Delivered(delivery, proof) => {
                        IngestPacketOutcome::Delivery { delivery, proof }
                    }
                    UpstreamDeliveryOutcome::OwesDecrypt(owed) => {
                        IngestPacketOutcome::OwesDecrypt(owed)
                    }
                    UpstreamDeliveryOutcome::OwesRatchetDecrypt(owed) => {
                        IngestPacketOutcome::OwesRatchetDecrypt(owed)
                    }
                    UpstreamDeliveryOutcome::NotForUs => {
                        IngestPacketOutcome::Ignored(IgnoreReason::NotForUs)
                    }
                }
            }

            Ingress::Proof {
                packet_hash,
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
                        crypto,
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
                if let Some((id, delivered)) = self.settle_channel_ack(
                    &link_id,
                    payload,
                    DeliveryProof::Explicit(packet_hash),
                    arrived_at,
                ) {
                    self.links.note_inbound(&link_id, arrived_at);
                    return IngestPacketOutcome::ChannelReceiptDelivered { id, delivered };
                }
                match self.prepare_receipt_proof_verify(
                    payload,
                    &DestinationHash::from_address(address),
                    packet_hash,
                    arrived_at,
                ) {
                    Some(owed) => IngestPacketOutcome::OwesReceiptProofVerify(owed),
                    None => IngestPacketOutcome::ReceiptProofIgnored,
                }
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
mod tests;
