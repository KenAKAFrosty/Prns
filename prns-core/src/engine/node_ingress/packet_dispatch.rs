use super::delivery::DeliveryIo;
use super::relay::{RelayAudience, RelayPathRequest};

use crate::crypto::X25519SecretKey;
use crate::engine::remote_control_pairing::{
    RemoteControlPairingRequestIngress, RemoteControlPairingRequestIngressOutcome,
};
use crate::engine::settlement::settle;
use crate::engine::LinkClosedReason;
use crate::engine::{
    CryptoOwed, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LinkEstablished, OwedWork, ProtocolViolationKind,
    RemoteControlControllerPairingRequestFailureCause,
    RemoteControlControllerPairingResponseArrival, RemoteControlControllerPairingResponseReceived,
    SendRequestFailure, SendRequestIntent, Settlement, WakeSchedule, WakeSchedules,
};
use crate::identity::{IdentitySigner, ENCRYPTION_IV_LEN};
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{Egress, InboundPacket};
use crate::routing::ingress::{ClassifiedInboundPacket, IngestEffects};
use crate::routing::links::channel::receive::receive as channel_receive;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::handshake::{negotiated_link_mtu, LinkProofSignOwed};
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::links::resources::receive::gate::AcceptedResourceAdmission;
use crate::routing::links::resources::ResourceOffer;
use crate::routing::proof::ProofRequest;
use crate::storage::StorageLayout;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN};

pub struct IngestIo<'a, FillEntropy, OnProofRequest, OnResourceOffer, Sink>
where
    FillEntropy: FnMut(&mut [u8]),
    OnProofRequest: FnMut(&ProofRequest) -> bool,
    OnResourceOffer: FnMut(&ResourceOffer) -> bool,
    Sink: FnMut(EngineReaction<'_, OwedWork<'_>>),
{
    pub interfaces: AttachedInterfaces<'a>,
    pub now: InstantMillis,
    pub fill_random: &'a mut FillEntropy,
    pub should_prove: &'a mut OnProofRequest,
    pub should_accept_resource: &'a mut OnResourceOffer,
    pub sink: &'a mut Sink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestPacketReport {
    pub wake_schedules: WakeSchedules,
    pub protocol_violation: Option<ProtocolViolationKind>,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_packet_into<F, P, A, K>(
        &mut self,
        packet: InboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_, OwedWork<'_>>),
    {
        self.ingest_packet_into_report(packet, io).wake_schedules
    }

    pub fn ingest_packet_into_report<F, P, A, K>(
        &mut self,
        packet: InboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
    ) -> IngestPacketReport
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_, OwedWork<'_>>),
    {
        self.ingest_classified_into_report(ClassifiedInboundPacket::classify(packet), io)
    }

    pub fn ingest_classified_into<F, P, A, K>(
        &mut self,
        packet: ClassifiedInboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_, OwedWork<'_>>),
    {
        self.ingest_classified_into_report(packet, io)
            .wake_schedules
    }

    pub fn ingest_classified_into_report<F, P, A, K>(
        &mut self,
        packet: ClassifiedInboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
    ) -> IngestPacketReport
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_, OwedWork<'_>>),
    {
        let IngestIo {
            interfaces,
            now,
            fill_random,
            should_prove,
            should_accept_resource,
            sink,
        } = io;
        let (source, ingress) = packet.into_parts();
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let mut effects = IngestEffects::default();
        let outcome = self.ingest_classified_with_effects(ingress, interfaces, &mut effects);

        //Consider cfg-gating this on metrics/observability?
        let protocol_violation = ProtocolViolationKind::of_outcome(&outcome);

        wake_schedule_changes.held_announce_release = effects.held_announce_release;
        let accepted_observation = effects.accepted_announce.take();
        let remote_control_pairing_availability =
            effects.remote_control_pairing_availability.take();
        if let Some(expiry) = effects.destination_identity_expiry {
            wake_schedule_changes.expired_destination_identities = WakeSchedule::AtMost(expiry);
        }
        if let Some(observation) = remote_control_pairing_availability {
            self.apply_remote_control_pairing_availability(
                observation,
                interfaces,
                &mut wake_schedule_changes,
                sink,
            );
        }
        match outcome {
            IngestPacketOutcome::Announce(ingest) => {
                self.apply_announce_ingest(
                    ingest,
                    accepted_observation,
                    source,
                    interfaces,
                    &mut wake_schedule_changes,
                    sink,
                );
            }
            IngestPacketOutcome::Delivery { delivery, proof } => {
                self.process_delivery(
                    delivery,
                    proof,
                    source,
                    now,
                    &mut DeliveryIo {
                        interfaces,
                        should_prove: &mut *should_prove,
                        sink: &mut *sink,
                    },
                );
            }
            IngestPacketOutcome::OwesDecrypt(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::Decrypt(owed)),
                )));
            }
            IngestPacketOutcome::OwesRatchetDecrypt(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::RatchetDecrypt(owed)),
                )));
            }
            IngestPacketOutcome::OwesAnnounceVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::AnnounceVerify(owed)),
                )));
            }
            IngestPacketOutcome::OwesRemoteControlPairingAvailabilityVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::RemoteControlPairingAvailabilityVerify(owed)),
                )));
            }
            IngestPacketOutcome::OwesReceiptProofVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::ReceiptProofVerify(owed)),
                )));
            }
            IngestPacketOutcome::OwesChannelAckVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::ChannelAckVerify(owed)),
                )));
            }
            IngestPacketOutcome::OwesLinkIdentityVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::LinkIdentityVerify(owed)),
                )));
            }
            IngestPacketOutcome::OwesTunnelSynthesizeVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::TunnelSynthesizeVerify(owed)),
                )));
            }
            IngestPacketOutcome::ReceiptProofIgnored => {}
            IngestPacketOutcome::TransportedLinkRequest {
                header,
                body,
                fire_on,
            } => {
                if interfaces.is_egress_eligible(fire_on, Egress::Transport) {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(header_len) = header.write(&mut buf) {
                        let wire_bytes = header_len + body.len;
                        buf[header_len..wire_bytes].copy_from_slice(body.as_bytes());
                        sink(EngineReaction::Directive(Directive::Send {
                            target: fire_on,
                            bytes: &buf[..wire_bytes],
                        }));
                    }
                }
            }
            IngestPacketOutcome::Forward(forward) => {
                if interfaces.is_egress_eligible(forward.fire_on, Egress::Transport) {
                    let size_hint = HEADER_MAX_LEN + forward.payload.len();
                    let mut fill = |slot: &mut [u8]| forward.to_wire(slot).ok();
                    sink(EngineReaction::Directive(Directive::EmitFrame {
                        target: forward.fire_on,
                        size_hint,
                        fill: &mut fill,
                    }));
                }
            }
            IngestPacketOutcome::AnswerPathRequest { destination } => {
                if interfaces.is_egress_eligible(source, Egress::Transmit) {
                    if let Ok(owed) = self.prepare_path_response_announce_sign(
                        &destination,
                        source,
                        now,
                        &mut *fill_random,
                    ) {
                        sink(EngineReaction::Directive(Directive::Fulfill(
                            OwedWork::Crypto(CryptoOwed::AnnounceSign(owed)),
                        )));
                    }
                }
            }
            IngestPacketOutcome::ScheduledPathResponse { .. } => {
                wake_schedule_changes.scheduled_announces = self.scheduled_announces_wake();
            }
            IngestPacketOutcome::PathResponseScheduleRejected {
                rejection: _rejection,
                ..
            } => {
                let _reason = match _rejection {
                    crate::routing::announce::schedule::ScheduleRejection::QueueFull => {
                        crate::routing::ingress::IgnoreReason::CapacityExhausted
                    }
                };
                #[cfg(feature = "runtime-metrics")]
                self.ignored_packet_counts.record(_reason);
            }
            IngestPacketOutcome::ForwardRecursivePathRequest { destination, id } => {
                self.relay_path_request(
                    RelayPathRequest {
                        destination,
                        id: &id,
                    },
                    source,
                    interfaces,
                    RelayAudience::OnlineNetworkInterfaces,
                    now,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::ForwardBoundaryPathRequest { destination, id } => {
                self.relay_path_request(
                    RelayPathRequest {
                        destination,
                        id: &id,
                    },
                    source,
                    interfaces,
                    RelayAudience::BoundaryAndGateway,
                    now,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::ForwardLocalClientPathRequest { destination, id } => {
                self.relay_path_request(
                    RelayPathRequest {
                        destination,
                        id: &id,
                    },
                    source,
                    interfaces,
                    RelayAudience::AllNetworkInterfaces,
                    now,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::RelayPathRequestToLocalClients { destination, id } => {
                self.relay_path_request(
                    RelayPathRequest {
                        destination,
                        id: &id,
                    },
                    source,
                    interfaces,
                    RelayAudience::LocalClients,
                    now,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::OwesLinkRtt(owed) => {
                self.process_owes_link_rtt(owed, source, interfaces, now, fill_random, sink);
            }
            IngestPacketOutcome::OwesLinkProofVerify(owed) => {
                sink(EngineReaction::Directive(Directive::Fulfill(
                    OwedWork::Crypto(CryptoOwed::LinkProofVerify(owed)),
                )));
            }
            IngestPacketOutcome::RequestReceived {
                destination,
                link_id,
                request_id,
                requester,
                path_hash,
                requested_at,
                rtt,
                data,
            } => {
                match self.ingest_remote_control_pairing_request(
                    RemoteControlPairingRequestIngress {
                        destination,
                        link_id,
                        request_id,
                        requester,
                        path_hash,
                        data,
                    },
                    interfaces,
                    now,
                    fill_random,
                    sink,
                ) {
                    RemoteControlPairingRequestIngressOutcome::Pairing(_pairing_outcome) => {
                        wake_schedule_changes.remote_control_pairing =
                            self.remote_control_pairing_wake();
                    }
                    RemoteControlPairingRequestIngressOutcome::ForwardToApplication => {
                        sink(EngineReaction::Journaled(Journaled::RequestReceived {
                            destination,
                            link_id,
                            request_id,
                            requester,
                            path_hash,
                            requested_at,
                            rtt,
                            data,
                        }));
                    }
                }
            }
            IngestPacketOutcome::ResponseSettled {
                id,
                intent,
                delivered,
                link_id,
                request_id,
                data,
            } => {
                let settlement = match intent {
                    SendRequestIntent::Application => {
                        sink(EngineReaction::Journaled(Journaled::ResponseReceived {
                            command_id: id,
                            link_id,
                            request_id,
                            data,
                        }));
                        Settlement::SendRequest(Ok(delivered))
                    }
                    SendRequestIntent::RemoteControlControllerPairing => {
                        let admission = self.admit_remote_control_controller_pairing_response(
                            RemoteControlControllerPairingResponseArrival::new(link_id, data),
                            now,
                            sink,
                        );
                        let effect = self
                            .remote_control_controller_pairing_response_effect(link_id, admission);
                        Settlement::RemoteControlControllerPairingRequest(Ok(
                            RemoteControlControllerPairingResponseReceived {
                                delivered,
                                admission,
                                effect,
                            },
                        ))
                    }
                };
                settle(sink, id, settlement);
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                match intent {
                    SendRequestIntent::Application => {}
                    SendRequestIntent::RemoteControlControllerPairing => {
                        wake_schedule_changes.remote_control_pairing =
                            self.remote_control_pairing_wake();
                    }
                }
            }
            IngestPacketOutcome::ResponseTooLarge {
                id,
                intent,
                link_id,
                ..
            } => {
                let settlement = self.failed_send_request_settlement(
                    link_id,
                    intent,
                    SendRequestFailure::ResponseTooLarge,
                );
                settle(sink, id, settlement);
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                match intent {
                    SendRequestIntent::Application => {}
                    SendRequestIntent::RemoteControlControllerPairing => {
                        wake_schedule_changes.remote_control_pairing =
                            self.remote_control_pairing_wake();
                    }
                }
            }
            IngestPacketOutcome::ChannelDataReceived {
                link_id,
                message_type,
                sequence,
                payload,
                packet_hash,
            } => {
                let outcome = channel_receive(
                    &mut self.channels,
                    &link_id,
                    sequence,
                    message_type,
                    payload,
                    |message_type, data| {
                        sink(EngineReaction::Journaled(
                            Journaled::ChannelMessageReceived {
                                link_id,
                                message_type,
                                data,
                            },
                        ));
                    },
                );
                if outcome.owes_proof() && interfaces.is_egress_eligible(source, Egress::Transmit) {
                    if let Ok(owed) = self.prepare_channel_ack_sign(source, &link_id, &packet_hash)
                    {
                        sink(EngineReaction::Directive(Directive::Fulfill(
                            CryptoOwed::ChannelAckSign(owed).into(),
                        )));
                    }
                }
            }
            IngestPacketOutcome::OwesResourceParts(request) => {
                self.serve_resource_request(&request, source, now, fill_random, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                self.emit_resource_pull(&link_id, &hash, now, fill_random, sink);
                self.emit_resource_open(&link_id, &hash, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ResourceOffered {
                link_id,
                original_hash,
                accepted,
            } => {
                let offer = ResourceOffer {
                    link_id,
                    remote_identity: self
                        .links
                        .phase_for(&link_id)
                        .and_then(|phase| match phase {
                            crate::routing::links::table::LinkPhase::Active {
                                remote_identity,
                                ..
                            } => *remote_identity,
                            _ => None,
                        }),
                    hash: accepted.hash,
                    uncompressed_data_bytes: accepted.uncompressed_data_bytes,
                    sealed_transfer_bytes: accepted.sealed_transfer_bytes,
                    part_count: accepted.part_count,
                    segment_index: accepted.segment_index,
                    total_segment_count: accepted.total_segment_count,
                    compression: accepted.compression,
                    has_metadata: accepted.has_metadata,
                };
                if (should_accept_resource)(&offer) {
                    match self.admit_or_queue_accepted_resource(
                        link_id,
                        original_hash,
                        accepted,
                        now,
                    ) {
                        AcceptedResourceAdmission::Pull { link_id, hash } => {
                            self.emit_resource_pull(&link_id, &hash, now, fill_random, sink);
                        }
                        AcceptedResourceAdmission::Pending => {}
                        AcceptedResourceAdmission::CapacityRejected {
                            link_id,
                            hash,
                            settled_request,
                        } => {
                            self.reject_offered_resource(&link_id, &hash, now, fill_random, sink);
                            if let Some(id) = settled_request {
                                settle(
                                    sink,
                                    id,
                                    Settlement::SendRequest(Err(
                                        SendRequestFailure::ResourceCapacity,
                                    )),
                                );
                            }
                        }
                        AcceptedResourceAdmission::Ignored(_) => {}
                    }
                    wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                    wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                } else {
                    self.reject_offered_resource(&link_id, &accepted.hash, now, fill_random, sink);
                }
            }
            IngestPacketOutcome::ResourceTooLarge {
                link_id,
                hash,
                settled_request,
            } => {
                self.reject_offered_resource(&link_id, &hash, now, fill_random, sink);
                if let Some(id) = settled_request {
                    settle(
                        sink,
                        id,
                        Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)),
                    );
                    wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                }
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::PairingResponseResourceUnsupported {
                link_id,
                hash,
                settled_request,
            } => {
                self.reject_offered_resource(&link_id, &hash, now, fill_random, sink);
                let settlement = self.failed_remote_control_controller_pairing_request_settlement(
                    link_id,
                    RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
                );
                settle(sink, settled_request, settlement);
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.remote_control_pairing = self.remote_control_pairing_wake();
            }
            IngestPacketOutcome::ResourceAdmissionPending => {
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceCapacityRejected {
                link_id,
                hash,
                settled_request,
            } => {
                self.reject_offered_resource(&link_id, &hash, now, fill_random, sink);
                if let Some(id) = settled_request {
                    settle(
                        sink,
                        id,
                        Settlement::SendRequest(Err(SendRequestFailure::ResourceCapacity)),
                    );
                    wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                }
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesResourceAssembly { link_id, hash } => {
                // Mark the final ready span in flight before conclusion observes the row. That
                // makes a complete transfer park as AwaitingOpen until its typed completion.
                self.emit_resource_open(&link_id, &hash, sink);
                self.conclude_resource(&link_id, &hash, now, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ResourceDeadlineAdvanced { link_id, hash } => {
                self.emit_resource_open(&link_id, &hash, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::IncomingResourceFailed {
                link_id,
                hash,
                cause,
                settled_request,
            } => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                    cause,
                }));
                if let Some(id) = settled_request {
                    settle(
                        sink,
                        id,
                        Settlement::SendRequest(Err(SendRequestFailure::from(cause))),
                    );
                    wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                }
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceRejectedByPeer {
                id,
                link_id,
                correlation,
            } => {
                settle(
                    sink,
                    id,
                    crate::routing::links::resources::send::resource_settlement(
                        correlation,
                        Err(crate::engine::SendResourceFailure::RejectedByPeer),
                    ),
                );
                self.fail_staged_continuation(&link_id, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceDelivered {
                id,
                link_id,
                correlation,
                last_segment,
            } => {
                if !last_segment
                    && self
                        .outgoing_assemblies
                        .static_continuation(&link_id)
                        .is_some()
                {
                    wake_schedule_changes.merge(self.continue_static_response_into(
                        &link_id,
                        now,
                        fill_random,
                        sink,
                    ));
                } else {
                    settle(
                        sink,
                        id,
                        crate::routing::links::resources::send::resource_settlement(
                            correlation,
                            Ok(()),
                        ),
                    );
                    self.promote_staged_resource(&link_id, now, fill_random, sink);
                }
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::LinkActivated {
                link_id,
                rtt_millis,
            } => {
                sink(EngineReaction::Journaled(Journaled::LinkEstablished(
                    LinkEstablished {
                        link_id,
                        rtt_millis,
                    },
                )));
            }
            IngestPacketOutcome::OwesLinkProof(accepted) => {
                if interfaces.is_egress_eligible(source, Egress::Transmit) {
                    let mut secret_bytes = [0u8; X25519SecretKey::LEN];
                    fill_random(&mut secret_bytes);
                    if let Some(held) = self.held_identities.get(&accepted.identity) {
                        let signing_secret = held.signing_secret_clone();
                        let responder_signing = held.signing_public_key();
                        sink(EngineReaction::Directive(Directive::Fulfill(
                            OwedWork::Crypto(CryptoOwed::LinkProofSign(LinkProofSignOwed {
                                request: accepted.request,
                                identity: accepted.identity,
                                proof_strategy: accepted.proof_strategy,
                                received_hops: accepted.received_hops,
                                arrived_at: accepted.arrived_at,
                                source_interface: source,
                                mtu: negotiated_link_mtu(
                                    accepted.request.mtu,
                                    link_mtu_ceiling(interfaces, source),
                                ),
                                signing_secret,
                                responder_signing,
                                ephemeral_secret: X25519SecretKey::new(secret_bytes),
                            })),
                        )));
                    }
                }
            }
            IngestPacketOutcome::OwesKeepaliveEcho { link_id } => {
                if interfaces.is_egress_eligible(source, Egress::Transmit) {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(written) = write_keepalive(&link_id, KEEPALIVE_ECHO, &mut buf) {
                        self.links.note_keepalive_sent(&link_id, now);
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &buf[..written],
                        }));
                    }
                }
            }
            IngestPacketOutcome::LinkClosedByPeer { link_id } => {
                self.retire_link(&link_id, LinkClosedReason::PeerClosed, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkClose { link_id, reason } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_random(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(dispatch) =
                    self.write_owed_link_close(&link_id, reason, &iv, &mut buf, sink)
                {
                    let target = dispatch.fire_on.unwrap_or(source);
                    if interfaces.is_egress_eligible(target, Egress::Transmit) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &buf[..dispatch.wire_bytes],
                        }));
                    }
                }
            }
            IngestPacketOutcome::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            } => {
                sink(EngineReaction::Journaled(
                    Journaled::LinkInterfaceMismatch {
                        link_id,
                        attached_interface,
                        arrived_on,
                    },
                ));
            }
            IngestPacketOutcome::Ignored(_reason) => {
                #[cfg(feature = "runtime-metrics")]
                self.ignored_packet_counts.record(_reason);
            }
        }
        wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
        IngestPacketReport {
            wake_schedules: wake_schedule_changes,
            protocol_violation,
        }
    }
}
