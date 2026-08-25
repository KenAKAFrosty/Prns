use super::classification::DataPacket;
use super::forward::PacketToForward;
use super::outcome::{DeferredCrypto, IgnoreReason, IngestPacketOutcome, LinkRttOwed};
use crate::crypto::X25519PublicKey;
use crate::engine::{
    DeliveryEvidence, EngineState, InstantMillis, LinkClosedReason, PacketReceiptDelivered,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceKind};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::send_single::DEFAULT_PER_HOP_TIMEOUT_MS;
use crate::routing::delivery::{Delivery, LinkDelivery};
use crate::routing::links::channel::parse_envelope;
use crate::routing::links::channel::table::ChannelTable;
use crate::routing::links::handshake::{
    link_proof_from, link_proof_parse, link_request_from, link_rtt_from, signalling_bytes_from,
    AcceptedLinkRequest, LinkProofVerifyOwed, LinkRequest, LinkRttError, LINK_PROOF_BODY_LEN,
    LINK_REQUEST_KEYS_LEN, SIGNALLED_LINK_PROOF_LEN, SIGNALLED_LINK_REQUEST_LEN,
};
use crate::routing::links::identify::peer_identity_from;
use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
use crate::routing::links::request::{
    parse_request_plaintext, parse_response_plaintext, RequestId,
};
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::transported::{
    extra_link_proof_timeout_ms, TrackTransportedLinkError, TransportSwitch, TransportedLink,
};
use crate::routing::links::LinkId;
use crate::routing::proof::{LinkProofOwed, ProofObligation};
use crate::routing::routes::RouteEvidenceHandle;
use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use crate::routing::NextHop;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireAddress, WireContext, WirePacketHeader, BROADCAST_MTU,
};

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

pub(super) enum RelayOutcome {
    Forward {
        header: WirePacketHeader,
        fire_on: InterfaceId,
    },
    NotTransportedByUs,
}

#[derive(Clone, Copy)]
pub(super) struct LinkRequestArrival<'a> {
    pub(super) packet_hash: PacketHash,
    pub(super) received_hops: u8,
    pub(super) source_interface: InterfaceId,
    pub(super) arrived_at: InstantMillis,
    pub(super) interfaces: AttachedInterfaces<'a>,
}

struct AcceptedTransportedLinkProof {
    switch: TransportSwitch,
    destination: DestinationHash,
    next_hop_interface: InterfaceId,
    received_interface: InterfaceId,
    route_evidence: RouteEvidenceHandle,
    route_update: TransportedRouteUpdate,
}

enum TransportedRouteUpdate {
    Unchanged,
    RebalancedTo(u8),
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_link_addressed<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        match self.relay_if_transported(
            data.header.address,
            data.header.context,
            PacketType::Data,
            received_hops,
            source_interface,
            arrived_at,
        ) {
            RelayOutcome::Forward { header, fire_on } => {
                return IngestPacketOutcome::Forward(PacketToForward {
                    header,
                    payload: data.payload,
                    fire_on,
                });
            }
            RelayOutcome::NotTransportedByUs => {}
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
        match data.header.context {
            WireContext::LinkRtt => {
                self.ingest_link_rtt(link_id, data.payload, source_interface, arrived_at)
            }
            WireContext::None => {
                self.ingest_link_data(data, packet_hash, source_interface, arrived_at)
            }
            WireContext::KeepAlive => self.ingest_keepalive(link_id, data.payload, arrived_at),
            WireContext::LinkClose => self.ingest_link_close(data, arrived_at),
            WireContext::LinkIdentify => self.ingest_link_identify(data, arrived_at),
            WireContext::Request => self.ingest_request_over_link(data, packet_hash, arrived_at),
            WireContext::Response => self.ingest_response_over_link(data, packet_hash, arrived_at),
            WireContext::ResourceRequest => self.ingest_resource_request(data, arrived_at),
            WireContext::ResourceAdvertisement => {
                self.ingest_resource_advertisement(data, arrived_at)
            }
            WireContext::Resource => self.ingest_resource_part(data, arrived_at),
            WireContext::ResourceHashUpdate => {
                self.ingest_resource_hashmap_update(data, arrived_at)
            }
            WireContext::ResourceInitiatorCancel => self.ingest_resource_cancel(data, arrived_at),
            WireContext::ResourceReceiverCancel => {
                self.ingest_resource_receiver_cancel(data, arrived_at)
            }
            WireContext::Channel => self.ingest_channel_data(data, packet_hash, arrived_at),
            WireContext::ResourceProof
            | WireContext::CacheRequest
            | WireContext::PathResponse
            | WireContext::Command
            | WireContext::CommandStatus
            | WireContext::LinkProof
            | WireContext::LinkRequestProof
            | WireContext::Unknown(_) => {
                IngestPacketOutcome::Ignored(IgnoreReason::UnhandledContext)
            }
        }
    }

    /// Transported-link retries bypass packet dedup to match RNS 1.4.2; `switch_through` prevents loops by validating hop direction.
    pub(super) fn relay_if_transported(
        &mut self,
        address: WireAddress,
        context: WireContext,
        packet_type: PacketType,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> RelayOutcome {
        let link_id = LinkId::from_address(address);
        if self.links.has_local_link(&link_id) || context == WireContext::LinkRequestProof {
            return RelayOutcome::NotTransportedByUs;
        }
        let Ok(switch) = self.transported_links.switch_through(
            &link_id,
            source_interface,
            received_hops,
            arrived_at,
        ) else {
            return RelayOutcome::NotTransportedByUs;
        };
        RelayOutcome::Forward {
            header: WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: DestinationType::Link,
                packet_type,
                hops: self.hops_across_local_boundary(
                    received_hops,
                    source_interface,
                    switch.fire_on,
                ),
                transport_id: None,
                address,
                context,
            },
            fire_on: switch.fire_on,
        }
    }

    fn ingest_transported_link_proof<'p>(
        &mut self,
        link_id: &LinkId,
        payload: &'p [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let accepted = match self.accept_transported_link_proof(
            link_id,
            payload,
            received_hops,
            source_interface,
            arrived_at,
        ) {
            Ok(accepted) => accepted,
            Err(reason) => return IngestPacketOutcome::Ignored(reason),
        };
        self.mark_interface_dirty(accepted.next_hop_interface);
        self.mark_interface_dirty(accepted.received_interface);
        match accepted.route_update {
            TransportedRouteUpdate::Unchanged => {}
            TransportedRouteUpdate::RebalancedTo(hops) => {
                let mut handle = accepted.route_evidence;
                if self
                    .routing_table
                    .resolve_route_evidence(&mut handle)
                    .is_some()
                {
                    self.routing_table
                        .rebalance_hops(&accepted.destination, hops);
                }
            }
        }
        let mut route_evidence = accepted.route_evidence;
        self.routing_table
            .apply_route_evidence(&mut route_evidence, arrived_at);
        IngestPacketOutcome::Forward(PacketToForward {
            header: WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: DestinationType::Link,
                packet_type: PacketType::Proof,
                hops: self.hops_across_local_boundary(
                    received_hops,
                    source_interface,
                    accepted.switch.fire_on,
                ),
                transport_id: None,
                address: link_id.to_address(),
                context: WireContext::LinkRequestProof,
            },
            payload,
            fire_on: accepted.switch.fire_on,
        })
    }

    fn accept_transported_link_proof(
        &mut self,
        link_id: &LinkId,
        payload: &[u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> Result<AcceptedTransportedLinkProof, IgnoreReason> {
        let entry = self
            .transported_links
            .entry_for(link_id)
            .ok_or(IgnoreReason::UnknownLink)?;
        let destination = entry.destination;
        let mode = entry.mode;
        let next_hop_interface = entry.next_hop_interface;
        let received_interface = entry.received_interface;
        let route_evidence = entry.route_evidence;
        let expected_hops = entry.remaining_hops;
        let already_validated = entry.validated_by_proof;
        if payload.len() != LINK_PROOF_BODY_LEN && payload.len() != SIGNALLED_LINK_PROOF_LEN {
            return Err(IgnoreReason::Malformed);
        }
        if already_validated || source_interface != next_hop_interface {
            return Err(IgnoreReason::ProofInvalid);
        }
        // Intentional deviation from RNS 1.4.2: the reference accepts an exact-hop LRPROOF by
        // shape. Prns unlocks later opaque switching, and credits route activity, only after the
        // responder's signature verifies for both exact and rebalanced hop counts.
        let stored = self
            .routing_table
            .stored_announce_for(&destination)
            .ok_or(IgnoreReason::UnknownIdentity)?;
        let responder_signing = *stored.announce.public_keys.signing.as_ed25519();
        let proof = link_proof_from(link_id, payload, &responder_signing)
            .map_err(|_| IgnoreReason::ProofInvalid)?;
        if proof.mode != mode {
            return Err(IgnoreReason::ProofInvalid);
        }
        if received_hops == expected_hops {
            let switch = self
                .transported_links
                .validate_by_proof(link_id, source_interface, received_hops, arrived_at)
                .map_err(|_| IgnoreReason::ProofInvalid)?;
            return Ok(AcceptedTransportedLinkProof {
                switch,
                destination,
                next_hop_interface,
                received_interface,
                route_evidence,
                route_update: TransportedRouteUpdate::Unchanged,
            });
        }
        let switch = self
            .transported_links
            .rebalance_and_validate_by_proof(link_id, source_interface, received_hops, arrived_at)
            .map_err(|_| IgnoreReason::ProofInvalid)?;
        Ok(AcceptedTransportedLinkProof {
            switch,
            destination,
            next_hop_interface,
            received_interface,
            route_evidence,
            route_update: TransportedRouteUpdate::RebalancedTo(received_hops),
        })
    }

    fn ingest_transported_link_request(
        &mut self,
        header: &WirePacketHeader,
        request: &LinkRequest,
        arrival: LinkRequestArrival<'_>,
    ) -> IngestPacketOutcome<'static> {
        let routed_through_us =
            self.network_transport_enabled() && header.transport_id == self.transport_id();

        let is_local_client_transit = arrival.source_interface.kind()
            == Some(InterfaceKind::LocalClient)
            || self.routes_via_local_client(&request.destination);

        if !routed_through_us && !is_local_client_transit {
            return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs);
        }

        match self.packet_hash_history.remember(arrival.packet_hash) {
            RememberPacketOutcome::AlreadyKnown => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        let Some(route) = self
            .routing_table
            .forwarding_route_for(&request.destination)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::NoRoute);
        };
        let Some(route_evidence) = self
            .routing_table
            .route_evidence_handle_for(&request.destination)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::NoRoute);
        };
        let fire_on = route.receiving_interface;
        let remaining_hops = route.hops.0;
        let forwarded_hops = self.hops_across_local_boundary(
            arrival.received_hops,
            arrival.source_interface,
            fire_on,
        );
        let forwarded_header = if remaining_hops > 1 {
            let NextHop::Via(next) = route.next_hop else {
                return IngestPacketOutcome::Ignored(IgnoreReason::NoRoute);
            };
            WirePacketHeader {
                hops: forwarded_hops,
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
                hops: forwarded_hops,
                transport_id: None,
                address: header.address,
                context: header.context,
            }
        };

        let maybe_arrival_hw_mtu = arrival
            .interfaces
            .descriptor_for(arrival.source_interface)
            .and_then(|c| c.hardware_mtu);
        let maybe_outbound_hw_mtu = arrival
            .interfaces
            .descriptor_for(fire_on)
            .and_then(|c| c.hardware_mtu);
        let mut body = ForwardedLinkRequestBody {
            bytes: [0u8; SIGNALLED_LINK_REQUEST_LEN],
            len: LINK_REQUEST_KEYS_LEN,
        };
        body.bytes[..X25519PublicKey::LEN].copy_from_slice(&request.initiator_encryption.0);
        body.bytes[X25519PublicKey::LEN..LINK_REQUEST_KEYS_LEN]
            .copy_from_slice(&request.initiator_signing.0);
        if request.signalled {
            if let Some(outbound_hw) = maybe_outbound_hw_mtu {
                let clamped = request
                    .mtu
                    .min(outbound_hw)
                    .min(maybe_arrival_hw_mtu.unwrap_or(usize::MAX));
                body.bytes[LINK_REQUEST_KEYS_LEN..SIGNALLED_LINK_REQUEST_LEN]
                    .copy_from_slice(&signalling_bytes_from(clamped, request.mode));
                body.len = SIGNALLED_LINK_REQUEST_LEN;
            }
        }

        let extra_proof_allowance = arrival
            .interfaces
            .descriptor_for(arrival.source_interface)
            .map(|c| extra_link_proof_timeout_ms(c.bitrate))
            .unwrap_or(0);
        let proof_timeout = InstantMillis(
            arrival
                .arrived_at
                .0
                .saturating_add(extra_proof_allowance)
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(remaining_hops.max(1))),
                ),
        );
        match self.transported_links.track(TransportedLink {
            link_id: request.link_id,
            destination: request.destination,
            route_evidence,
            mode: request.mode,
            next_hop: match route.next_hop {
                NextHop::Via(next) => Some(next),
                NextHop::Direct => None,
            },
            next_hop_interface: fire_on,
            received_interface: arrival.source_interface,
            taken_hops: arrival.received_hops,
            remaining_hops,
            validated_by_proof: false,
            last_active: arrival.arrived_at,
            proof_timeout,
        }) {
            Ok(()) => {}
            Err(TrackTransportedLinkError::AlreadyTracked) => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            Err(TrackTransportedLinkError::TableFull) => {
                return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted)
            }
        }
        IngestPacketOutcome::TransportedLinkRequest {
            header: forwarded_header,
            body,
            fire_on,
        }
    }

    pub(super) fn ingest_link_proof<'p>(
        &mut self,
        link_id: LinkId,
        payload: &'p [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        deferred: Option<&mut DeferredCrypto>,
    ) -> IngestPacketOutcome<'p> {
        let Some(LinkPhase::Pending {
            destination: link_destination,
            mode,
            requested_at,
            command_id,
            initiator_secret,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return self.ingest_transported_link_proof(
                &link_id,
                payload,
                received_hops,
                source_interface,
                arrived_at,
            );
        };
        let Some(stored) = self.routing_table.stored_announce_for(link_destination) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnknownIdentity);
        };
        let responder_signing = *stored.announce.public_keys.signing.as_ed25519();
        let requested_at = *requested_at;
        let command_id = *command_id;
        let mode = *mode;
        if let Some(deferred) = deferred {
            let Ok(parsed) = link_proof_parse(&link_id, payload, &responder_signing) else {
                return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
            };
            if parsed.proof.mode != mode {
                return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
            }
            *deferred = DeferredCrypto::LinkProofVerify(LinkProofVerifyOwed {
                link_id,
                source_interface,
                received_hops,
                responder_encryption: parsed.proof.responder_encryption,
                responder_signing,
                initiator_secret: initiator_secret.cloned(),
                command_id,
                arrived_at,
                rtt: RttMillis::measured_between(requested_at, arrived_at),
                mtu: if parsed.proof.mtu == 0 {
                    BROADCAST_MTU
                } else {
                    parsed.proof.mtu
                },
                signed_data: parsed.signed_data,
                signed_bytes: parsed.signed_bytes,
                signature: parsed.signature,
            });
            return IngestPacketOutcome::OwesLinkProofVerify;
        }
        let Ok(proof) = link_proof_from(&link_id, payload, &responder_signing) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        };
        if proof.mode != mode {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        }
        IngestPacketOutcome::OwesLinkRtt(LinkRttOwed {
            link_id,
            received_hops,
            responder_encryption: proof.responder_encryption,
            responder_signing,
            command_id,
            arrived_at,
            rtt: RttMillis::measured_between(requested_at, arrived_at),
            mtu: if proof.mtu == 0 {
                BROADCAST_MTU
            } else {
                proof.mtu
            },
        })
    }

    pub(super) fn ingest_link_rtt(
        &mut self,
        link_id: LinkId,
        payload: &[u8],
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let Some(LinkPhase::Handshake {
            key, requested_at, ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let reported = match link_rtt_from(&link_id, payload, key) {
            Ok(reported) => reported,
            Err(LinkRttError::Malformed) => {
                return IngestPacketOutcome::OwesLinkClose {
                    link_id,
                    reason: LinkClosedReason::MalformedRtt,
                };
            }
            Err(e) => return IngestPacketOutcome::Ignored(IgnoreReason::LinkRttError(e)),
        };
        let measured = RttMillis::measured_between(*requested_at, arrived_at);
        let rtt = measured.max(reported.rtt);
        let Ok(destination) =
            self.links
                .activate_responding(&link_id, rtt, source_interface, arrived_at)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        self.mark_interface_dirty(source_interface);
        let default_strategy = self
            .upstream_app_destinations
            .default_resource_strategy(&destination);
        let _ = self.links.set_resource_strategy(&link_id, default_strategy);
        IngestPacketOutcome::LinkActivated {
            link_id,
            rtt_millis: rtt.millis(),
        }
    }

    fn remember_link_data_packet(&mut self, packet_hash: PacketHash) -> Option<PacketHash> {
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => None,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {
                Some(packet_hash)
            }
        }
    }

    pub(super) fn ingest_link_data<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        if !matches!(
            self.links.phase_for(&link_id),
            Some(LinkPhase::Active { .. }),
        ) {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        }

        let Some(packet_hash) = self.remember_link_data_packet(packet_hash) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate);
        };

        let Some(LinkPhase::Active { key, role, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
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
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
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

    pub(super) fn ingest_channel_data<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok(envelope) = parse_envelope(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
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

    pub(super) fn ingest_link_identify(
        &mut self,
        data: DataPacket<'_>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(LinkPhase::Active {
            key,
            role: LinkRole::Responder { .. },
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let Some(identity) = peer_identity_from(&link_id, plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        };
        self.links.note_identified(&link_id, identity);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::PeerIdentified { link_id, identity }
    }

    pub(super) fn ingest_request_over_link<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(packet_hash) = self.remember_link_data_packet(packet_hash) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate);
        };
        let Some(LinkPhase::Active {
            key,
            role: LinkRole::Responder { destination, .. },
            remote_identity,
            rtt,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let destination = *destination;
        let remote_identity = *remote_identity;
        let request_rtt = *rtt;
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let plaintext: &'p [u8] = plaintext;
        let maximum_request_bytes = self
            .upstream_app_destinations
            .lookup_single(&destination)
            .map(|registered| registered.maximum_request_bytes)
            .unwrap_or_default();
        if !maximum_request_bytes.allows(plaintext.len() as u64) {
            return IngestPacketOutcome::Ignored(IgnoreReason::RequestTooLarge);
        }
        let Ok(parsed) = parse_request_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        if !self
            .request_handlers
            .permits(&destination, &parsed.path_hash, remote_identity.as_ref())
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::PermissionDenied);
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::RequestReceived {
            destination,
            link_id,
            request_id: RequestId::of_packet(&packet_hash),
            requester: remote_identity,
            path_hash: parsed.path_hash,
            requested_at: parsed.requested_at,
            rtt: request_rtt,
            data: parsed.data,
        }
    }

    pub(super) fn ingest_response_over_link<'p>(
        &mut self,
        data: DataPacket<'p>,
        packet_hash: PacketHash,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        if self.remember_link_data_packet(packet_hash).is_none() {
            return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate);
        }
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok((request_id, response_data)) = parse_response_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let Some(maximum_response_bytes) = self.receipts.pending_request_response_limit(request_id)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        let response_size = response_data.len().saturating_sub(2) as u64;
        if !maximum_response_bytes.allows(response_size) {
            let Some(proven) = self.receipts.settle_by_request_id(request_id) else {
                return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
            };
            self.links.note_inbound(&link_id, arrived_at);
            return IngestPacketOutcome::ResponseTooLarge {
                id: proven.command_id,
                link_id,
                request_id,
            };
        }
        let Some(proven) = self.receipts.settle_by_request_id(request_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResponseSettled {
            id: proven.command_id,
            delivered: PacketReceiptDelivered {
                rtt: RttMillis::measured_between(proven.sent_at, arrived_at),
                evidence: DeliveryEvidence::Response,
            },
            link_id,
            request_id,
            data: response_data,
        }
    }

    pub(super) fn ingest_keepalive(
        &mut self,
        link_id: LinkId,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let &[byte] = payload else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let Some(LinkPhase::Active { role, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch);
        };
        match (role, byte) {
            (LinkRole::Responder { .. }, KEEPALIVE_REQUEST) => {
                let echo_due = self.links.keepalive_echo_due(&link_id, arrived_at);
                self.links.note_inbound(&link_id, arrived_at);
                if echo_due {
                    IngestPacketOutcome::OwesKeepaliveEcho { link_id }
                } else {
                    IngestPacketOutcome::Ignored(IgnoreReason::Consumed)
                }
            }
            (LinkRole::Initiator { .. } | LinkRole::Responder { .. }, KEEPALIVE_ECHO) => {
                self.links.note_inbound(&link_id, arrived_at);
                IngestPacketOutcome::Ignored(IgnoreReason::Consumed)
            }
            _ => IngestPacketOutcome::Ignored(IgnoreReason::Malformed),
        }
    }

    pub(super) fn ingest_link_close(
        &mut self,
        data: DataPacket<'_>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::from_address(data.header.address);
        let (key, attached_interface) = match self.links.phase_for(&link_id) {
            Some(LinkPhase::Active {
                key,
                attached_interface,
                ..
            }) => (key, Some(*attached_interface)),
            Some(LinkPhase::Handshake { key, .. }) => (key, None),
            Some(LinkPhase::Pending { .. }) | None => {
                return IngestPacketOutcome::Ignored(IgnoreReason::LinkPhaseMismatch)
            }
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::DecryptFailed);
        };
        if plaintext != link_id.as_bytes() {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        }
        self.links.note_inbound(&link_id, arrived_at);
        self.reconcile_pending_link_route_evidence();
        self.links.remove(&link_id);
        self.channels.close(&link_id);
        self.pending_resource_offers.remove_link(&link_id);
        self.incoming_assemblies.clear(&link_id);
        self.outgoing_assemblies.clear(&link_id);
        if let Some(interface) = attached_interface {
            self.mark_interface_dirty(interface);
        }
        IngestPacketOutcome::LinkClosedByPeer { link_id }
    }

    pub(super) fn ingest_link_request(
        &mut self,
        header: &WirePacketHeader,
        payload: &[u8],
        arrival: LinkRequestArrival<'_>,
    ) -> IngestPacketOutcome<'static> {
        if header.destination_type != DestinationType::Single {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        let Ok(request) = link_request_from(header, payload) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        };
        let Some(registered) = self
            .upstream_app_destinations
            .lookup_single(&request.destination)
        else {
            return self.ingest_transported_link_request(header, &request, arrival);
        };
        if let Some(transport_id) = header.transport_id {
            if self.transport_id() != Some(transport_id) {
                return IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance);
            }
        }
        match registered.link_request_policy {
            LinkRequestPolicy::AcceptNone => {
                return IngestPacketOutcome::Ignored(IgnoreReason::LinkRequestsRefused)
            }
            LinkRequestPolicy::AcceptAll => {}
        }
        if self.held_identities.get(&registered.identity).is_none() {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnknownIdentity);
        }

        match self.packet_hash_history.remember(arrival.packet_hash) {
            RememberPacketOutcome::AlreadyKnown => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        IngestPacketOutcome::OwesLinkProof(AcceptedLinkRequest {
            request,
            identity: registered.identity,
            proof_strategy: registered.proof_strategy,
            received_hops: arrival.received_hops,
            arrived_at: arrival.arrived_at,
        })
    }
}
