use super::*;

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

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5 `Transport.inbound`'s LINKREQUEST-in-transport arm: a request addressed
    /// through us toward a routed destination books a transported row and forwards,
    /// re-headered for its remaining distance, its MTU signalling clamped to what this
    /// path segment can carry. The LRPROOF arm: the relay validates the proof itself
    /// against the announced identity it holds (over the right side, at the right
    /// distance), marks the row live, and sends it on toward the initiator.
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

    pub(super) fn classify_link_proof<'p>(
        &mut self,
        destination: &DestinationHash,
        payload: &'p [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        deferred: Option<&mut DeferredCrypto>,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*destination.as_bytes());
        let Some(LinkPhase::Pending {
            destination: link_destination,
            requested_at,
            command_id,
            initiator_secret,
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
        let requested_at = *requested_at;
        let command_id = *command_id;
        if let Some(deferred) = deferred {
            let Ok(parsed) = link_proof_parse(&link_id, payload, &responder_signing) else {
                return IngestPacketOutcome::Ignored;
            };
            deferred.link_proof_verify = Some(LinkProofVerifyOwed {
                link_id,
                source_interface,
                responder_encryption: parsed.proof.responder_encryption,
                responder_signing,
                initiator_secret: initiator_secret.cloned(),
                command_id,
                rtt: Rtt::measured_between(requested_at, arrived_at),
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
            return IngestPacketOutcome::Ignored;
        };
        IngestPacketOutcome::OwesLinkRtt(LinkRttOwed {
            link_id,
            responder_encryption: proof.responder_encryption,
            responder_signing,
            command_id,
            rtt: Rtt::measured_between(requested_at, arrived_at),
            mtu: if proof.mtu == 0 {
                BROADCAST_MTU
            } else {
                proof.mtu
            },
        })
    }

    pub(super) fn classify_link_rtt(
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

    pub(super) fn classify_link_data<'p>(
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

    /// RNS 1.3.5 `Link.receive`'s CHANNEL branch: channel packets carry the protocol's own
    /// sequence dedup, so the packet-hash duplicate filter is skipped (a byte-identical
    /// retransmit must reach the receive algorithm to be re-acked, exactly as RNS exempts
    /// CHANNEL from `packet_filter`). The hash is still taken, over the ciphertext before
    /// the in-place open, for the ack the arrival unconditionally owes.
    pub(super) fn classify_channel_data<'p>(
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

    pub(super) fn classify_link_identify(
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

    pub(super) fn classify_request_over_link<'p>(
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

    pub(super) fn classify_response_over_link<'p>(
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
            delivered: PacketReceiptDelivered {
                rtt: Rtt::measured_between(proven.sent_at, arrived_at),
            },
            link_id,
            request_id,
            data: response_data,
        }
    }

    pub(super) fn classify_keepalive(
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

    pub(super) fn classify_link_close(
        &mut self,
        data: DataPacket<'_>,
    ) -> IngestPacketOutcome<'static> {
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

    pub(super) fn classify_link_request(
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

        IngestPacketOutcome::OwesLinkProof(AcceptedLinkRequest {
            request,
            identity,
            proof_strategy,
            received_hops,
            arrived_at,
        })
    }
}
