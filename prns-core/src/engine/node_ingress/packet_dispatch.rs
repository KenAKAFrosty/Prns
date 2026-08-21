use super::delivery::DeliveryIo;
use super::journal_route_removal;
use super::relay::{RelayAudience, RelayPathRequest};

use crate::crypto::ratchets::RatchetRotation;
use crate::crypto::{ed25519_sign, X25519SecretKey};
use crate::engine::settlement::settle;
use crate::engine::LinkClosedReason;
use crate::engine::{
    DeferredCrypto, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LinkEstablished, PathResponseWriteOutcome, ProofIngest, SendRequestFailure,
    Settlement, WakeSchedule, WakeSchedules,
};
use crate::identity::{IdentitySigner, ENCRYPTION_IV_LEN};
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{Egress, InboundPacket};
use crate::routing::ingress::{ClassifiedInboundPacket, IngestEffects};
use crate::routing::links::channel::receive::receive as channel_receive;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::handshake::{negotiated_link_mtu, LinkProofSignOwed};
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::links::resources::ResourceOffer;
use crate::routing::proof::{
    DeferredProofSign, ProofRequest, EXPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::storage::StorageLayout;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN};

pub struct IngestIo<'a, FillEntropy, OnProofRequest, OnResourceOffer, Sink>
where
    FillEntropy: FnMut(&mut [u8]),
    OnProofRequest: FnMut(&ProofRequest) -> bool,
    OnResourceOffer: FnMut(&ResourceOffer) -> bool,
    Sink: FnMut(EngineReaction<'_>),
{
    pub interfaces: AttachedInterfaces<'a>,
    pub now: InstantMillis,
    pub fill_entropy: &'a mut FillEntropy,
    pub should_prove: &'a mut OnProofRequest,
    pub should_accept_resource: &'a mut OnResourceOffer,
    pub sink: &'a mut Sink,
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
        K: FnMut(EngineReaction<'_>),
    {
        self.ingest_classified_into(ClassifiedInboundPacket::classify(packet), io)
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
        K: FnMut(EngineReaction<'_>),
    {
        let IngestIo {
            interfaces,
            now,
            fill_entropy,
            should_prove,
            should_accept_resource,
            sink,
        } = io;
        let mut deferred_sign: Option<DeferredProofSign> = None;
        let wake = self.ingest_classified_into_deferring(
            packet,
            IngestIo {
                interfaces,
                now,
                fill_entropy: &mut *fill_entropy,
                should_prove: &mut *should_prove,
                should_accept_resource: &mut *should_accept_resource,
                sink: &mut *sink,
            },
            &mut deferred_sign,
            None,
        );
        if let Some(deferred) = deferred_sign {
            let signature = ed25519_sign(&deferred.signing_secret, deferred.packet_hash.as_bytes());
            let mut proof = [0u8; EXPLICIT_PROOF_WIRE_LEN];
            if let Ok(written) =
                self.write_signed_proof(&deferred.packet_hash, &signature, &mut proof)
            {
                sink(EngineReaction::Directive(Directive::Send {
                    target: deferred.target,
                    bytes: &proof[..written],
                }));
            }
        }
        wake
    }

    pub fn ingest_packet_into_deferring<F, P, A, K>(
        &mut self,
        packet: InboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
        deferred_sign: &mut Option<DeferredProofSign>,
        deferred: Option<&mut DeferredCrypto>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        self.ingest_classified_into_deferring(
            ClassifiedInboundPacket::classify(packet),
            io,
            deferred_sign,
            deferred,
        )
    }

    pub fn ingest_classified_into_deferring<F, P, A, K>(
        &mut self,
        packet: ClassifiedInboundPacket<'_>,
        io: IngestIo<'_, F, P, A, K>,
        deferred_sign: &mut Option<DeferredProofSign>,
        mut deferred: Option<&mut DeferredCrypto>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        A: FnMut(&ResourceOffer) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        let IngestIo {
            interfaces,
            now,
            fill_entropy,
            should_prove,
            should_accept_resource,
            sink,
        } = io;
        let (source, ingress) = packet.into_parts();
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let mut effects = IngestEffects::default();
        let outcome = self.ingest_classified_with_effects(
            ingress,
            &mut *fill_entropy,
            interfaces,
            &mut |removed| sink(EngineReaction::Journaled(journal_route_removal(removed))),
            deferred.as_deref_mut(),
            &mut effects,
        );
        let accepted_observation = effects.accepted_announce.take();
        if let Some(expiry) = effects.destination_identity_expiry {
            wake_schedule_changes.expired_destination_identities = WakeSchedule::AtMost(expiry);
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
                        deferred_sign: &mut *deferred_sign,
                        sink: &mut *sink,
                    },
                );
            }
            IngestPacketOutcome::OwesDecrypt => {}
            IngestPacketOutcome::OwesRatchetDecrypt => {}
            IngestPacketOutcome::OwesAnnounceVerify => {}
            IngestPacketOutcome::Proof(ProofIngest::SendSinglePacketDelivered {
                id,
                delivered,
            }) => {
                settle(sink, id, Settlement::SendSinglePacket(Ok(delivered)));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendToLinkDelivered { id, delivered }) => {
                settle(sink, id, Settlement::SendToLink(Ok(delivered)));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendToChannelDelivered { id, delivered }) => {
                settle(sink, id, Settlement::SendToChannel(Ok(delivered)));
                wake_schedule_changes.channel_timeouts = self.channel_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::Ignored) => {}
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
                    let mut response = [0u8; BROADCAST_MTU];
                    if let PathResponseWriteOutcome::Written {
                        wire_bytes,
                        ratchet_rotation,
                    } = self.write_path_response_for_upstream(
                        &destination,
                        now,
                        &mut *fill_entropy,
                        &mut response,
                    ) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &response[..wire_bytes],
                        }));
                        if ratchet_rotation == RatchetRotation::Minted {
                            sink(EngineReaction::Journaled(Journaled::SelfRatchetRotated {
                                destination,
                            }));
                        }
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
                    RelayAudience::OnlineTransports,
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
                    RelayAudience::Transports,
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
                self.process_owes_link_rtt(owed, source, interfaces, now, fill_entropy, sink);
            }
            IngestPacketOutcome::OwesLinkProofVerify => {}
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
            IngestPacketOutcome::ResponseSettled {
                id,
                delivered,
                link_id,
                request_id,
                data,
            } => {
                sink(EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id: id,
                    link_id,
                    request_id,
                    data,
                }));
                settle(sink, id, Settlement::SendRequest(Ok(delivered)));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResponseTooLarge { id, .. } => {
                settle(
                    sink,
                    id,
                    Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)),
                );
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
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
                    let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                    if let Ok(written) = self.write_channel_ack(&link_id, &packet_hash, &mut proof)
                    {
                        self.links.note_outbound(&link_id, now);
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &proof[..written],
                        }));
                    }
                }
            }
            IngestPacketOutcome::OwesResourceParts(request) => {
                self.serve_resource_request(&request, source, now, fill_entropy, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
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
                        IngestPacketOutcome::OwesResourcePull { link_id, hash } => {
                            self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
                        }
                        IngestPacketOutcome::ResourceAdmissionPending => {}
                        IngestPacketOutcome::ResourceCapacityRejected {
                            link_id,
                            hash,
                            settled_request,
                        } => {
                            self.reject_offered_resource(&link_id, &hash, now, fill_entropy, sink);
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
                        IngestPacketOutcome::Ignored(_) => {}
                        _ => unreachable!("Resource admission returned an unrelated outcome"),
                    }
                    wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                    wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                } else {
                    self.reject_offered_resource(&link_id, &accepted.hash, now, fill_entropy, sink);
                }
            }
            IngestPacketOutcome::ResourceTooLarge {
                link_id,
                hash,
                settled_request,
            } => {
                self.reject_offered_resource(&link_id, &hash, now, fill_entropy, sink);
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
            IngestPacketOutcome::ResourceAdmissionPending => {
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceCapacityRejected {
                link_id,
                hash,
                settled_request,
            } => {
                self.reject_offered_resource(&link_id, &hash, now, fill_entropy, sink);
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
                self.conclude_resource(&link_id, &hash, now, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ResourceDeadlineAdvanced => {
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
                        Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
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
                        fill_entropy,
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
                    self.promote_staged_resource(&link_id, now, fill_entropy, sink);
                }
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::PeerIdentified { link_id, identity } => {
                sink(EngineReaction::Journaled(Journaled::PeerIdentified {
                    link_id,
                    identity,
                }));
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
                    fill_entropy(&mut secret_bytes);
                    if let Some(deferred) = deferred {
                        if let Some(held) = self.held_identities.get(&accepted.identity) {
                            let signing_secret = held.signing_secret_clone();
                            let responder_signing = held.signing_public_key();
                            *deferred = DeferredCrypto::LinkProofSign(LinkProofSignOwed {
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
                            });
                        }
                    } else {
                        let mut buf = [0u8; BROADCAST_MTU];
                        if let Ok(written) = self.write_owed_link_proof(
                            &accepted,
                            X25519SecretKey::new(secret_bytes),
                            link_mtu_ceiling(interfaces, source),
                            &mut buf,
                        ) {
                            sink(EngineReaction::Directive(Directive::Send {
                                target: source,
                                bytes: &buf[..written],
                            }));
                        }
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
                sink(EngineReaction::Journaled(Journaled::LinkClosed {
                    link_id,
                    reason: LinkClosedReason::PeerClosed,
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkClose { link_id, reason } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(dispatch) = self.write_owed_link_close(&link_id, &iv, &mut buf) {
                    let target = dispatch.fire_on.unwrap_or(source);
                    if interfaces.is_egress_eligible(target, Egress::Transmit) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &buf[..dispatch.wire_bytes],
                        }));
                    }
                    sink(EngineReaction::Journaled(Journaled::LinkClosed {
                        link_id,
                        reason,
                    }));
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
            IngestPacketOutcome::TunnelObserved { expires } => {
                wake_schedule_changes.expired_routes = WakeSchedule::AtMost(expires);
            }
            IngestPacketOutcome::Ignored(_reason) => {
                #[cfg(feature = "runtime-metrics")]
                self.ignored_packet_counts.record(_reason);
            }
        }
        wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
        wake_schedule_changes
    }
}
