use crate::crypto::ratchets::RatchetRotation;
use crate::crypto::{
    ed25519_sign, Ed25519Signature, X25519PublicKey, X25519SecretKey, X25519SharedSecret,
};
use crate::engine::execute::settle;
use crate::engine::write_implicit_proof_wire_packet;
use crate::engine::LinkClosedReason;
use crate::engine::{
    write_path_request_wire_packet, AnnounceIngest, AnnounceVerifyOwed, CommandId, DecryptOwed,
    DeferredCrypto, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LinkEstablished, LinkRttOwed, PathFound, PathRequestIdBytes,
    PathResponseWriteOutcome, ProofIngest, RatchetDecryptOwed, SendRequestFailure, Settlement,
    WakeSchedule, WakeSchedules,
};
use crate::identity::{
    decrypt_finish_in_place, IdentitySigner, OpenedBy, OpenedToken, ENCRYPTION_IV_LEN,
};
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{Egress, InboundPacket, InterfaceId, InterfaceKind};
use crate::routing::announce::{Announce, AnnounceArrival};
use crate::routing::delivery::{Delivery, SingleDelivery};
use crate::routing::links::channel::receive::receive as channel_receive;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::handshake::{
    negotiated_link_mtu, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::links::resources::ResourceOffer;
use crate::routing::links::table::LinkActivation;
use crate::routing::links::LinkId;
use crate::routing::proof::{
    DeferredProofSign, LinkProofOwed, ProofObligation, ProofOwed, ProofRequest,
    IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
use crate::routing::RemovedRoute;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, BROADCAST_MTU, HEADER_MAX_LEN};

pub(crate) fn journal_route_removal(removed: RemovedRoute) -> Journaled<'static> {
    Journaled::RouteRemoved {
        destination: removed.destination,
        cause: removed.cause,
    }
}

pub struct IngestIo<'a, F, P, A, K>
where
    F: FnMut(&mut [u8]),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
    K: FnMut(EngineReaction<'_>),
{
    pub interfaces: AttachedInterfaces<'a>,
    pub now: InstantMillis,
    pub fill_entropy: &'a mut F,
    pub should_prove: &'a mut P,
    pub should_accept_resource: &'a mut A,
    pub sink: &'a mut K,
}

struct DeliveryIo<'a, P, K>
where
    P: FnMut(&ProofRequest) -> bool,
    K: FnMut(EngineReaction<'_>),
{
    interfaces: AttachedInterfaces<'a>,
    should_prove: &'a mut P,
    deferred_sign: &'a mut Option<DeferredProofSign>,
    sink: &'a mut K,
}

enum ResolvedProof {
    Withheld,
    Implicit(ProofOwed),
    OverLink(LinkProofOwed),
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5 `Interface.process_held_announces`.
    pub fn fire_due_held_announces<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake = WakeSchedules::UNCHANGED;
        let mut released_any = false;
        while let Some(interface) = self.next_due_held_interface(now) {
            self.interface_announce_limits
                .schedule_next_held_release(interface, now);
            if !self
                .interface_announce_limits
                .rate_is_under_limit(interface, now)
            {
                continue;
            }
            let mut app_data = [0u8; BROADCAST_MTU];
            let Some(released) = self
                .held_announces
                .release_lowest_hop_for(interface, &mut app_data)
            else {
                continue;
            };
            let held = released.held_announce;
            let app_data_len = released.app_data_len;
            let announce = Announce {
                destination: held.destination,
                public_keys: held.announce.public_keys,
                dotted_name_hash: held.announce.dotted_name_hash,
                announce_id: held.announce.announce_id,
                ratchet: held.announce.ratchet,
                signature: held.announce.signature,
                app_data: &app_data[..app_data_len],
            };
            let arrival = AnnounceArrival {
                announce,
                hops: held.hops,
                arrived_at: now,
                receiving_interface: held.receiving_interface,
                next_hop: held.next_hop,
                is_path_response: held.is_path_response,
            };
            let ingest =
                self.ingest_announce(&arrival, &mut *fill_entropy, interfaces, &mut |removed| {
                    sink(EngineReaction::Journaled(journal_route_removal(removed)))
                });
            if let AnnounceIngest::Accepted(accepted) = ingest {
                released_any = true;
                sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                    destination: accepted.destination,
                    hops: accepted.hops,
                    source_interface: held.receiving_interface,
                }));
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    settle(
                        sink,
                        settled.command_id,
                        Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    );
                }
            }
        }
        wake.held_announce_release = self.held_announce_release_wake();
        if released_any {
            wake.scheduled_announces = self.scheduled_announces_wake();
            wake.path_request_timeouts = self.path_request_timeouts_wake();
            wake.expired_routes = self.route_expiry_wake(interfaces);
        }
        wake
    }

    fn next_due_held_interface(&self, now: InstantMillis) -> Option<InterfaceId> {
        self.held_announces.interfaces().find(|&interface| {
            self.interface_announce_limits
                .next_held_release_at(interface)
                .is_some_and(|release| release.0 <= now.0)
        })
    }

    fn process_delivery<'d, P, K>(
        &mut self,
        delivery: Delivery<'d>,
        proof: ProofObligation,
        source: InterfaceId,
        io: &mut DeliveryIo<'_, P, K>,
    ) where
        P: FnMut(&ProofRequest) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        (io.sink)(EngineReaction::Journaled(Journaled::Delivered(delivery)));
        let resolved = match proof {
            ProofObligation::None => ResolvedProof::Withheld,
            ProofObligation::Owed(owed) => ResolvedProof::Implicit(owed),
            ProofObligation::OwedIfApp(owed) => match delivery {
                Delivery::Single(single) => {
                    if (io.should_prove)(&ProofRequest {
                        destination: single.destination,
                        plaintext: single.plaintext,
                    }) {
                        ResolvedProof::Implicit(owed)
                    } else {
                        ResolvedProof::Withheld
                    }
                }
                Delivery::Plain(_) | Delivery::Group(_) | Delivery::Link(_) => {
                    ResolvedProof::Withheld
                }
            },
            ProofObligation::OwedOverLink(owed) => ResolvedProof::OverLink(owed),
            ProofObligation::OwedIfAppOverLink(owed) => match delivery {
                Delivery::Link(link) => {
                    if (io.should_prove)(&ProofRequest {
                        destination: owed.destination,
                        plaintext: link.plaintext,
                    }) {
                        ResolvedProof::OverLink(owed)
                    } else {
                        ResolvedProof::Withheld
                    }
                }
                Delivery::Plain(_) | Delivery::Single(_) | Delivery::Group(_) => {
                    ResolvedProof::Withheld
                }
            },
        };
        match resolved {
            ResolvedProof::Withheld => {}
            ResolvedProof::Implicit(owed) => {
                if io.interfaces.is_egress_eligible(source, Egress::Transmit) {
                    if let Some(signing_secret) = self
                        .held_identities
                        .get(&owed.identity)
                        .map(|held| held.signing_secret_clone())
                    {
                        *io.deferred_sign = Some(DeferredProofSign {
                            target: source,
                            packet_hash: owed.packet_hash,
                            signing_secret,
                        });
                    }
                }
            }
            ResolvedProof::OverLink(owed) => {
                if io.interfaces.is_egress_eligible(source, Egress::Transmit) {
                    let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                    if let Ok(written) = self.write_link_proof(&owed, &mut proof) {
                        (io.sink)(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &proof[..written],
                        }));
                    }
                }
            }
        }
    }

    fn relay_path_request(
        &self,
        destination: DestinationHash,
        id: &PathRequestIdBytes,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        audience: RelayAudience,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let Some(via) = self.transport_id else {
            return;
        };
        let mut buf = [0u8; BROADCAST_MTU];
        let Ok(wire_len) = write_path_request_wire_packet(destination, Some(via), id, &mut buf)
        else {
            return;
        };
        for descriptor in interfaces {
            let in_audience = match audience {
                RelayAudience::Transports => true,
                RelayAudience::LocalClients => {
                    descriptor.id.kind() == Some(InterfaceKind::LocalClient)
                }
            };
            if in_audience && descriptor.id != source && descriptor.capabilities.allows_transport()
            {
                sink(EngineReaction::Directive(Directive::Send {
                    target: descriptor.id,
                    bytes: &buf[..wire_len],
                }));
            }
        }
    }

    pub fn resume_decrypt(
        &mut self,
        owed: DecryptOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        deferred_sign: &mut Option<DeferredProofSign>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let DecryptOwed {
            destination,
            context,
            arrived_at,
            source_interface,
            identity,
            proof_strategy,
            packet_hash,
            mut token,
            ..
        } = owed;
        let Ok(plaintext) = decrypt_finish_in_place(&shared, &identity, &mut token) else {
            return;
        };
        let proof = ProofObligation::for_delivery(
            proof_strategy,
            ProofOwed {
                packet_hash,
                identity,
            },
        );
        let delivery = Delivery::Single(SingleDelivery {
            destination,
            context,
            plaintext,
            opened_by: OpenedBy::IdentityKey,
            arrived_at,
            source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            source_interface,
            &mut DeliveryIo {
                interfaces,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }

    pub fn resume_ratchet_decrypt(
        &mut self,
        owed: RatchetDecryptOwed,
        opened: OpenedToken<'_>,
        interfaces: AttachedInterfaces<'_>,
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        deferred_sign: &mut Option<DeferredProofSign>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let proof = ProofObligation::for_delivery(
            owed.proof_strategy,
            ProofOwed {
                packet_hash: owed.packet_hash,
                identity: owed.identity,
            },
        );
        let delivery = Delivery::Single(SingleDelivery {
            destination: owed.destination,
            context: owed.context,
            plaintext: opened.plaintext,
            opened_by: opened.opened_by,
            arrived_at: owed.arrived_at,
            source_interface: owed.source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            owed.source_interface,
            &mut DeliveryIo {
                interfaces,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }

    fn emit_link_established(
        command_id: CommandId,
        link_id: LinkId,
        rtt: RttMillis,
        target: InterfaceId,
        written: &[u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        sink(EngineReaction::Directive(Directive::Send {
            target,
            bytes: written,
        }));
        settle(
            sink,
            command_id,
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id,
                rtt_ms: rtt.millis(),
            })),
        );
    }

    fn process_owes_link_rtt<F>(
        &mut self,
        owed: LinkRttOwed,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        if !interfaces.is_egress_eligible(source, Egress::Transmit) {
            return WakeSchedule::Unchanged;
        }
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_rtt(
            &owed.link_id,
            &owed.responder_encryption,
            &LinkActivation {
                rtt: owed.rtt,
                mtu: owed.mtu.min(link_mtu_ceiling(interfaces, source)),
                attached_interface: source,
                peer_signing: owed.responder_signing,
            },
            now,
            &iv,
            &mut buf,
        ) {
            Self::emit_link_established(
                owed.command_id,
                owed.link_id,
                owed.rtt,
                source,
                &buf[..written],
                sink,
            );
        }
        self.link_deadlines_wake()
    }

    fn process_owes_link_rtt_with_shared<F>(
        &mut self,
        owed: LinkProofVerifyOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        let source = owed.source_interface;
        if !interfaces.is_egress_eligible(source, Egress::Transmit) {
            return WakeSchedule::Unchanged;
        }
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_rtt_with_shared(
            &owed.link_id,
            &shared,
            &LinkActivation {
                rtt: owed.rtt,
                mtu: owed.mtu.min(link_mtu_ceiling(interfaces, source)),
                attached_interface: source,
                peer_signing: owed.responder_signing,
            },
            now,
            &iv,
            &mut buf,
        ) {
            Self::emit_link_established(
                owed.command_id,
                owed.link_id,
                owed.rtt,
                source,
                &buf[..written],
                sink,
            );
        }
        self.link_deadlines_wake()
    }

    pub fn resume_link_proof<F>(
        &mut self,
        owed: LinkProofVerifyOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake = WakeSchedules::UNCHANGED;
        wake.link_deadlines = self.process_owes_link_rtt_with_shared(
            owed,
            shared,
            interfaces,
            now,
            fill_entropy,
            sink,
        );
        wake
    }

    pub fn resume_link_proof_sign(
        &mut self,
        owed: LinkProofSignOwed,
        responder_encryption: X25519PublicKey,
        shared: X25519SharedSecret,
        signature: Ed25519Signature,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        if !interfaces.is_egress_eligible(owed.source_interface, Egress::Transmit) {
            return wake;
        }
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_proof_with_parts(
            &owed,
            &responder_encryption,
            &shared,
            &signature,
            &mut buf,
        ) {
            sink(EngineReaction::Directive(Directive::Send {
                target: owed.source_interface,
                bytes: &buf[..written],
            }));
        }
        wake.link_deadlines = self.link_deadlines_wake();
        wake
    }

    fn apply_announce_ingest(
        &mut self,
        ingest: AnnounceIngest,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        wake: &mut WakeSchedules,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        match ingest {
            AnnounceIngest::Accepted(accepted) => {
                sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                    destination: accepted.destination,
                    hops: accepted.hops,
                    source_interface: source,
                }));
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    settle(
                        sink,
                        settled.command_id,
                        Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    );
                }
                wake.scheduled_announces = self.scheduled_announces_wake();
                wake.path_request_timeouts = self.path_request_timeouts_wake();
                wake.expired_routes = self
                    .routing_table
                    .existing_route_for(&accepted.destination, interfaces)
                    .map_or(WakeSchedule::Unchanged, |route| {
                        WakeSchedule::AtMost(route.expires_at)
                    });
            }
            AnnounceIngest::Ignored => {
                wake.scheduled_announces = self.scheduled_announces_wake();
            }
            AnnounceIngest::Held => {
                wake.held_announce_release = self.held_announce_release_wake();
            }
            AnnounceIngest::HeldDropped { destination, cause } => {
                sink(EngineReaction::Journaled(Journaled::AnnounceHeldDropped {
                    destination,
                    source_interface: source,
                    cause,
                }));
            }
        }
    }

    pub fn resume_announce(
        &mut self,
        owed: AnnounceVerifyOwed,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        let Ok(announce) = Announce::from_wire_unverified(&owed.header, &owed.payload) else {
            return wake;
        };
        let source = owed.source_interface;
        self.interface_announce_limits
            .record(source, owed.arrived_at);
        let arrival = AnnounceArrival {
            announce,
            hops: owed.received_hops,
            arrived_at: owed.arrived_at,
            receiving_interface: source,
            next_hop: owed.next_hop,
            is_path_response: owed.is_path_response,
        };
        let ingest = self.ingest_announce(&arrival, fill_entropy, interfaces, &mut |removed| {
            sink(EngineReaction::Journaled(journal_route_removal(removed)))
        });
        self.apply_announce_ingest(ingest, source, interfaces, &mut wake, sink);
        wake
    }

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
        let IngestIo {
            interfaces,
            now,
            fill_entropy,
            should_prove,
            should_accept_resource,
            sink,
        } = io;
        let mut deferred_sign: Option<DeferredProofSign> = None;
        let wake = self.ingest_packet_into_deferring(
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
            let mut proof = [0u8; IMPLICIT_PROOF_WIRE_LEN];
            if let Ok(written) =
                write_implicit_proof_wire_packet(&deferred.packet_hash, &signature, &mut proof)
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
        let source = packet.source_interface;
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let outcome = self.ingest_packet_with(
            packet,
            &mut *fill_entropy,
            interfaces,
            &mut |removed| sink(EngineReaction::Journaled(journal_route_removal(removed))),
            deferred.as_deref_mut(),
        );
        match outcome {
            IngestPacketOutcome::Announce(ingest) => {
                self.apply_announce_ingest(
                    ingest,
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
                    &mut DeliveryIo {
                        interfaces,
                        should_prove: &mut *should_prove,
                        deferred_sign: &mut *deferred_sign,
                        sink: &mut *sink,
                    },
                );
            }
            //Not dropped work: these outcomes only surface when `deferred` captured the job for the host's crypto pool.
            //The engine re-enters through the matching resume_* call once the pool answers.
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
                        let wire_len = header_len + body.len;
                        buf[header_len..wire_len].copy_from_slice(body.as_bytes());
                        sink(EngineReaction::Directive(Directive::Send {
                            target: fire_on,
                            bytes: &buf[..wire_len],
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
                        wire_len,
                        ratchet_rotation,
                    } = self.write_path_response_for_upstream(
                        &destination,
                        now,
                        &mut *fill_entropy,
                        &mut response,
                    ) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &response[..wire_len],
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
            IngestPacketOutcome::ForwardRecursivePathRequest { destination, id } => {
                self.relay_path_request(
                    destination,
                    &id,
                    source,
                    interfaces,
                    RelayAudience::Transports,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::RelayPathRequestToLocalClients { destination, id } => {
                self.relay_path_request(
                    destination,
                    &id,
                    source,
                    interfaces,
                    RelayAudience::LocalClients,
                    sink,
                );
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::OwesLinkRtt(owed) => {
                self.process_owes_link_rtt(owed, source, interfaces, now, fill_entropy, sink);
            }
            //Not dropped work: these outcomes only surface when `deferred` captured the job for the host's crypto pool.
            //The engine re-enters through the matching resume_* call once the pool answers.
            IngestPacketOutcome::OwesLinkProofVerify => {}
            IngestPacketOutcome::RequestReceived {
                link_id,
                request_id,
                path_hash,
                requested_at,
                rtt,
                data,
            } => {
                sink(EngineReaction::Journaled(Journaled::RequestReceived {
                    link_id,
                    request_id,
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
                    hash: accepted.hash,
                    uncompressed_data_len: accepted.uncompressed_data_len,
                    sealed_transfer_len: accepted.sealed_transfer_len,
                    part_count: accepted.part_count,
                    segment_index: accepted.segment_index,
                    total_segment_count: accepted.total_segment_count,
                    compression: accepted.compression,
                    has_metadata: accepted.has_metadata,
                };
                if (should_accept_resource)(&offer) {
                    if let IngestPacketOutcome::OwesResourcePull { link_id, hash } =
                        self.admit_accepted_resource(link_id, original_hash, accepted, now)
                    {
                        self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
                        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                        wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                    }
                } else {
                    self.reject_offered_resource(&link_id, &accepted.hash, fill_entropy, sink);
                }
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
            IngestPacketOutcome::ResourceRejectedByPeer { id, link_id } => {
                settle(
                    sink,
                    id,
                    Settlement::SendResource(Err(
                        crate::engine::SendResourceFailure::RejectedByPeer,
                    )),
                );
                self.fail_staged_continuation(&link_id, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceDelivered { id, link_id } => {
                settle(sink, id, Settlement::SendResource(Ok(())));
                self.promote_staged_resource(&link_id, now, fill_entropy, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::PeerIdentified { link_id, identity } => {
                sink(EngineReaction::Journaled(Journaled::PeerIdentified {
                    link_id,
                    identity,
                }));
            }
            IngestPacketOutcome::LinkActivated { link_id, rtt_ms } => {
                sink(EngineReaction::Journaled(Journaled::LinkEstablished(
                    LinkEstablished { link_id, rtt_ms },
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
                            bytes: &buf[..dispatch.wire_len],
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
        //Recomputed for every packet rather than per-arm: any link packet's ingest may note activity and re-arm that link's keepalive or stale deadline, and both sources hold maintained minimums, so the read is O(1).
        wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
        wake_schedule_changes
    }
}

#[derive(Clone, Copy)]
enum RelayAudience {
    Transports,
    LocalClients,
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use crate::crypto::{
        ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey,
        Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::test_support::{transporting_interfaces, TestStorageLayout};
    use crate::engine::CommandId;
    use crate::engine::{Directive, EngineReaction};
    use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
    use crate::routing::links::channel::{write_envelope, ChannelSequence, MessageType};
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::{LinkId, LinkKey};
    use crate::routing::proof::LINK_PROOF_WIRE_LEN;
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, WireContext, BROADCAST_MTU, HEADER_MIN_LEN,
    };
    use std::vec::Vec;

    const LANE: [u8; 8] = [0xEE; 8];

    fn shared() -> crate::crypto::X25519SharedSecret {
        x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x44; 32]),
        )
    }

    fn active_initiator() -> (
        EngineState<TestStorageLayout>,
        LinkId,
        LinkKey,
        Ed25519PublicKey,
    ) {
        let link_id = LinkId::new([0x5C; 16]);
        let link_signing = Ed25519SecretKey::new([0x42; 32]);
        let link_signing_public = ed25519_public_key(&link_signing);
        let mut state = EngineState::<TestStorageLayout>::default();
        state
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing,
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        state
            .links
            .activate_initiated(
                &link_id,
                LinkKey::derive(&link_id, &shared()),
                &LinkActivation {
                    rtt: crate::units::RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: InterfaceId::new(LANE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();
        (
            state,
            link_id,
            LinkKey::derive(&link_id, &shared()),
            link_signing_public,
        )
    }

    fn channel_frame(
        key: &LinkKey,
        link_id: &LinkId,
        message_type: MessageType,
        sequence: ChannelSequence,
        body: &[u8],
    ) -> Vec<u8> {
        let mut envelope = [0u8; BROADCAST_MTU];
        let env_len = write_envelope(message_type, sequence, body, &mut envelope).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let len = write_link_packet(
            link_id,
            key,
            BROADCAST_MTU,
            WireContext::Channel,
            &envelope[..env_len],
            &[0u8; 16],
            &mut frame,
        )
        .unwrap();
        frame[..len].to_vec()
    }

    type FeedOutcome = (Vec<(MessageType, Vec<u8>)>, Option<Vec<u8>>);

    fn feed(state: &mut EngineState<TestStorageLayout>, frame: &[u8], now: u64) -> FeedOutcome {
        let mut raw = frame.to_vec();
        let mut messages = Vec::new();
        let mut ack = None;
        state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: InterfaceId::new(LANE),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(now),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Journaled(Journaled::ChannelMessageReceived {
                        message_type,
                        data,
                        ..
                    }) => messages.push((message_type, data.to_vec())),
                    EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                        ack = Some(bytes.to_vec())
                    }
                    _ => {}
                },
            },
        );
        (messages, ack)
    }

    fn assert_valid_ack(
        ack: &[u8],
        ciphertext: &[u8],
        link_id: &LinkId,
        signer: &Ed25519PublicKey,
    ) {
        assert_eq!(
            ack.len(),
            LINK_PROOF_WIRE_LEN,
            "the ack is one explicit proof"
        );
        let expected = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &link_id.to_address(),
            WireContext::Channel,
            ciphertext,
        );
        assert_eq!(
            &ack[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN],
            expected.as_bytes(),
            "the ack names the packet it proves",
        );
        let signature = Ed25519Signature(
            ack[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
                .try_into()
                .unwrap(),
        );
        ed25519_verify(signer, expected.as_bytes(), &signature)
            .expect("the ack verifies against the initiator's link signing key");
    }

    #[test]
    fn an_in_order_channel_message_is_journaled_and_unconditionally_acked() {
        let (mut state, link_id, key, signer) = active_initiator();
        let frame = channel_frame(
            &key,
            &link_id,
            MessageType(7),
            ChannelSequence(0),
            b"hello channel",
        );
        let ciphertext = frame[HEADER_MIN_LEN..].to_vec();

        let (messages, ack) = feed(&mut state, &frame, 2_000);
        assert_eq!(
            messages,
            std::vec![(MessageType(7), b"hello channel".to_vec())],
            "the message is delivered to the journal in order",
        );
        assert_valid_ack(
            &ack.expect("a channel arrival owes an ack even when should_prove says no"),
            &ciphertext,
            &link_id,
            &signer,
        );
    }

    #[test]
    fn a_gap_then_its_fill_journals_the_whole_run_in_order() {
        let (mut state, link_id, key, _signer) = active_initiator();

        let ahead = channel_frame(&key, &link_id, MessageType(1), ChannelSequence(1), b"one");
        let (messages, ack) = feed(&mut state, &ahead, 2_000);
        assert!(
            messages.is_empty(),
            "the out-of-order arrival waits for the gap"
        );
        assert!(
            ack.is_some(),
            "but it is still acked so the sender stops resending"
        );

        let gap = channel_frame(&key, &link_id, MessageType(0), ChannelSequence(0), b"zero");
        let (messages, ack) = feed(&mut state, &gap, 2_100);
        assert_eq!(
            messages,
            std::vec![
                (MessageType(0), b"zero".to_vec()),
                (MessageType(1), b"one".to_vec()),
            ],
            "filling the gap drains the buffered run in one arrival, in sequence order",
        );
        assert!(ack.is_some(), "the gap-filling arrival is acked too");
    }
}

#[cfg(test)]
mod link_wake_tests {
    use super::*;
    use crate::engine::test_support::{routable_descriptor, TestStorageLayout};
    use crate::engine::{CommandId, EngineState, IngestIo};
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
    use crate::routing::links::table::{InitiatedLink, LinkActivation};
    use crate::routing::links::{LinkId, LinkKey};
    use crate::units::RttMillis;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    fn engine_with_active_link() -> (EngineState<TestStorageLayout>, LinkId, InterfaceId) {
        use crate::crypto::{
            x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey,
            X25519SecretKey,
        };
        let link_id = LinkId::new([0x0F; 16]);
        let lane = InterfaceId::new([0xEE; 8]);
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x55; 32]),
        );
        let mut engine = EngineState::<TestStorageLayout>::default();
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        engine
            .links
            .activate_initiated(
                &link_id,
                LinkKey::derive(&link_id, &shared),
                &LinkActivation {
                    rtt: RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: lane,
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();
        (engine, link_id, lane)
    }

    #[test]
    fn a_keepalive_echo_ingest_reports_the_rearmed_link_deadline() {
        let (mut engine, link_id, lane) = engine_with_active_link();
        let before = engine.link_deadlines_wake();

        let mut frame = [0u8; BROADCAST_MTU];
        let written = write_keepalive(&link_id, KEEPALIVE_ECHO, &mut frame).unwrap();
        let wake = engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: lane,
                bytes: &mut frame[..written],
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[routable_descriptor(lane)]),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |_| {},
            },
        );

        let truth = engine.link_deadlines_wake();
        assert_ne!(
            before, truth,
            "the echo notes link activity and re-arms the link deadline",
        );
        assert_eq!(
            wake.link_deadlines, truth,
            "the ingest delta must carry the re-armed deadline, or the reactor's cached schedule rots and wakes late",
        );
    }
}
