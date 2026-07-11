use super::*;
use crate::interfaces::AttachedInterfaces;

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
    Duplicate,
    NotTransportedByUs,
}

// RNS `Transport.packet_filter`.
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

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_link_addressed<'p>(
        &mut self,
        data: DataPacket<'p>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        match self.relay_if_transported(
            data.header.address,
            data.header.context,
            data.payload,
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
            RelayOutcome::Duplicate => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
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
            WireContext::None => self.ingest_link_data(data, source_interface, arrived_at),
            WireContext::KeepAlive => self.ingest_keepalive(link_id, data.payload, arrived_at),
            WireContext::LinkClose => self.ingest_link_close(data),
            WireContext::LinkIdentify => self.ingest_link_identify(data, arrived_at),
            WireContext::Request => self.ingest_request_over_link(data, arrived_at),
            WireContext::Response => self.ingest_response_over_link(data, arrived_at),
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
            WireContext::Channel => self.ingest_channel_data(data, arrived_at),
            // Not an active link's data: proofs travel as Proof packets (dispatched separately); the rest are transport/announce contexts or unrecognized bytes.
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn relay_if_transported(
        &mut self,
        address: WireAddress,
        context: WireContext,
        payload: &[u8],
        packet_type: PacketType,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> RelayOutcome {
        let link_id = LinkId::from_address(address);
        if self.links.has_local_link(&link_id) || context == WireContext::LinkRequestProof {
            return RelayOutcome::NotTransportedByUs;
        }
        let Ok(switch) =
            self.transported_links
                .switch(&link_id, source_interface, received_hops, arrived_at)
        else {
            return RelayOutcome::NotTransportedByUs;
        };
        if !switch_exempt_from_duplicate_filter(context) {
            let packet_hash = PacketHash::of_fields(
                DestinationType::Link,
                packet_type,
                &address,
                context,
                payload,
            );
            match self.packet_hash_history.remember(packet_hash) {
                RememberPacketOutcome::AlreadyKnown => return RelayOutcome::Duplicate,
                RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {
                }
            }
        }
        RelayOutcome::Forward {
            header: WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: DestinationType::Link,
                packet_type,
                hops: received_hops,
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
        let Some(entry) = self.transported_links.entry_for(link_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnknownLink);
        };
        let destination = entry.destination;
        let next_hop_interface = entry.next_hop_interface;
        let received_interface = entry.received_interface;
        let Some(stored) = self.routing_table.stored_announce_for(&destination) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnknownIdentity);
        };
        let responder_signing = *stored.announce.public_keys.signing.as_ed25519();
        if link_proof_from(link_id, payload, &responder_signing).is_err() {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        }
        let Ok(switch) = self.transported_links.validate_by_proof(
            link_id,
            source_interface,
            received_hops,
            arrived_at,
        ) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
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
                address: link_id.to_address(),
                context: WireContext::LinkRequestProof,
            },
            payload,
            fire_on: switch.fire_on,
        })
    }

    fn ingest_transported_link_request(
        &mut self,
        header: &WirePacketHeader,
        request: &LinkRequest,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
    ) -> IngestPacketOutcome<'static> {
        let addressed_through_us =
            self.transport_id.is_some() && header.transport_id == self.transport_id;
        let local_client_transit = source_interface.kind() == Some(InterfaceKind::LocalClient)
            || self.routes_via_local_client(&request.destination);
        if !addressed_through_us && !local_client_transit {
            return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs);
        }
        let Some(route) = self
            .routing_table
            .forwarding_route_for(&request.destination)
        else {
            return IngestPacketOutcome::Ignored(IgnoreReason::NoRoute);
        };
        let fire_on = route.receiving_interface;
        let remaining_hops = route.hops.0;
        let forwarded_header = if remaining_hops > 1 {
            let NextHop::Via(next) = route.next_hop else {
                return IngestPacketOutcome::Ignored(IgnoreReason::NoRoute);
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
                address: header.address,
                context: header.context,
            }
        };

        let maybe_arrival_hw_mtu = interfaces
            .descriptor_for(source_interface)
            .and_then(|c| c.hardware_mtu);
        let maybe_outbound_hw_mtu = interfaces
            .descriptor_for(fire_on)
            .and_then(|c| c.hardware_mtu);
        let mut body = ForwardedLinkRequestBody {
            bytes: [0u8; SIGNALLED_LINK_REQUEST_LEN],
            len: LINK_REQUEST_KEYS_LEN,
        };
        body.bytes[..32].copy_from_slice(&request.initiator_encryption.0);
        body.bytes[32..LINK_REQUEST_KEYS_LEN].copy_from_slice(&request.initiator_signing.0);
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

        let extra_proof_allowance = interfaces
            .descriptor_for(source_interface)
            .map(|c| extra_link_proof_timeout_ms(c.bitrate))
            .unwrap_or(0);
        let proof_timeout = InstantMillis(
            arrived_at
                .0
                .saturating_add(extra_proof_allowance)
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(remaining_hops.max(1))),
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
                remaining_hops,
                validated_by_proof: false,
                last_active: arrived_at,
                proof_timeout,
            })
            .is_err()
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted);
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
        if let Some(deferred) = deferred {
            let Ok(parsed) = link_proof_parse(&link_id, payload, &responder_signing) else {
                return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
            };
            *deferred = DeferredCrypto::LinkProofVerify(LinkProofVerifyOwed {
                link_id,
                source_interface,
                responder_encryption: parsed.proof.responder_encryption,
                responder_signing,
                initiator_secret: initiator_secret.cloned(),
                command_id,
                rtt: RttMillis::measured_between(requested_at, arrived_at),
                mtu: if parsed.proof.mtu == 0 {
                    BROADCAST_MTU
                } else {
                    parsed.proof.mtu
                },
                signed_data: parsed.signed_data,
                signed_len: parsed.signed_len,
                signature: parsed.signature,
            });
            return IngestPacketOutcome::OwesLinkProofVerify;
        }
        let Ok(proof) = link_proof_from(&link_id, payload, &responder_signing) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid);
        };
        IngestPacketOutcome::OwesLinkRtt(LinkRttOwed {
            link_id,
            responder_encryption: proof.responder_encryption,
            responder_signing,
            command_id,
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
            rtt_ms: rtt.millis(),
        }
    }

    fn remember_link_data_packet(
        &mut self,
        address: &WireAddress,
        context: WireContext,
        payload: &[u8],
    ) -> Option<PacketHash> {
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            address,
            context,
            payload,
        );
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

        let Some(packet_hash) =
            self.remember_link_data_packet(&data.header.address, data.header.context, data.payload)
        else {
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
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.header.address,
            data.header.context,
            data.payload,
        );
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
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        let Some(packet_hash) =
            self.remember_link_data_packet(&data.header.address, data.header.context, data.payload)
        else {
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
            link_id,
            request_id: RequestId::of_packet(&packet_hash),
            path_hash: parsed.path_hash,
            requested_at: parsed.requested_at,
            rtt: request_rtt,
            data: parsed.data,
        }
    }

    pub(super) fn ingest_response_over_link<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::from_address(data.header.address);
        if self
            .remember_link_data_packet(&data.header.address, data.header.context, data.payload)
            .is_none()
        {
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
        let Some(proven) = self.receipts.settle_by_request_id(request_id) else {
            return IngestPacketOutcome::Ignored(IgnoreReason::Superseded);
        };
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResponseSettled {
            id: proven.command_id,
            delivered: PacketReceiptDelivered {
                rtt: RttMillis::measured_between(proven.sent_at, arrived_at),
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
                self.links.note_inbound(&link_id, arrived_at);
                IngestPacketOutcome::OwesKeepaliveEcho { link_id }
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
        self.links.remove(&link_id);
        self.channels.close(&link_id);
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
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
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
            return self.ingest_transported_link_request(
                header,
                &request,
                received_hops,
                source_interface,
                arrived_at,
                interfaces,
            );
        };
        if self.held_identities.get(&registered.identity).is_none() {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnknownIdentity);
        }

        let packet_hash = PacketHash::of_fields(
            DestinationType::Single,
            PacketType::LinkRequest,
            &request.destination.to_address(),
            header.context,
            payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => {
                return IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
            }
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        IngestPacketOutcome::OwesLinkProof(AcceptedLinkRequest {
            request,
            identity: registered.identity,
            proof_strategy: registered.proof_strategy,
            received_hops,
            arrived_at,
        })
    }
}
