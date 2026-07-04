use crate::crypto::{
    ed25519_sign, Ed25519Signature, X25519PublicKey, X25519SecretKey, X25519SharedSecret,
};
use crate::engine::write_implicit_proof_wire_packet;
use crate::engine::LinkClosedReason;
use crate::engine::{
    write_path_request_wire_packet, AnnounceIngest, AnnounceVerifyOwed, CommandId, DecryptOwed,
    DeferredCrypto, Directive, EngineReaction, EngineState, IngestPacketOutcome, InstantMillis,
    Journaled, LinkEstablished, LinkRttOwed, PathFound, PathResponseWriteOutcome, ProofIngest,
    RatchetDecryptOwed, Settlement, WakeSchedule, WakeSchedules,
};
use crate::identity::{decrypt_finish_in_place, IdentitySigner, ENCRYPTION_IV_LEN};
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::routing::announce::defaults::JitterSeed;
use crate::routing::announce::{Announce, AnnounceArrival, AnnounceEntropy};
use crate::routing::delivery::{Delivery, SingleDelivery};
use crate::routing::links::channel::receive::receive as channel_receive;
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::handshake::{LinkProofSignOwed, LinkProofVerifyOwed};
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::links::table::LinkActivation;
use crate::routing::links::LinkId;
use crate::routing::proof::{
    DeferredProofSign, ProofObligation, ProofOwed, ProofRequest, IMPLICIT_PROOF_WIRE_LEN,
    LINK_PROOF_WIRE_LEN,
};
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::routing::{RemovedRoute, RouteRemovalCause};
use crate::storage::StorageLayout;
use crate::units::Rtt;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN};

pub(crate) fn journal_removal(removed: RemovedRoute) -> Journaled<'static> {
    match removed.cause {
        RouteRemovalCause::Expired => Journaled::RouteExpired {
            destination: removed.destination,
        },
        RouteRemovalCause::Evicted => Journaled::RouteEvicted {
            destination: removed.destination,
        },
        RouteRemovalCause::InterfaceGone => Journaled::RouteInterfaceGone {
            destination: removed.destination,
        },
    }
}

pub struct IngestIo<'a, F, P, K>
where
    F: FnMut(&mut [u8]),
    P: FnMut(&ProofRequest) -> bool,
    K: FnMut(EngineReaction<'_>),
{
    pub view: &'a [InterfaceConfig],
    pub now: InstantMillis,
    pub fill_entropy: &'a mut F,
    pub should_prove: &'a mut P,
    pub sink: &'a mut K,
}

struct DeliveryIo<'a, P, K>
where
    P: FnMut(&ProofRequest) -> bool,
    K: FnMut(EngineReaction<'_>),
{
    view: &'a [InterfaceConfig],
    should_prove: &'a mut P,
    deferred_sign: &'a mut Option<DeferredProofSign>,
    sink: &'a mut K,
}

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.3.5 `Interface.process_held_announces` (Interfaces/Interface.py:234).
    pub fn fire_due_held_announces<F>(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
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
                .advance_held_release(interface, now);
            if !self.interface_announce_limits.rate_subsided(interface, now) {
                continue;
            }
            let Some(slot) = self.held_announces.lowest_hop_slot(interface) else {
                continue;
            };
            let mut app_data = [0u8; BROADCAST_MTU];
            let Some((held, app_data_len)) = self.held_announces.take(slot, &mut app_data) else {
                continue;
            };
            let announce = Announce {
                destination: held.destination,
                public_keys: held.announce.public_keys,
                dotted_name_hash: held.announce.dotted_name_hash,
                announce_id: held.announce.retained_announce_id,
                maybe_ratchet: held.announce.maybe_ratchet,
                signature: held.announce.signature,
                app_data: &app_data[..app_data_len],
            };
            let mut jitter_bytes = [0u8; core::mem::size_of::<u64>()];
            fill_entropy(&mut jitter_bytes);
            let jitter = JitterSeed(u64::from_le_bytes(jitter_bytes));
            let arrival = AnnounceArrival {
                announce,
                hops: held.hops,
                arrived_at: now,
                receiving_interface: held.receiving_interface,
                next_hop: held.next_hop,
                is_path_response: held.is_path_response,
            };
            let ingest = self.ingest_announce(&arrival, jitter, view, &mut |removed| {
                sink(EngineReaction::Journaled(journal_removal(removed)))
            });
            if let AnnounceIngest::Accepted(accepted) = ingest {
                released_any = true;
                sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                    destination: accepted.destination,
                    hops: accepted.hops,
                    source_interface: held.receiving_interface,
                }));
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id: settled.command_id,
                        settlement: Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    }));
                }
            }
        }
        wake.held_announce_release = self.held_announce_release_wake();
        wake.scheduled_announces = self.scheduled_announces_wake();
        if released_any {
            wake.path_request_timeouts = self.path_request_timeouts_wake();
            wake.expired_routes = self.route_expiry_wake(view);
        }
        wake
    }

    fn next_due_held_interface(&self, now: InstantMillis) -> Option<InterfaceId> {
        self.held_announces.interfaces().find(|&interface| {
            self.interface_announce_limits
                .held_release_for(interface)
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
        let owed = match proof {
            ProofObligation::None
            | ProofObligation::OwedOverLink(_)
            | ProofObligation::OwedIfAppOverLink(_) => None,
            ProofObligation::Owed(owed) => Some(owed),
            ProofObligation::OwedIfApp(owed) => match delivery {
                Delivery::Single(single) => (io.should_prove)(&ProofRequest {
                    destination: single.destination,
                    plaintext: single.plaintext,
                })
                .then_some(owed),
                Delivery::Plain(_) | Delivery::Group(_) | Delivery::Link(_) => None,
            },
        };
        if let Some(owed) = owed {
            if is_egress_eligible(io.view, source, Egress::Transmit) {
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
        let owed_over_link = match proof {
            ProofObligation::None | ProofObligation::Owed(_) | ProofObligation::OwedIfApp(_) => {
                None
            }
            ProofObligation::OwedOverLink(owed) => Some(owed),
            ProofObligation::OwedIfAppOverLink(owed) => match delivery {
                Delivery::Link(link) => (io.should_prove)(&ProofRequest {
                    destination: owed.destination,
                    plaintext: link.plaintext,
                })
                .then_some(owed),
                Delivery::Plain(_) | Delivery::Single(_) | Delivery::Group(_) => None,
            },
        };
        if let Some(owed) = owed_over_link {
            if is_egress_eligible(io.view, source, Egress::Transmit) {
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

    pub fn resume_decrypt(
        &mut self,
        owed: DecryptOwed,
        shared: X25519SharedSecret,
        view: &[InterfaceConfig],
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
            recipient_identity_hash,
            mut token,
            ..
        } = owed;
        let Ok(plaintext) = decrypt_finish_in_place(&shared, &recipient_identity_hash, &mut token)
        else {
            return;
        };
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
        let delivery = Delivery::Single(SingleDelivery {
            destination,
            context,
            plaintext,
            arrived_at,
            source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            source_interface,
            &mut DeliveryIo {
                view,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }

    pub fn resume_ratchet_decrypt(
        &mut self,
        owed: RatchetDecryptOwed,
        plaintext: &[u8],
        view: &[InterfaceConfig],
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        deferred_sign: &mut Option<DeferredProofSign>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let proof = match owed.proof_strategy {
            ProofStrategy::ProveAll => ProofObligation::Owed(ProofOwed {
                packet_hash: owed.packet_hash,
                identity: owed.identity,
            }),
            ProofStrategy::ProveNone => ProofObligation::None,
            ProofStrategy::ProveIf => ProofObligation::OwedIfApp(ProofOwed {
                packet_hash: owed.packet_hash,
                identity: owed.identity,
            }),
        };
        let delivery = Delivery::Single(SingleDelivery {
            destination: owed.destination,
            context: owed.context,
            plaintext,
            arrived_at: owed.arrived_at,
            source_interface: owed.source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            owed.source_interface,
            &mut DeliveryIo {
                view,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }

    fn emit_link_established(
        command_id: CommandId,
        link_id: LinkId,
        rtt: Rtt,
        target: InterfaceId,
        written: &[u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        sink(EngineReaction::Directive(Directive::Send {
            target,
            bytes: written,
        }));
        sink(EngineReaction::Journaled(Journaled::CommandSettled {
            id: command_id,
            settlement: Settlement::EstablishLink(Ok(LinkEstablished {
                link_id,
                rtt_ms: rtt.millis(),
            })),
        }));
    }

    fn process_owes_link_rtt<F>(
        &mut self,
        owed: LinkRttOwed,
        source: InterfaceId,
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        if !is_egress_eligible(view, source, Egress::Transmit) {
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
                mtu: owed.mtu.min(link_mtu_ceiling(view, source)),
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
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        let source = owed.source_interface;
        if !is_egress_eligible(view, source, Egress::Transmit) {
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
                mtu: owed.mtu.min(link_mtu_ceiling(view, source)),
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
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake = WakeSchedules::UNCHANGED;
        wake.link_deadlines =
            self.process_owes_link_rtt_with_shared(owed, shared, view, now, fill_entropy, sink);
        wake
    }

    pub fn resume_link_proof_sign(
        &mut self,
        owed: LinkProofSignOwed,
        responder_encryption: X25519PublicKey,
        shared: X25519SharedSecret,
        signature: Ed25519Signature,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        if !is_egress_eligible(view, owed.source_interface, Egress::Transmit) {
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
        view: &[InterfaceConfig],
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
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id: settled.command_id,
                        settlement: Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    }));
                }
                wake.scheduled_announces = self.scheduled_announces_wake();
                wake.path_request_timeouts = self.path_request_timeouts_wake();
                wake.expired_routes = self
                    .routing_table
                    .existing_route_for(&accepted.destination, view)
                    .map_or(WakeSchedule::Unchanged, |route| {
                        WakeSchedule::AtMost(route.expires)
                    });
            }
            AnnounceIngest::Ignored => {
                wake.scheduled_announces = self.scheduled_announces_wake();
            }
            AnnounceIngest::Held => {
                wake.held_announce_release = self.held_announce_release_wake();
            }
        }
    }

    pub fn resume_announce(
        &mut self,
        owed: AnnounceVerifyOwed,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        let Ok(announce) = Announce::from_wire_unverified(&owed.header, &owed.payload) else {
            return wake;
        };
        let source = owed.source_interface;
        let arrival = AnnounceArrival {
            announce,
            hops: owed.received_hops,
            arrived_at: owed.arrived_at,
            receiving_interface: source,
            next_hop: owed.next_hop,
            is_path_response: owed.is_path_response,
        };
        let ingest = self.ingest_announce(&arrival, owed.jitter, view, &mut |removed| {
            sink(EngineReaction::Journaled(journal_removal(removed)))
        });
        self.apply_announce_ingest(ingest, source, view, &mut wake, sink);
        wake
    }

    pub fn ingest_packet_into<F, P, K>(
        &mut self,
        packet: InboundPacket<'_>,
        jitter: JitterSeed,
        io: IngestIo<'_, F, P, K>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        let IngestIo {
            view,
            now,
            fill_entropy,
            should_prove,
            sink,
        } = io;
        let mut deferred_sign: Option<DeferredProofSign> = None;
        let wake = self.ingest_packet_into_deferring(
            packet,
            jitter,
            IngestIo {
                view,
                now,
                fill_entropy: &mut *fill_entropy,
                should_prove: &mut *should_prove,
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

    pub fn ingest_packet_into_deferring<F, P, K>(
        &mut self,
        packet: InboundPacket<'_>,
        jitter: JitterSeed,
        io: IngestIo<'_, F, P, K>,
        deferred_sign: &mut Option<DeferredProofSign>,
        mut deferred: Option<&mut DeferredCrypto>,
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
        P: FnMut(&ProofRequest) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        let IngestIo {
            view,
            now,
            fill_entropy,
            should_prove,
            sink,
        } = io;
        let source = packet.source_interface;
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        let outcome = self.ingest_packet_with(
            packet,
            jitter,
            view,
            &mut |removed| sink(EngineReaction::Journaled(journal_removal(removed))),
            deferred.as_deref_mut(),
        );
        match outcome {
            IngestPacketOutcome::Announce(ingest) => {
                self.apply_announce_ingest(ingest, source, view, &mut wake_schedule_changes, sink);
            }
            IngestPacketOutcome::Delivery { delivery, proof } => {
                self.process_delivery(
                    delivery,
                    proof,
                    source,
                    &mut DeliveryIo {
                        view,
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
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSinglePacket(Ok(delivered)),
                }));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendToLinkDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendToLink(Ok(delivered)),
                }));
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::SendToChannelDelivered { id, delivered }) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendToChannel(Ok(delivered)),
                }));
                wake_schedule_changes.channel_timeouts = self.channel_timeouts_wake();
            }
            IngestPacketOutcome::Proof(ProofIngest::Ignored) => {}
            IngestPacketOutcome::TransportedLinkRequest {
                header,
                body,
                fire_on,
            } => {
                if is_egress_eligible(view, fire_on, Egress::Transport) {
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
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::Forward(forward) => {
                if is_egress_eligible(view, forward.fire_on, Egress::Transport) {
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
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut entropy_bytes = [0u8; AnnounceEntropy::LEN];
                    fill_entropy(&mut entropy_bytes);
                    let entropy = AnnounceEntropy::new(entropy_bytes);
                    let mut response = [0u8; BROADCAST_MTU];
                    if let PathResponseWriteOutcome::Written { wire_len } =
                        self.write_path_response_announce(&destination, now, entropy, &mut response)
                    {
                        sink(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &response[..wire_len],
                        }));
                    }
                }
            }
            IngestPacketOutcome::ScheduledPathResponse { .. } => {
                wake_schedule_changes.scheduled_announces = self.scheduled_announces_wake();
            }
            IngestPacketOutcome::ForwardPathRequestForDiscovery { destination, id } => {
                if let Some(via) = self.transport_id {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(wire_len) =
                        write_path_request_wire_packet(destination, Some(via), &id, &mut buf)
                    {
                        for config in view {
                            if config.id != source && config.capabilities.allows_transport() {
                                sink(EngineReaction::Directive(Directive::Send {
                                    target: config.id,
                                    bytes: &buf[..wire_len],
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::RelayPathRequestToLocalClients { destination, id } => {
                if let Some(via) = self.transport_id {
                    let mut buf = [0u8; BROADCAST_MTU];
                    if let Ok(wire_len) =
                        write_path_request_wire_packet(destination, Some(via), &id, &mut buf)
                    {
                        for config in view {
                            if config.id != source
                                && config.id.kind() == Some(InterfaceKind::LocalClient)
                                && config.capabilities.allows_transport()
                            {
                                sink(EngineReaction::Directive(Directive::Send {
                                    target: config.id,
                                    bytes: &buf[..wire_len],
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            IngestPacketOutcome::OwesLinkRtt(owed) => {
                wake_schedule_changes.link_deadlines =
                    self.process_owes_link_rtt(owed, source, view, now, fill_entropy, sink);
            }
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
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendRequest(Ok(delivered)),
                }));
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
                if outcome.owes_proof() && is_egress_eligible(view, source, Egress::Transmit) {
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
            }
            IngestPacketOutcome::OwesResourceAssembly { link_id, hash } => {
                self.conclude_resource(&link_id, &hash, now, sink);
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            IngestPacketOutcome::ResourceProgressed => {
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceConcludedFailed { link_id, hash } => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceRejectedByPeer { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Err(
                        crate::engine::SendResourceFailure::RejectedByPeer,
                    )),
                }));
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            IngestPacketOutcome::ResourceDelivered { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Ok(())),
                }));
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
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkProof(accepted) => {
                if is_egress_eligible(view, source, Egress::Transmit) {
                    let mut secret_bytes = [0u8; 32];
                    fill_entropy(&mut secret_bytes);
                    if let Some(deferred) = deferred {
                        if let Some(held) = self.held_identities.get(&accepted.identity) {
                            let signing_secret = held.signing_secret_clone();
                            let responder_signing = held.signing_public_key();
                            deferred.link_proof_sign = Some(LinkProofSignOwed {
                                request: accepted.request,
                                identity: accepted.identity,
                                proof_strategy: accepted.proof_strategy,
                                received_hops: accepted.received_hops,
                                arrived_at: accepted.arrived_at,
                                source_interface: source,
                                mtu: if accepted.request.mtu == 0 {
                                    BROADCAST_MTU
                                } else {
                                    accepted.request.mtu
                                }
                                .min(link_mtu_ceiling(view, source)),
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
                            link_mtu_ceiling(view, source),
                            &mut buf,
                        ) {
                            sink(EngineReaction::Directive(Directive::Send {
                                target: source,
                                bytes: &buf[..written],
                            }));
                        }
                    }
                    wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
                }
            }
            IngestPacketOutcome::OwesKeepaliveEcho { link_id } => {
                if is_egress_eligible(view, source, Egress::Transmit) {
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
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            IngestPacketOutcome::OwesLinkClose { link_id, reason } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(dispatch) = self.write_owed_link_close(&link_id, &iv, &mut buf) {
                    let target = dispatch.fire_on.unwrap_or(source);
                    if is_egress_eligible(view, target, Egress::Transmit) {
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
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
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
            IngestPacketOutcome::Ignored => {}
        }
        wake_schedule_changes
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Egress {
    Transmit,
    Transport,
}

pub(crate) fn is_egress_eligible(
    interfaces: &[InterfaceConfig],
    target: InterfaceId,
    egress_kind: Egress,
) -> bool {
    interfaces
        .iter()
        .find(|config| config.id == target)
        .is_some_and(|config| match egress_kind {
            Egress::Transmit => config.capabilities.allows_transmit(),
            Egress::Transport => config.capabilities.allows_transport(),
        })
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use crate::crypto::{
        ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey,
        Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::test_support::{transporting_view, Cap, TEST_ENTROPY};
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

    fn active_initiator() -> (EngineState<Cap>, LinkId, LinkKey, Ed25519PublicKey) {
        let link_id = LinkId::new([0x5C; 16]);
        let link_signing = Ed25519SecretKey::new([0x42; 32]);
        let link_signing_public = ed25519_public_key(&link_signing);
        let mut state = EngineState::<Cap>::default();
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
                    rtt: crate::units::Rtt(250),
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

    fn feed(state: &mut EngineState<Cap>, frame: &[u8], now: u64) -> FeedOutcome {
        let mut raw = frame.to_vec();
        let mut messages = Vec::new();
        let mut ack = None;
        state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: InterfaceId::new(LANE),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &transporting_view(),
                now: InstantMillis(now),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_| false,
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
            &DestinationHash::new(*link_id.as_bytes()),
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
