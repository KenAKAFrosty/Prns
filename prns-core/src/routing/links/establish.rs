use crate::crypto::{
    x25519_diffie_hellman, x25519_public_key, Ed25519SecretKey, Ed25519Signature, X25519PublicKey,
    X25519SecretKey, X25519SharedSecret,
};
use crate::engine::commands::{CommandId, CommandOutcome, EstablishLink, EstablishLinkRejection};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::routing::delivery::send_single::{
    DEFAULT_FIRST_HOP_TIMEOUT_MS, DEFAULT_PER_HOP_TIMEOUT_MS,
};
use crate::routing::links::handshake::{
    write_link_proof, write_link_proof_from_parts, write_link_request, write_link_rtt,
    AcceptedLinkRequest, LinkProofSignOwed,
};
use crate::routing::links::table::{
    InitiatedLink, LinkActivation, LinkPhase, OverdueLink, RespondingLink, TrackLinkError,
};
use crate::routing::links::{LinkId, LinkKey, LinkMode, MAX_LINK_MTU};
use crate::routing::{NextHop, RouteResponsiveness};
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

pub const ESTABLISH_LINK_ENTROPY_LEN: usize = IDENTITY_SECRET_KEY_LEN;

pub fn link_mtu_ceiling(interfaces: &[InterfaceConfig], interface_id: InterfaceId) -> usize {
    interfaces
        .iter()
        .find(|config| config.id == interface_id)
        .and_then(|config| config.hardware_mtu)
        .unwrap_or(BROADCAST_MTU)
        .min(MAX_LINK_MTU)
}

/// RNS 1.3.5 `Link.KEEPALIVE` (360s); the responder's establishment timeout rides on it.
pub const LINK_KEEPALIVE_MS: u64 = 360_000;

/// A fresh X25519 ‖ Ed25519 pair, the same layout an identity persists. Move-only
/// and never shown; consuming it keys exactly one link request, so one draw can
/// never key two.
pub struct EstablishLinkEntropy([u8; ESTABLISH_LINK_ENTROPY_LEN]);

impl EstablishLinkEntropy {
    pub const LEN: usize = ESTABLISH_LINK_ENTROPY_LEN;

    pub const fn new(bytes: [u8; ESTABLISH_LINK_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    fn into_parts(self) -> (X25519SecretKey, Ed25519SecretKey, InMemoryNodeIdentity) {
        let ephemeral = InMemoryNodeIdentity::from_secret_key_bytes(&self.0);
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&self.0[..32]);
        let mut signing = [0u8; 32];
        signing.copy_from_slice(&self.0[32..]);
        (
            X25519SecretKey::new(scalar),
            Ed25519SecretKey::new(signing),
            ephemeral,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRequestDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteEstablishLinkRejection {
    RouteVanished,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}

impl From<TrackLinkError> for WriteEstablishLinkRejection {
    fn from(error: TrackLinkError) -> Self {
        match error {
            TrackLinkError::TableFull => Self::LinkTableFull,
            TrackLinkError::AlreadyTracked => Self::DuplicateLinkId,
        }
    }
}

#[must_use]
pub enum EstablishLinkWriteOutcome {
    Written(LinkRequestDispatch),
    Failed {
        failure: WriteEstablishLinkRejection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteLinkProofError {
    IdentityNotHeld,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteLinkRttError {
    NotPending,
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_establish_link(&self, id: CommandId, establish: EstablishLink) -> CommandOutcome {
        if self
            .routing_table
            .retained_announce_for(&establish.destination)
            .is_none()
        {
            return CommandOutcome::EstablishLinkRejected {
                id,
                rejection: EstablishLinkRejection::NoRouteToDestination,
            };
        }
        CommandOutcome::OwesLinkRequest { id, establish }
    }

    /// RNS 1.3.5 `Link.__init__`, which always signals the default MTU and mode.
    pub fn write_commanded_link_request(
        &mut self,
        id: CommandId,
        establish: &EstablishLink,
        now: InstantMillis,
        entropy: EstablishLinkEntropy,
        view: &[InterfaceConfig],
        buf: &mut [u8],
    ) -> EstablishLinkWriteOutcome {
        use EstablishLinkWriteOutcome::{Failed, Written};

        let Some(retained) = self
            .routing_table
            .retained_announce_for(&establish.destination)
        else {
            return Failed {
                failure: WriteEstablishLinkRejection::RouteVanished,
            };
        };
        let hops = retained.hops;
        let fire_on = retained.receiving_interface;

        let (initiator_secret, link_signing, ephemeral) = entropy.into_parts();
        let encryption_public = *ephemeral.encryption_public_key().as_x25519();
        let signing_public = *ephemeral.signing_public_key().as_ed25519();
        let link_id = LinkId::derive(&establish.destination, &encryption_public, &signing_public);

        let via = match retained.next_hop {
            NextHop::Via(next) => Some(next),
            NextHop::Direct => None,
        };
        let Ok(wire_len) = write_link_request(
            &establish.destination,
            via,
            &encryption_public,
            &signing_public,
            link_mtu_ceiling(view, fire_on),
            LinkMode::Aes256Cbc,
            buf,
        ) else {
            return Failed {
                failure: WriteEstablishLinkRejection::Serialize,
            };
        };

        let timeout_at = InstantMillis(
            now.0
                .saturating_add(DEFAULT_FIRST_HOP_TIMEOUT_MS)
                .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops.max(1)))),
        );
        match self.links.track_initiated(InitiatedLink {
            link_id,
            destination: establish.destination,
            initiator_secret,
            link_signing,
            requested_at: now,
            timeout_at,
            command_id: id,
        }) {
            Ok(()) => Written(LinkRequestDispatch {
                wire_len,
                fire_on,
                link_id,
            }),
            Err(error) => Failed {
                failure: error.into(),
            },
        }
    }

    /// RNS 1.3.5 `Link.validate_request`, echoing the negotiated MTU and mode.
    pub fn write_owed_link_proof(
        &mut self,
        accepted: &AcceptedLinkRequest,
        ephemeral_secret: X25519SecretKey,
        mtu_ceiling: usize,
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkProofError> {
        let request = &accepted.request;
        let held = self
            .held_identities
            .get(&accepted.identity)
            .ok_or(WriteLinkProofError::IdentityNotHeld)?;
        let responder_encryption = x25519_public_key(&ephemeral_secret);
        let shared = x25519_diffie_hellman(&ephemeral_secret, &request.initiator_encryption);
        let key = LinkKey::derive(&request.link_id, &shared);

        let mtu = if request.mtu == 0 {
            BROADCAST_MTU
        } else {
            request.mtu
        }
        .min(mtu_ceiling);
        let written = write_link_proof(
            &request.link_id,
            &responder_encryption,
            &held,
            mtu,
            request.mode,
            buf,
        )
        .map_err(|_| WriteLinkProofError::Serialize)?;
        self.track_responding_link(accepted, key, mtu)?;
        Ok(written)
    }

    /// The pool twin of [`Self::write_owed_link_proof`]; same bytes either way.
    pub fn write_owed_link_proof_with_parts(
        &mut self,
        owed: &LinkProofSignOwed,
        responder_encryption: &X25519PublicKey,
        shared: &X25519SharedSecret,
        signature: &Ed25519Signature,
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkProofError> {
        let key = LinkKey::derive(&owed.request.link_id, shared);
        let written = write_link_proof_from_parts(
            &owed.request.link_id,
            responder_encryption,
            signature,
            owed.mtu,
            owed.request.mode,
            buf,
        )
        .map_err(|_| WriteLinkProofError::Serialize)?;
        self.track_responding_link(
            &AcceptedLinkRequest {
                request: owed.request,
                identity: owed.identity,
                proof_strategy: owed.proof_strategy,
                received_hops: owed.received_hops,
                arrived_at: owed.arrived_at,
            },
            key,
            owed.mtu,
        )?;
        Ok(written)
    }

    fn track_responding_link(
        &mut self,
        accepted: &AcceptedLinkRequest,
        key: LinkKey,
        mtu: usize,
    ) -> Result<(), WriteLinkProofError> {
        let &AcceptedLinkRequest {
            ref request,
            identity,
            proof_strategy,
            received_hops,
            arrived_at: requested_at,
            ..
        } = accepted;
        let timeout_at = InstantMillis(
            requested_at
                .0
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(received_hops.max(1))),
                )
                .saturating_add(LINK_KEEPALIVE_MS),
        );
        match self.links.track_responding(RespondingLink {
            link_id: request.link_id,
            key,
            requested_at,
            timeout_at,
            mtu,
            initiator_signing: request.initiator_signing,
            destination: request.destination,
            identity,
            proof_strategy,
        }) {
            Ok(()) => Ok(()),
            Err(TrackLinkError::TableFull) => Err(WriteLinkProofError::LinkTableFull),
            Err(TrackLinkError::AlreadyTracked) => Err(WriteLinkProofError::DuplicateLinkId),
        }
    }

    /// RNS 1.3.5 `Link.validate_proof`: the measured RTT rides out encrypted and the
    /// link flips ACTIVE as initiator.
    pub fn write_owed_link_rtt(
        &mut self,
        link_id: &LinkId,
        responder_encryption: &X25519PublicKey,
        activation: &LinkActivation,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        let shared = {
            let Some(LinkPhase::Pending {
                initiator_secret, ..
            }) = self.links.phase_for(link_id)
            else {
                return Err(WriteLinkRttError::NotPending);
            };
            x25519_diffie_hellman(initiator_secret, responder_encryption)
        };
        self.write_owed_link_rtt_with_shared(link_id, &shared, activation, now, iv, buf)
    }

    /// The pool twin of [`Self::write_owed_link_rtt`]; same bytes either way.
    pub fn write_owed_link_rtt_with_shared(
        &mut self,
        link_id: &LinkId,
        shared: &X25519SharedSecret,
        activation: &LinkActivation,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        let Some(LinkPhase::Pending { destination, .. }) = self.links.phase_for(link_id) else {
            return Err(WriteLinkRttError::NotPending);
        };
        let destination = *destination;
        let key = LinkKey::derive(link_id, shared);
        let written = write_link_rtt(link_id, &key, activation.rtt, iv, buf)
            .map_err(|_| WriteLinkRttError::Serialize)?;
        self.links
            .activate_initiated(link_id, key, activation, now)
            .map_err(|_| WriteLinkRttError::NotPending)?;
        self.mark_interface_dirty(activation.attached_interface);
        self.routing_table
            .mark_responsiveness(&destination, RouteResponsiveness::Responsive);
        Ok(written)
    }

    pub fn pop_timed_out_link(&mut self, now: InstantMillis) -> Option<OverdueLink> {
        self.links.pop_overdue(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::IngestIo;
    use crate::engine::{
        AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, Directive, EngineCommand,
        EngineReaction, EngineState, IngestPacketOutcome, IssuedCommand, Journaled, LaneWake,
        LinkEstablished, PacketReceiptDelivered, SendToLinkFailure, Settlement,
    };
    use crate::engine::{EstablishLinkFailure, WakeSchedules};
    use crate::interfaces::{InboundPacket, InterfaceConfig};
    use crate::routing::links::handshake::parse_link_request;
    use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
    use crate::routing::links::table::LinkPhase;
    use crate::routing::links::table::LinkRole;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::routing::RouteResponsiveness;
    use crate::units::Rtt;
    use crate::wire::DestinationHash;

    impl EstablishLinkWriteOutcome {
        #[track_caller]
        fn dispatched(self) -> LinkRequestDispatch {
            match self {
                Self::Written(dispatch) => dispatch,
                Self::Failed { failure } => panic!("expected Written, got Failed({failure:?})"),
            }
        }
    }

    const PEER_DESTINATION_HEX: &str = "c3cfae69b36bb6e3bbfd96a3b5867a59";

    fn peer_destination() -> DestinationHash {
        DestinationHash::new(hx(PEER_DESTINATION_HEX).try_into().unwrap())
    }

    fn arrival() -> InterfaceId {
        InterfaceId::new([0xA1; 8])
    }

    fn arrival_view() -> [InterfaceConfig; 1] {
        [routable_descriptor(arrival())]
    }

    fn vector_establish_entropy() -> EstablishLinkEntropy {
        let mut bytes = [0x77u8; EstablishLinkEntropy::LEN];
        bytes[32..].fill(0x88);
        EstablishLinkEntropy::new(bytes)
    }

    fn establish() -> EstablishLink {
        EstablishLink {
            destination: peer_destination(),
        }
    }

    fn hear_announce(state: &mut EngineState<Cap>, wire: &[u8]) {
        let mut raw = wire.to_vec();
        let outcome = state.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &arrival_view(),
        );
        assert!(
            matches!(
                outcome,
                IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
            ),
            "the announce fixture must take a route before linking",
        );
    }

    fn neighbor_with_a_route() -> EngineState<Cap> {
        let mut announcer = personal_node_announcer();
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = announcer
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();

        let mut state = EngineState::new(second_secret_key());
        hear_announce(&mut state, &announce_buf[..announce_len]);
        state
    }

    #[test]
    fn a_commanded_link_request_frames_tracks_and_arms_the_lane() {
        let mut state = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];

        let dispatch = state
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        assert_eq!(dispatch.fire_on, arrival());
        let parsed = parse_link_request(&buf[..dispatch.wire_len]).unwrap();
        assert_eq!(parsed.destination, peer_destination());
        assert_eq!(parsed.link_id, dispatch.link_id);
        assert_eq!(parsed.mtu, BROADCAST_MTU);
        assert_eq!(parsed.mode, LinkMode::Aes256Cbc);

        let (_, _, ephemeral) = vector_establish_entropy().into_parts();
        assert_eq!(
            parsed.initiator_encryption,
            *ephemeral.encryption_public_key().as_x25519(),
        );
        assert_eq!(
            parsed.initiator_signing,
            *ephemeral.signing_public_key().as_ed25519(),
        );

        assert!(matches!(
            state.links.phase_for(&dispatch.link_id),
            Some(LinkPhase::Pending {
                command_id: CommandId(7),
                ..
            }),
        ));
        assert_eq!(
            state.link_deadlines_wake(),
            LaneWake::At(InstantMillis(13_000)),
            "one direct hop arms first-hop + one per-hop increment",
        );
    }

    #[test]
    fn an_establish_link_needs_a_known_route_and_takes_relayed_ones() {
        let mut state = EngineState::<Cap>::new(second_secret_key());
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(7),
                    command: EngineCommand::EstablishLink(establish()),
                },
                &arrival_view(),
            ),
            CommandOutcome::EstablishLinkRejected {
                id: CommandId(7),
                rejection: EstablishLinkRejection::NoRouteToDestination,
            },
        );

        hear_announce(&mut state, &hx(RNS_1_3_5_RETRANSMITTED_ANNOUNCE));
        let outcome = state.ingest_command(
            IssuedCommand {
                id: CommandId(8),
                command: EngineCommand::EstablishLink(establish()),
            },
            &arrival_view(),
        );
        assert!(
            matches!(outcome, CommandOutcome::OwesLinkRequest { .. }),
            "a route through a relay is linkable in transport, got {outcome:?}",
        );
    }

    #[test]
    fn the_command_lane_fires_the_link_request_at_the_route_interface() {
        let mut state = neighbor_with_a_route();
        let mut sent = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();

        let delta = state.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::EstablishLink(establish()),
            },
            &arrival_view(),
            InstantMillis(1_000),
            &mut |bytes: &mut [u8]| bytes.fill(0x77),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    sent.push((target, bytes.to_vec()));
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );

        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, arrival());
        let parsed = parse_link_request(&sent[0].1).unwrap();
        assert_eq!(parsed.destination, peer_destination());
        assert!(
            settled.is_empty(),
            "an in-flight establishment settles later, not in its own cycle",
        );
        assert_eq!(delta.link_deadlines, LaneWake::At(InstantMillis(13_000)),);
    }

    #[test]
    fn a_silent_handshake_settles_its_command_at_the_deadline() {
        let mut state = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        fn settled_of(reaction: EngineReaction<'_>) -> Option<(CommandId, Settlement)> {
            match reaction {
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    Some((id, settlement))
                }
                _ => None,
            }
        }

        let mut settled = std::vec::Vec::new();
        let early = state.fire_due_link_deadlines(
            InstantMillis(12_999),
            &arrival_view(),
            &mut |bytes: &mut [u8]| bytes.fill(0xE1),
            &mut |reaction| settled.extend(settled_of(reaction)),
        );
        assert!(settled.is_empty(), "the deadline has not passed yet");
        assert_eq!(early.link_deadlines, LaneWake::At(InstantMillis(13_000)),);

        let after = state.fire_due_link_deadlines(
            InstantMillis(13_000),
            &arrival_view(),
            &mut |bytes: &mut [u8]| bytes.fill(0xE1),
            &mut |reaction| settled.extend(settled_of(reaction)),
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
            )],
        );
        assert_eq!(after.link_deadlines, LaneWake::Idle);
        assert!(state.links.is_empty());
        assert_eq!(
            after.scheduled_announces,
            WakeSchedules::UNCHANGED.scheduled_announces,
            "only the link lane moves",
        );
    }

    #[test]
    fn a_timed_out_link_request_marks_its_destination_unresponsive() {
        let mut state = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();
        assert_eq!(
            state
                .routing_table
                .existing_route_for(&peer_destination(), &arrival_view())
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "the route is unconfirmed until a proof returns",
        );

        let _ = state.fire_due_link_deadlines(
            InstantMillis(13_000),
            &arrival_view(),
            &mut |bytes: &mut [u8]| bytes.fill(0xE1),
            &mut |_| {},
        );

        assert_eq!(
            state
                .routing_table
                .existing_route_for(&peer_destination(), &arrival_view())
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
            "our own link request that never established marks its destination unresponsive",
        );
    }

    #[test]
    fn the_initiator_link_activating_marks_its_destination_responsive() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        assert_eq!(
            initiator
                .routing_table
                .existing_route_for(&peer_destination(), &arrival_view())
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
        );

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let _ = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);

        assert_eq!(
            initiator
                .routing_table
                .existing_route_for(&peer_destination(), &arrival_view())
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Responsive,
            "the initiator's link reaching active confirms its destination's route",
        );
    }

    #[test]
    fn a_link_request_for_a_held_destination_owes_its_proof() {
        let mut initiator = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        let mut responder = personal_node_announcer();
        let identity = responder.held_identity_hashes()[0];
        let mut raw = buf[..dispatch.wire_len].to_vec();
        let outcome = responder.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &arrival_view(),
        );
        assert_eq!(
            outcome,
            IngestPacketOutcome::OwesLinkProof(AcceptedLinkRequest {
                request: parse_link_request(&buf[..dispatch.wire_len]).unwrap(),
                identity,
                proof_strategy: crate::routing::upstream_app_destinations::ProofStrategy::ProveNone,
                received_hops: 1,
                arrived_at: InstantMillis(2_000),
            }),
        );

        let mut replay = buf[..dispatch.wire_len].to_vec();
        let replayed = responder.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: arrival(),
                bytes: &mut replay,
            },
            TEST_ENTROPY,
            &arrival_view(),
        );
        assert_eq!(
            replayed,
            IngestPacketOutcome::Ignored,
            "a replayed request deduplicates away",
        );
    }

    #[test]
    fn a_link_request_for_an_unknown_destination_stays_ignored() {
        let mut initiator = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        let mut bystander = EngineState::<Cap>::new(second_secret_key());
        let mut raw = buf[..dispatch.wire_len].to_vec();
        let outcome = bystander.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &arrival_view(),
        );
        assert_eq!(outcome, IngestPacketOutcome::Ignored);
        assert!(bystander.links.is_empty());
    }

    #[test]
    fn the_two_ends_agree_on_the_session_key_through_the_proof() {
        let mut initiator = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        let mut responder = personal_node_announcer();
        let mut sent = std::vec::Vec::new();
        let mut raw = buf[..dispatch.wire_len].to_vec();
        let delta = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_000),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0x99),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                        sent.push((target, bytes.to_vec()));
                    }
                },
            },
        );

        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].0,
            arrival(),
            "the proof answers back on the arrival interface",
        );

        let responder_identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let proof = crate::routing::links::handshake::validate_link_proof(
            &sent[0].1,
            responder_identity.signing_public_key().as_ed25519(),
        )
        .unwrap();
        assert_eq!(proof.link_id, dispatch.link_id);
        assert_eq!(
            proof.mtu, BROADCAST_MTU,
            "the proof echoes the request's mtu"
        );
        assert_eq!(proof.mode, LinkMode::Aes256Cbc);

        let Some(LinkPhase::Pending {
            initiator_secret, ..
        }) = initiator.links.phase_for(&dispatch.link_id)
        else {
            panic!("the initiator must still hold its pending establishment");
        };
        let shared = x25519_diffie_hellman(initiator_secret, &proof.responder_encryption);
        let initiator_key = LinkKey::derive(&dispatch.link_id, &shared);

        let Some(LinkPhase::Handshake {
            key: responder_key, ..
        }) = responder.links.phase_for(&dispatch.link_id)
        else {
            panic!("the responder must be tracking the handshake");
        };

        let iv = [0xA5u8; 16];
        let mut sealed_by_initiator = [0u8; 96];
        let mut sealed_by_responder = [0u8; 96];
        let n = initiator_key
            .seal(&iv, b"two ends, one key", &mut sealed_by_initiator)
            .unwrap();
        let m = responder_key
            .seal(&iv, b"two ends, one key", &mut sealed_by_responder)
            .unwrap();
        assert_eq!(
            &sealed_by_initiator[..n],
            &sealed_by_responder[..m],
            "both ends derive the same session key",
        );

        assert_eq!(
            responder.links.earliest_timeout_at(),
            Some(InstantMillis(2_000 + 6_000 + 360_000)),
            "the responder waits per-hop plus keepalive for the LRRTT",
        );
        assert_eq!(delta.link_deadlines, LaneWake::At(InstantMillis(368_000)),);
    }

    fn reactions_of(
        engine: &mut EngineState<Cap>,
        bytes: &[u8],
        arrived_at: u64,
        iv_fill: u8,
    ) -> (
        std::vec::Vec<std::vec::Vec<u8>>,
        std::vec::Vec<(CommandId, Settlement)>,
        WakeSchedules,
    ) {
        reactions_of_on(engine, bytes, arrived_at, iv_fill, &arrival_view())
    }

    fn reactions_of_on(
        engine: &mut EngineState<Cap>,
        bytes: &[u8],
        arrived_at: u64,
        iv_fill: u8,
        view: &[InterfaceConfig],
    ) -> (
        std::vec::Vec<std::vec::Vec<u8>>,
        std::vec::Vec<(CommandId, Settlement)>,
        WakeSchedules,
    ) {
        let mut sent = std::vec::Vec::new();
        let mut journaled = std::vec::Vec::new();
        let mut raw = bytes.to_vec();
        let delta = engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(arrived_at),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view,
                now: InstantMillis(arrived_at),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Send { target, bytes }) => {
                        assert_eq!(
                            target,
                            arrival(),
                            "every answer rides the arrival interface"
                        );
                        sent.push(bytes.to_vec());
                    }
                    EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                        if let Some(frame) = crate::engine::test_support::filled_frame(fill) {
                            sent.push(frame);
                        }
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        journaled.push((id, settlement));
                    }
                    EngineReaction::Journaled(Journaled::LinkEstablished(established)) => {
                        journaled.push((
                            CommandId(u64::MAX),
                            Settlement::EstablishLink(Ok(established)),
                        ));
                    }
                    _ => {}
                },
            },
        );
        (sent, journaled, delta)
    }

    #[test]
    fn the_full_handshake_activates_both_ends() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = personal_node_announcer();
        let (proofs, journaled, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        assert_eq!(proofs.len(), 1);
        assert!(journaled.is_empty());

        let (rtts, settled, delta) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        assert_eq!(rtts.len(), 1, "the validated proof owes exactly one LRRTT");
        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id,
                    rtt_ms: 250,
                })),
            )],
            "the command settles established with the measured round trip",
        );
        assert!(matches!(
            initiator.links.phase_for(&link_id),
            Some(LinkPhase::Active {
                role: LinkRole::Initiator { .. },
                rtt: Rtt(250),
                ..
            }),
        ));
        assert_eq!(
            delta.link_deadlines,
            LaneWake::At(InstantMillis(1_250 + 51_428)),
            "activation swaps the establishment deadline for the keepalive one",
        );

        let (replay_sent, replay_journaled, _) =
            reactions_of(&mut initiator, &proofs[0], 1_300, 0xA5);
        assert!(replay_sent.is_empty() && replay_journaled.is_empty());

        let (responder_sent, established, delta) =
            reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);
        assert!(responder_sent.is_empty(), "activation answers nothing back");
        assert_eq!(
            established,
            std::vec![(
                CommandId(u64::MAX),
                Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id,
                    rtt_ms: 500,
                })),
            )],
            "the responder journals the link up at max(measured, reported)",
        );
        assert_eq!(
            delta.link_deadlines,
            LaneWake::At(InstantMillis(1_600 + 205_714 + 7_000)),
            "the responder arms its teardown at twice the keepalive plus the rtt*4 + STALE_GRACE grace",
        );

        let Some(LinkPhase::Active {
            key: initiator_key,
            role: LinkRole::Initiator { .. },
            ..
        }) = initiator.links.phase_for(&link_id)
        else {
            panic!("the initiator must be active");
        };
        let Some(LinkPhase::Active {
            key: responder_key,
            role: LinkRole::Responder { .. },
            rtt: Rtt(500),
            ..
        }) = responder.links.phase_for(&link_id)
        else {
            panic!("the responder must be active at the measured rtt");
        };
        let iv = [0xC7u8; 16];
        let mut by_initiator = [0u8; 96];
        let mut by_responder = [0u8; 96];
        let n = initiator_key
            .seal(&iv, b"the link is real", &mut by_initiator)
            .unwrap();
        let m = responder_key
            .seal(&iv, b"the link is real", &mut by_responder)
            .unwrap();
        assert_eq!(
            &by_initiator[..n],
            &by_responder[..m],
            "both active ends hold the same session key",
        );
    }

    #[test]
    fn a_proof_for_an_unknown_link_is_ignored() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);

        let mut bystander = EngineState::<Cap>::new(second_secret_key());
        let mut raw = proofs[0].clone();
        let outcome = bystander.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_250),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &arrival_view(),
        );
        assert_eq!(outcome, IngestPacketOutcome::Ignored);
    }

    #[test]
    fn a_tampered_lrrtt_keeps_the_handshake_pending() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);

        let mut tampered = rtts[0].clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let (sent, journaled, _) = reactions_of(&mut responder, &tampered, 1_600, 0xB5);
        assert!(sent.is_empty() && journaled.is_empty());
        assert!(
            matches!(
                responder.links.phase_for(&dispatch.link_id),
                Some(LinkPhase::Handshake { .. }),
            ),
            "an unauthenticated LRRTT never moves the link; the genuine one still can",
        );
    }

    #[test]
    fn an_authenticated_but_malformed_lrrtt_tears_the_link_down() {
        use crate::engine::reaction::LinkClosedReason;
        use crate::wire::WirePacketHeader;

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();

        let mut responder = personal_node_announcer();
        let (_, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);

        let mut frame = std::vec![0x0Cu8, 0x00];
        frame.extend_from_slice(dispatch.link_id.as_bytes());
        frame.push(0xFE);
        let Some(LinkPhase::Handshake { key, .. }) = responder.links.phase_for(&dispatch.link_id)
        else {
            panic!("the responder must be awaiting its LRRTT");
        };
        let mut not_msgpack = [0xC1u8; 9];
        not_msgpack[1..].fill(0x55);
        let mut sealed = [0u8; 64];
        let n = key.seal(&[0xB5; 16], &not_msgpack, &mut sealed).unwrap();
        frame.extend_from_slice(&sealed[..n]);

        let mut closes = std::vec::Vec::new();
        let mut journaled = std::vec::Vec::new();
        let mut raw = frame.clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_600),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(1_600),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xB6),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Send { target, bytes }) => {
                        assert_eq!(target, arrival());
                        closes.push(bytes.to_vec());
                    }
                    EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                        journaled.push((link_id, reason));
                    }
                    _ => {}
                },
            },
        );

        assert_eq!(
            journaled,
            std::vec![(dispatch.link_id, LinkClosedReason::Protocol)],
            "the reference tears down here; with teardown vocabulary, so do we",
        );
        assert!(responder.links.phase_for(&dispatch.link_id).is_none());
        assert_eq!(
            closes.len(),
            1,
            "the peer is told with the sealed LINKCLOSE"
        );
        let (header, _) = WirePacketHeader::parse(&closes[0]).unwrap();
        assert_eq!(header.context, crate::wire::WireContext::LinkClose);
        assert_eq!(header.destination.as_bytes(), dispatch.link_id.as_bytes());
    }

    #[test]
    fn link_data_crosses_the_active_link_and_journals_the_delivery() {
        use crate::engine::{SendToLink, SendToLinkPayload};
        use crate::routing::delivery::Delivery;

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let mut sent = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id,
                    payload: SendToLinkPayload::from_slice(b"hello over the link").unwrap(),
                }),
            },
            &arrival_view(),
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xD1),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    assert_eq!(target, arrival(), "the data fires on the link's interface");
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
                    assert_eq!(target, arrival(), "the data fires on the link's interface");
                    if let Some(bytes) = filled_frame(fill) {
                        sent.push(bytes);
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );
        assert_eq!(sent.len(), 1);
        assert!(
            settled.is_empty(),
            "a link send settles through its receipt now, never at emission",
        );

        let mut delivered = std::vec::Vec::new();
        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_100),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD2),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::Delivered(Delivery::Link(link))) =
                        reaction
                    {
                        delivered.push((link.link_id, link.plaintext.to_vec()));
                    }
                },
            },
        );
        assert_eq!(
            delivered,
            std::vec![(link_id, b"hello over the link".to_vec())],
            "the responder opens the frame under the session key and journals it",
        );

        let mut replay = sent[0].clone();
        let mut replayed = std::vec::Vec::new();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: arrival(),
                bytes: &mut replay,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD3),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::Delivered(_)) = reaction {
                        replayed.push(());
                    }
                },
            },
        );
        assert!(replayed.is_empty(), "a replayed frame deduplicates away");
    }

    fn proving_node_announcer(strategy: ProofStrategy) -> EngineState<Cap> {
        use crate::engine::RatchetPolicy;

        let mut state: EngineState<Cap> = EngineState::new(fixed_secret_key());
        let node = state.held_identity_hashes()[0];
        state
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"hello-personal",
                strategy,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        state
    }

    fn commanded_link_data(
        engine: &mut EngineState<Cap>,
        link_id: LinkId,
        payload: &[u8],
        now: u64,
        iv_fill: u8,
    ) -> std::vec::Vec<u8> {
        use crate::engine::{SendToLink, SendToLinkPayload};

        let mut sent = std::vec::Vec::new();
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id,
                    payload: SendToLinkPayload::from_slice(payload).unwrap(),
                }),
            },
            &arrival_view(),
            InstantMillis(now),
            &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(bytes) = filled_frame(fill) {
                        sent.push(bytes);
                    }
                }
                _ => {}
            },
        );
        assert_eq!(sent.len(), 1, "the link data frame fires");
        sent.remove(0)
    }

    #[test]
    fn a_prove_all_responder_proves_link_data_the_reference_way() {
        use crate::crypto::{ed25519_verify, Ed25519Signature};
        use crate::routing::dedup::PacketHash;
        use crate::routing::proof::EXPLICIT_PROOF_PAYLOAD_LEN;
        use crate::wire::{DestinationType, PacketType, WireContext, WirePacketHeader};

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let data = commanded_link_data(&mut initiator, link_id, b"prove this", 2_000, 0xD1);
        let (answers, _, _) = reactions_of(&mut responder, &data, 2_100, 0xD2);
        assert_eq!(answers.len(), 1, "the ProveAll responder answers a proof");

        let (header, payload) = WirePacketHeader::parse(&answers[0]).unwrap();
        assert_eq!(header.packet_type, PacketType::Proof);
        assert_eq!(header.destination_type, DestinationType::Link);
        assert_eq!(header.destination.as_bytes(), link_id.as_bytes());
        assert_eq!(header.context, WireContext::None);
        assert_eq!(header.hops, 0);
        assert_eq!(payload.len(), EXPLICIT_PROOF_PAYLOAD_LEN);

        let (data_header, data_payload) = WirePacketHeader::parse(&data).unwrap();
        let expected_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data_header.destination,
            data_header.context,
            data_payload,
        );
        assert_eq!(
            &payload[..32],
            expected_hash.as_bytes(),
            "the proof names the ciphertext frame's packet hash",
        );

        let Some(LinkPhase::Active { peer_signing, .. }) = initiator.links.phase_for(&link_id)
        else {
            panic!("the initiator holds the active link");
        };
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&payload[32..]);
        ed25519_verify(peer_signing, &payload[..32], &Ed25519Signature(signature)).expect(
            "the proof validates against the announced identity the initiator already holds",
        );

        let proof_frame = answers[0].clone();
        let (echoes, journaled, _) = reactions_of(&mut initiator, &proof_frame, 2_200, 0xF1);
        assert!(echoes.is_empty(), "a proof is an ending, not a beginning");
        assert_eq!(
            journaled,
            std::vec![(
                CommandId(9),
                Settlement::SendToLink(Ok(PacketReceiptDelivered {
                    rtt: Rtt::from_millis(200)
                })),
            )],
            "the receipt settles the send with the proof's round trip",
        );
        assert!(
            initiator.receipts.is_empty(),
            "settlement removes the receipt — a replayed proof finds nothing",
        );
    }

    #[test]
    fn a_forged_link_proof_settles_nothing() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let data = commanded_link_data(&mut initiator, link_id, b"prove this", 2_000, 0xD1);
        let (answers, _, _) = reactions_of(&mut responder, &data, 2_100, 0xD2);
        let mut forged = answers[0].clone();
        let last = forged.len() - 1;
        forged[last] ^= 0x01;

        let (_, journaled, _) = reactions_of(&mut initiator, &forged, 2_200, 0xF1);
        assert!(journaled.is_empty(), "a forged signature settles nothing");
        assert_eq!(
            initiator.receipts.len(),
            1,
            "the receipt stays outstanding for its timeout",
        );
    }

    #[test]
    fn an_unproven_link_send_times_out_at_the_traffic_deadline() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let _ = commanded_link_data(&mut initiator, link_id, b"never proven", 2_000, 0xD1);
        assert_eq!(
            initiator.receipt_timeouts_wake(),
            LaneWake::At(InstantMillis(3_500)),
            "the deadline is max(rtt × 6, 5 ms) past the send: 2_000 + 250 × 6",
        );

        let mut settled = std::vec::Vec::new();
        let mut collect = |reaction: EngineReaction<'_>| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        };
        let _ = initiator.settle_timed_out_receipts(InstantMillis(3_499), &mut collect);
        let _ = initiator.settle_timed_out_receipts(InstantMillis(3_500), &mut collect);
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
            )],
            "past the deadline the send settles Timeout, exactly once",
        );
    }

    #[test]
    fn the_initiator_never_proves_link_data() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let data = commanded_link_data(&mut responder, link_id, b"no proof owed", 2_000, 0xC1);
        let (answers, _, _) = reactions_of(&mut initiator, &data, 2_100, 0xC2);
        assert!(
            answers.is_empty(),
            "the initiator's side of a link is a remote destination, and it never proves",
        );
    }

    #[test]
    fn the_app_decider_gates_the_prove_if_link_proof() {
        use crate::engine::ProofRequest;

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = proving_node_announcer(ProofStrategy::ProveIf);
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let mut answer_deferred = |data: &[u8], arrived_at: u64, agree: bool| {
            let mut requests = std::vec::Vec::new();
            let mut answers = std::vec::Vec::new();
            let mut raw = data.to_vec();
            let _ = responder.ingest_packet_into(
                InboundPacket {
                    arrived_at: InstantMillis(arrived_at),
                    source_interface: arrival(),
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                IngestIo {
                    view: &arrival_view(),
                    now: InstantMillis(arrived_at),
                    fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD2),
                    should_prove: &mut |request: &ProofRequest| {
                        requests.push((request.destination, request.plaintext.to_vec()));
                        agree
                    },
                    sink: &mut |reaction| {
                        if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                            answers.push(bytes.to_vec());
                        }
                    },
                },
            );
            (requests, answers)
        };

        let data = commanded_link_data(&mut initiator, link_id, b"ask the app", 2_000, 0xD1);
        let (requests, answers) = answer_deferred(&data, 2_100, true);
        assert_eq!(
            requests,
            std::vec![(personal_node_destination(), b"ask the app".to_vec())],
            "the decider sees the registered destination and the decrypted content",
        );
        assert_eq!(answers.len(), 1, "the decider agreed, so the proof answers");

        let again = commanded_link_data(&mut initiator, link_id, b"ask once more", 3_000, 0xE1);
        let (requests, answers) = answer_deferred(&again, 3_100, false);
        assert_eq!(requests.len(), 1);
        assert!(
            answers.is_empty(),
            "the decider declined, so no proof goes out"
        );
    }

    #[test]
    fn a_link_establishes_and_carries_data_through_a_transport_node() {
        use crate::routing::delivery::Delivery;

        let iface_to_a = arrival();
        let iface_to_b = InterfaceId::new([0xB7; 8]);
        let relay_view = [
            routable_descriptor(iface_to_a),
            routable_descriptor(iface_to_b),
        ];

        let mut relay = EngineState::<Cap>::new(fixed_secret_key());
        relay.set_transport_id(TEST_TRANSPORT_ID);
        let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
        let mut announce_buf = [0u8; BROADCAST_MTU];
        let announce_len = responder
            .write_commanded_announce(
                &AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
                InstantMillis(100),
                TEST_ANNOUNCE_ENTROPY,
                TEST_RATCHET_ENTROPY,
                &mut announce_buf,
            )
            .written_len();
        let ingest_via = |engine: &mut EngineState<Cap>,
                          bytes: &[u8],
                          iface: InterfaceId,
                          now: u64,
                          iv_fill: u8,
                          view: &[InterfaceConfig]| {
            let mut sent = std::vec::Vec::new();
            let mut journaled = std::vec::Vec::new();
            let mut settled = std::vec::Vec::new();
            let mut closed = std::vec::Vec::new();
            let mut raw = bytes.to_vec();
            let _ = engine.ingest_packet_into(
                InboundPacket {
                    arrived_at: InstantMillis(now),
                    source_interface: iface,
                    bytes: &mut raw,
                },
                TEST_ENTROPY,
                IngestIo {
                    view,
                    now: InstantMillis(now),
                    fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
                    should_prove: &mut |_: &crate::engine::ProofRequest| false,
                    sink: &mut |reaction| match reaction {
                        EngineReaction::Directive(Directive::Send { target, bytes }) => {
                            sent.push((target, bytes.to_vec()));
                        }
                        EngineReaction::Directive(Directive::EmitFrame {
                            target, fill, ..
                        }) => {
                            if let Some(frame) = crate::engine::test_support::filled_frame(fill) {
                                sent.push((target, frame));
                            }
                        }
                        EngineReaction::Journaled(Journaled::Delivered(Delivery::Link(link))) => {
                            journaled.push(link.plaintext.to_vec());
                        }
                        EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                            settled.push((id, settlement));
                        }
                        EngineReaction::Journaled(Journaled::LinkClosed { reason, .. }) => {
                            closed.push(reason);
                        }
                        _ => {}
                    },
                },
            );
            (sent, journaled, settled, closed)
        };
        let _ = ingest_via(
            &mut relay,
            &announce_buf[..announce_len],
            iface_to_b,
            500,
            0x10,
            &relay_view,
        );
        assert_eq!(
            relay
                .routing_table
                .existing_route_for(&personal_node_destination(), &relay_view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "the relay's freshly heard route to B is unconfirmed",
        );

        let mut initiator = EngineState::<Cap>::new(second_secret_key());
        hear_announce(&mut initiator, &hx(RNS_1_3_5_RETRANSMITTED_ANNOUNCE));

        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let (switched, _, _, _) = ingest_via(
            &mut relay,
            &request[..dispatch.wire_len],
            iface_to_a,
            1_100,
            0x20,
            &relay_view,
        );
        assert_eq!(switched.len(), 1, "the relay forwards the link request");
        assert_eq!(switched[0].0, iface_to_b);
        assert!(
            relay.transported_links.entry_for(&link_id).is_some(),
            "the relay carries the pending link",
        );

        let (proofs, _, _) = reactions_of(&mut responder, &switched[0].1, 1_200, 0x99);
        let (returned, _, _, _) =
            ingest_via(&mut relay, &proofs[0], iface_to_b, 1_300, 0x30, &relay_view);
        assert_eq!(returned.len(), 1, "the relay returns the validated proof");
        assert_eq!(returned[0].0, iface_to_a);
        assert!(
            relay
                .transported_links
                .entry_for(&link_id)
                .unwrap()
                .validated,
            "the proof validated the transported row",
        );
        assert_eq!(
            relay
                .routing_table
                .existing_route_for(&personal_node_destination(), &relay_view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Responsive,
            "validating the transported proof confirms the relay's route to B",
        );

        let (rtts, _, _) = reactions_of(&mut initiator, &returned[0].1, 1_400, 0xA5);
        let (switched_rtt, _, _, _) =
            ingest_via(&mut relay, &rtts[0], iface_to_a, 1_500, 0x40, &relay_view);
        assert_eq!(switched_rtt.len(), 1, "the relay switches the sealed LRRTT");
        assert_eq!(switched_rtt[0].0, iface_to_b);
        let (_, _, _) = reactions_of(&mut responder, &switched_rtt[0].1, 1_600, 0xB5);
        assert!(matches!(
            responder.links.phase_for(&link_id),
            Some(LinkPhase::Active { .. }),
        ));

        let data = commanded_link_data(&mut initiator, link_id, b"across the mesh", 2_000, 0xD1);
        let (switched_data, _, _, _) =
            ingest_via(&mut relay, &data, iface_to_a, 2_100, 0x50, &relay_view);
        assert_eq!(switched_data.len(), 1);
        let (proof_answers, delivered, _, _) = ingest_via(
            &mut responder,
            &switched_data[0].1,
            arrival(),
            2_200,
            0x60,
            &arrival_view(),
        );
        assert_eq!(
            delivered,
            std::vec![b"across the mesh".to_vec()],
            "the relay switched ciphertext it could never read",
        );

        assert_eq!(proof_answers.len(), 1, "the ProveAll responder proves");
        let (switched_proof, _, _, _) = ingest_via(
            &mut relay,
            &proof_answers[0].1,
            iface_to_b,
            2_300,
            0x61,
            &relay_view,
        );
        assert_eq!(switched_proof.len(), 1);
        assert_eq!(switched_proof[0].0, iface_to_a);
        let (_, _, settled, _) = ingest_via(
            &mut initiator,
            &switched_proof[0].1,
            arrival(),
            2_400,
            0x62,
            &arrival_view(),
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::SendToLink(Ok(crate::engine::PacketReceiptDelivered {
                    rtt: Rtt::from_millis(400),
                })),
            )],
            "the proof crossed two hops and settled the send",
        );

        let mut keepalive = [0u8; BROADCAST_MTU];
        let n = crate::routing::links::maintenance::write_keepalive(
            &link_id,
            KEEPALIVE_REQUEST,
            &mut keepalive,
        )
        .unwrap();
        let (switched_keepalive, _, _, _) = ingest_via(
            &mut relay,
            &keepalive[..n],
            iface_to_a,
            2_500,
            0x63,
            &relay_view,
        );
        assert_eq!(switched_keepalive.len(), 1);
        assert_eq!(switched_keepalive[0].0, iface_to_b);
        let (echoes, _, _, _) = ingest_via(
            &mut responder,
            &switched_keepalive[0].1,
            arrival(),
            2_600,
            0x64,
            &arrival_view(),
        );
        assert_eq!(echoes.len(), 1, "the responder echoes the keepalive");
        let (switched_echo, _, _, _) = ingest_via(
            &mut relay,
            &echoes[0].1,
            iface_to_b,
            2_700,
            0x65,
            &relay_view,
        );

        let (keepalive_again, _, _, _) = ingest_via(
            &mut relay,
            &keepalive[..n],
            iface_to_a,
            2_800,
            0x66,
            &relay_view,
        );
        assert_eq!(
            keepalive_again.len(),
            1,
            "an identical keepalive switches every time it arrives",
        );
        let mut part = [0u8; BROADCAST_MTU];
        let part_len = crate::routing::links::data::write_link_raw_packet(
            &link_id,
            crate::wire::PacketType::Data,
            crate::wire::WireContext::Resource,
            BROADCAST_MTU,
            b"the same raw part, twice",
            &mut part,
        )
        .unwrap();
        for resend in 0..2 {
            let (switched_part, _, _, _) = ingest_via(
                &mut relay,
                &part[..part_len],
                iface_to_a,
                2_850 + resend,
                0x67,
                &relay_view,
            );
            assert_eq!(
                switched_part.len(),
                1,
                "a byte-identical resource part switches on send and on resend",
            );
        }
        let (data_replay, _, _, _) =
            ingest_via(&mut relay, &data, iface_to_a, 2_900, 0x68, &relay_view);
        assert_eq!(
            data_replay.len(),
            0,
            "a replayed sealed data frame stays behind the duplicate filter",
        );
        assert_eq!(switched_echo.len(), 1);
        assert_eq!(
            switched_echo[0].0, iface_to_a,
            "the echo returns to A's side"
        );

        let mut close_frames = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(10),
                command: EngineCommand::CloseLink(crate::engine::CloseLink { link_id }),
            },
            &arrival_view(),
            InstantMillis(2_800),
            &mut |bytes: &mut [u8]| bytes.fill(0x66),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    close_frames.push(bytes.to_vec());
                }
            },
        );
        let (switched_close, _, _, _) = ingest_via(
            &mut relay,
            &close_frames[0],
            iface_to_a,
            2_900,
            0x67,
            &relay_view,
        );
        assert_eq!(switched_close.len(), 1);
        let (_, _, _, closed) = ingest_via(
            &mut responder,
            &switched_close[0].1,
            arrival(),
            3_000,
            0x68,
            &arrival_view(),
        );
        assert_eq!(
            closed,
            std::vec![crate::engine::reaction::LinkClosedReason::PeerClosed],
            "the goodbye crossed the mesh",
        );
        assert!(
            responder.links.phase_for(&link_id).is_none(),
            "the responder's session is gone",
        );
    }

    #[test]
    fn a_request_passes_the_allow_gate_only_after_the_peer_identifies() {
        use crate::engine::{
            Identify, PacketReceiptDelivered, Respond, RespondData, SendRequest, SendRequestData,
            SendRequestFailure,
        };
        use crate::routing::links::request::RequestId;
        use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let asker = initiator.held_identity_hashes()[0];
        responder
            .register_request_handler(
                &personal_node_destination(),
                "/status",
                RequestPolicy::AllowList,
            )
            .unwrap();
        responder
            .allow_requester(&personal_node_destination(), "/status", asker)
            .unwrap();

        let command = |engine: &mut EngineState<Cap>,
                       id: u64,
                       command: EngineCommand,
                       now: u64,
                       iv_fill: u8| {
            let mut sent = std::vec::Vec::new();
            let mut settled = std::vec::Vec::new();
            let _ = engine.ingest_command_into(
                IssuedCommand {
                    id: CommandId(id),
                    command,
                },
                &arrival_view(),
                InstantMillis(now),
                &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
                &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                        sent.push(bytes.to_vec());
                    }
                    EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                        if let Some(bytes) = filled_frame(fill) {
                            sent.push(bytes);
                        }
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        settled.push((id, settlement));
                    }
                    _ => {}
                },
            );
            (sent, settled)
        };
        let ask = SendRequest {
            link_id,
            path_hash: RequestPathHash::of("/status"),
            data: SendRequestData::from_slice(&[0xC4, 0x03, b'a', b's', b'k']).unwrap(),
        };

        let (sent, settled) = command(
            &mut initiator,
            20,
            EngineCommand::SendRequest(ask.clone()),
            2_000,
            0xD1,
        );
        assert_eq!(sent.len(), 1);
        assert!(settled.is_empty(), "the request awaits its response");
        let mut heard = std::vec::Vec::new();
        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_100),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::RequestReceived { .. })
                    | EngineReaction::Directive(Directive::Send { .. }) = reaction
                    {
                        heard.push(());
                    }
                },
            },
        );
        assert!(heard.is_empty(), "a stranger's request is silently refused");

        let (identify_frames, _) = command(
            &mut initiator,
            21,
            EngineCommand::Identify(Identify {
                link_id,
                identity: asker,
            }),
            2_200,
            0xE1,
        );
        let mut raw = identify_frames[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_300),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_300),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |_| {},
            },
        );

        let Some(LinkPhase::Active {
            remote_identity, ..
        }) = responder.links.phase_for(&link_id)
        else {
            panic!("active");
        };
        assert_eq!(
            *remote_identity,
            Some(asker),
            "identify stored the identity"
        );
        let (sent, _) = command(
            &mut initiator,
            22,
            EngineCommand::SendRequest(ask),
            2_400,
            0xF1,
        );
        let mut received: std::vec::Vec<(RequestId, std::vec::Vec<u8>)> = std::vec::Vec::new();
        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_500),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_500),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::RequestReceived {
                        link_id: heard_link,
                        request_id,
                        path_hash,
                        data,
                        ..
                    }) = reaction
                    {
                        assert_eq!(heard_link, link_id);
                        assert_eq!(path_hash, RequestPathHash::of("/status"));
                        received.push((request_id, data.to_vec()));
                    }
                },
            },
        );
        assert_eq!(received.len(), 1, "the identified peer's request lands");
        assert_eq!(received[0].1, &[0xC4, 0x03, b'a', b's', b'k']);
        let request_id = received[0].0;

        let (responses, settled) = command(
            &mut responder,
            23,
            EngineCommand::Respond(Respond {
                link_id,
                request_id,
                data: RespondData::from_slice(&[0xC4, 0x02, b'o', b'k']).unwrap(),
            }),
            2_600,
            0xA9,
        );
        assert_eq!(
            settled,
            std::vec![(CommandId(23), Settlement::Respond(Ok(())))],
            "a response is fire-and-forget",
        );
        let mut answered = std::vec::Vec::new();
        let mut concluded = std::vec::Vec::new();
        let mut raw = responses[0].clone();
        let _ = initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_700),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_700),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Journaled(Journaled::ResponseReceived {
                        request_id: answered_id,
                        data,
                        ..
                    }) => {
                        assert_eq!(answered_id, request_id);
                        answered.push(data.to_vec());
                    }
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        concluded.push((id, settlement));
                    }
                    _ => {}
                },
            },
        );
        assert_eq!(answered, std::vec![std::vec![0xC4, 0x02, b'o', b'k']]);
        assert_eq!(
            concluded,
            std::vec![(
                CommandId(22),
                Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: Rtt::from_millis(300)
                })),
            )],
            "the response settles the request with the measured round trip",
        );

        let mut expired = std::vec::Vec::new();
        let mut collect = |reaction: EngineReaction<'_>| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                expired.push((id, settlement));
            }
        };
        let _ = initiator.settle_timed_out_receipts(InstantMillis(14_749), &mut collect);
        let _ = initiator.settle_timed_out_receipts(InstantMillis(14_750), &mut collect);
        assert_eq!(
            expired,
            std::vec![(
                CommandId(20),
                Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
            )],
        );
    }

    #[test]
    fn the_initiator_identifies_itself_and_the_responder_journals_it() {
        use crate::engine::{Identify, IdentifyRejection};
        use crate::wire::{WireContext, WirePacketHeader};

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let link_id = dispatch.link_id;

        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let revealed = initiator.held_identity_hashes()[0];
        let mut sent = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(11),
                command: EngineCommand::Identify(Identify {
                    link_id,
                    identity: revealed,
                }),
            },
            &arrival_view(),
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xE7),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(bytes) = filled_frame(fill) {
                        sent.push(bytes);
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );
        assert_eq!(
            settled,
            std::vec![(CommandId(11), Settlement::Identify(Ok(())))],
            "an identify is fire-and-forget: it settles at emission",
        );
        let (header, _) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.context, WireContext::LinkIdentify);

        let mut identified = std::vec::Vec::new();
        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_100),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::PeerIdentified {
                        link_id,
                        identity,
                    }) = reaction
                    {
                        identified.push((link_id, identity));
                    }
                },
            },
        );
        assert_eq!(
            identified,
            std::vec![(link_id, revealed)],
            "the responder validates the signature and surfaces the identity",
        );

        let mut echoed = std::vec::Vec::new();
        let mut replay = sent[0].clone();
        let _ = initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: arrival(),
                bytes: &mut replay,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::PeerIdentified { .. }) = reaction {
                        echoed.push(());
                    }
                },
            },
        );
        assert!(echoed.is_empty(), "an initiator never accepts an identify");

        let mut tampered = sent[0].clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let mut forged = std::vec::Vec::new();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_300),
                source_interface: arrival(),
                bytes: &mut tampered,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_300),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::PeerIdentified { .. }) = reaction {
                        forged.push(());
                    }
                },
            },
        );
        assert!(forged.is_empty(), "a tampered identify surfaces nothing");

        let outcome = responder.ingest_command(
            IssuedCommand {
                id: CommandId(12),
                command: EngineCommand::Identify(Identify {
                    link_id,
                    identity: responder.held_identity_hashes()[0],
                }),
            },
            &arrival_view(),
        );
        assert!(
            matches!(
                outcome,
                crate::engine::CommandOutcome::IdentifyRejected {
                    rejection: IdentifyRejection::NotInitiator,
                    ..
                },
            ),
            "got {outcome:?}",
        );
    }

    #[test]
    fn a_send_to_link_demands_an_active_link() {
        use crate::engine::{SendToLink, SendToLinkPayload, SendToLinkRejection};

        let mut initiator = neighbor_with_a_route();
        let send = |link_id| IssuedCommand {
            id: CommandId(9),
            command: EngineCommand::SendToLink(SendToLink {
                link_id,
                payload: SendToLinkPayload::from_slice(b"too early").unwrap(),
            }),
        };

        assert_eq!(
            initiator.ingest_command(send(LinkId::new([0x77; 16])), &arrival_view()),
            CommandOutcome::SendToLinkRejected {
                id: CommandId(9),
                rejection: SendToLinkRejection::NoSuchLink,
            },
        );

        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        assert_eq!(
            initiator.ingest_command(send(dispatch.link_id), &arrival_view()),
            CommandOutcome::SendToLinkRejected {
                id: CommandId(9),
                rejection: SendToLinkRejection::LinkNotActive,
            },
        );
    }

    fn established_pair() -> (EngineState<Cap>, EngineState<Cap>, LinkId) {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let mut responder = personal_node_announcer();
        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);
        (initiator, responder, dispatch.link_id)
    }

    #[test]
    fn a_registered_default_resource_strategy_greets_the_link_at_activation() {
        use crate::routing::links::resources::ResourceStrategy;

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut request,
            )
            .dispatched();
        let mut responder = personal_node_announcer();
        let opened_gate = ResourceStrategy::Accept {
            max_uncompressed_len: 1 << 20,
            accept_compressed: false,
        };
        assert!(responder.set_default_resource_strategy(&personal_node_destination(), opened_gate));

        let (proofs, _, _) =
            reactions_of(&mut responder, &request[..dispatch.wire_len], 1_100, 0x99);
        let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
        let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

        let Some(LinkPhase::Active {
            resource_strategy, ..
        }) = responder.links.phase_for(&dispatch.link_id)
        else {
            panic!("the responder's link must be active");
        };
        assert_eq!(
            *resource_strategy, opened_gate,
            "the destination's default is stamped at activation — no command, no race",
        );
    }

    fn fire_deadlines(
        state: &mut EngineState<Cap>,
        now: u64,
    ) -> (
        std::vec::Vec<std::vec::Vec<u8>>,
        std::vec::Vec<(LinkId, crate::engine::reaction::LinkClosedReason)>,
    ) {
        let mut sent = std::vec::Vec::new();
        let mut closed = std::vec::Vec::new();
        let _ = state.fire_due_link_deadlines(
            InstantMillis(now),
            &arrival_view(),
            &mut |bytes: &mut [u8]| bytes.fill(0xE7),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    assert_eq!(target, arrival());
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                    closed.push((link_id, reason));
                }
                _ => {}
            },
        );
        (sent, closed)
    }

    #[test]
    fn a_quiet_link_keepalives_then_goes_stale_and_closes() {
        use crate::engine::reaction::LinkClosedReason;
        use crate::wire::{WireContext, WirePacketHeader};

        let (mut initiator, mut responder, link_id) = established_pair();

        let (sent, closed) = fire_deadlines(&mut initiator, 52_677);
        assert!(sent.is_empty() && closed.is_empty(), "nothing fires early");

        let (sent, closed) = fire_deadlines(&mut initiator, 52_678);
        assert!(closed.is_empty());
        assert_eq!(sent.len(), 1, "the rtt-paced keepalive fires");
        let (header, payload) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.context, WireContext::KeepAlive);
        assert_eq!(payload, &[KEEPALIVE_REQUEST]);

        let mut echoes = std::vec::Vec::new();
        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(52_690),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(52_690),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xE8),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                        echoes.push(bytes.to_vec());
                    }
                },
            },
        );
        assert_eq!(echoes.len(), 1, "the responder answers the keepalive");
        let (header, payload) = WirePacketHeader::parse(&echoes[0]).unwrap();
        assert_eq!(header.context, WireContext::KeepAlive);
        assert_eq!(payload, &[KEEPALIVE_ECHO]);

        let mut raw = echoes[0].clone();
        let _ = initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(52_700),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(52_700),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xE9),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |_| {},
            },
        );

        let (sent, closed) = fire_deadlines(&mut initiator, 104_128);
        assert!(closed.is_empty(), "the echo postponed staleness");
        assert_eq!(sent.len(), 1, "a second keepalive rides the new cadence");

        let (sent, closed) = fire_deadlines(&mut initiator, 52_700 + 102_856);
        assert!(
            closed.is_empty(),
            "reaching the stale boundary sends a final keepalive, not a teardown",
        );
        assert_eq!(sent.len(), 1, "the stale link pings its peer one last time");
        let (header, payload) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.context, WireContext::KeepAlive);
        assert_eq!(payload, &[KEEPALIVE_REQUEST]);

        let (sent, closed) = fire_deadlines(&mut initiator, 52_700 + 102_856 + 6_000);
        assert_eq!(
            sent.len(),
            1,
            "only after the rtt*4 + STALE_GRACE grace does the stale link tell its peer",
        );
        let (header, _) = WirePacketHeader::parse(&sent[0]).unwrap();
        assert_eq!(header.context, WireContext::LinkClose);
        assert_eq!(closed, std::vec![(link_id, LinkClosedReason::Timeout)]);
        assert!(initiator.links.is_empty(), "the closed link is forgotten");
    }

    #[test]
    fn a_close_link_command_settles_and_closes_the_peer() {
        use crate::engine::reaction::LinkClosedReason;
        use crate::engine::{CloseLink, CloseLinkRejection};

        let (mut initiator, mut responder, link_id) = established_pair();

        assert_eq!(
            initiator.ingest_command(
                IssuedCommand {
                    id: CommandId(11),
                    command: EngineCommand::CloseLink(CloseLink {
                        link_id: LinkId::new([0x77; 16]),
                    }),
                },
                &arrival_view(),
            ),
            CommandOutcome::CloseLinkRejected {
                id: CommandId(11),
                rejection: CloseLinkRejection::NoSuchLink,
            },
        );

        let mut sent = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(12),
                command: EngineCommand::CloseLink(CloseLink { link_id }),
            },
            &arrival_view(),
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xEA),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(bytes) = filled_frame(fill) {
                        sent.push(bytes);
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );
        assert_eq!(
            settled,
            std::vec![(CommandId(12), Settlement::CloseLink(Ok(())))],
        );
        assert_eq!(sent.len(), 1);
        assert!(initiator.links.is_empty());

        let mut tampered = sent[0].clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let mut journaled = std::vec::Vec::new();
        let mut raw = tampered;
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_100),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_100),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xEB),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) =
                        reaction
                    {
                        journaled.push((link_id, reason));
                    }
                },
            },
        );
        assert!(
            journaled.is_empty(),
            "an unauthenticated close never drops the link",
        );
        assert!(responder.links.phase_for(&link_id).is_some());

        let mut raw = sent[0].clone();
        let _ = responder.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: arrival(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            IngestIo {
                view: &arrival_view(),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xEC),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) =
                        reaction
                    {
                        journaled.push((link_id, reason));
                    }
                },
            },
        );
        assert_eq!(
            journaled,
            std::vec![(link_id, LinkClosedReason::PeerClosed)],
        );
        assert!(responder.links.is_empty());
    }

    #[test]
    fn a_narrow_interface_negotiates_the_link_mtu_down_end_to_end() {
        use crate::engine::{SendToLink, SendToLinkFailure, SendToLinkPayload};
        use crate::routing::links::data::LinkDataError;

        fn narrow_view() -> [InterfaceConfig; 1] {
            let mut config = routable_descriptor(arrival());
            config.hardware_mtu = Some(300);
            [config]
        }

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &narrow_view(),
                &mut request,
            )
            .dispatched();
        let parsed = parse_link_request(&request[..dispatch.wire_len]).unwrap();
        assert_eq!(
            parsed.mtu, 300,
            "the initiator signals its interface's ceiling"
        );

        let mut responder = personal_node_announcer();
        let (proofs, _, _) = reactions_of_on(
            &mut responder,
            &request[..dispatch.wire_len],
            1_100,
            0x99,
            &narrow_view(),
        );
        let (rtts, _, _) = reactions_of_on(&mut initiator, &proofs[0], 1_250, 0xA5, &narrow_view());
        let (_, _, _) = reactions_of_on(&mut responder, &rtts[0], 1_600, 0xB5, &narrow_view());

        for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
            let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id)
            else {
                panic!("the {name} must be active");
            };
            assert_eq!(*mtu, 300, "the {name} settled on the narrow mtu");
        }

        let mut settled = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id: dispatch.link_id,
                    payload: SendToLinkPayload::from_slice(&[0x42; 250]).unwrap(),
                }),
            },
            &narrow_view(),
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xD1),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    let _ = filled_frame(fill);
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::SendToLink(Err(SendToLinkFailure::WriteFailed(
                    LinkDataError::PayloadTooLong,
                ))),
            )],
            "250 bytes overflow the narrow link's 223-byte MDU",
        );

        let mut sent = std::vec::Vec::new();
        let _ = initiator.ingest_command_into(
            IssuedCommand {
                id: CommandId(10),
                command: EngineCommand::SendToLink(SendToLink {
                    link_id: dispatch.link_id,
                    payload: SendToLinkPayload::from_slice(&[0x42; 200]).unwrap(),
                }),
            },
            &narrow_view(),
            InstantMillis(2_100),
            &mut |bytes: &mut [u8]| bytes.fill(0xD2),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(bytes) = filled_frame(fill) {
                        sent.push(bytes);
                    }
                }
                _ => {}
            },
        );
        assert_eq!(sent.len(), 1, "200 bytes fit the narrow link");
        assert!(
            sent[0].len() <= 300,
            "the frame respects the negotiated mtu"
        );
    }

    #[test]
    fn a_fat_interface_negotiates_up_to_the_engine_ceiling_and_no_further() {
        use crate::routing::links::MAX_LINK_MTU;

        fn fat_view() -> [InterfaceConfig; 1] {
            let mut config = routable_descriptor(arrival());
            config.hardware_mtu = Some(1_064);
            [config]
        }
        let negotiable = MAX_LINK_MTU.min(1_064);

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &fat_view(),
                &mut request,
            )
            .dispatched();
        let parsed = parse_link_request(&request[..dispatch.wire_len]).unwrap();
        assert_eq!(
            parsed.mtu, negotiable,
            "a fat interface signals up to the engine ceiling, never past it",
        );

        let mut responder = personal_node_announcer();
        let (proofs, _, _) = reactions_of_on(
            &mut responder,
            &request[..dispatch.wire_len],
            1_100,
            0x99,
            &fat_view(),
        );
        let (rtts, _, _) = reactions_of_on(&mut initiator, &proofs[0], 1_250, 0xA5, &fat_view());
        let (_, _, _) = reactions_of_on(&mut responder, &rtts[0], 1_600, 0xB5, &fat_view());

        for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
            let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id)
            else {
                panic!("the {name} must be active");
            };
            assert_eq!(
                *mtu, negotiable,
                "the {name} settled on the negotiable ceiling; raising MAX_LINK_MTU \
                 (with the seam frame) is what unlocks the rest of this interface",
            );
        }
    }

    #[test]
    fn the_real_usb_descriptors_negotiate_their_declared_ceilings() {
        use crate::interfaces::usb_auto::core::{
            device_descriptor, host_descriptor, DEVICE_USB_HW_MTU, HOST_USB_HW_MTU,
        };
        use crate::routing::links::MAX_LINK_MTU;

        let host = host_descriptor(arrival());
        let device = device_descriptor(arrival());
        assert_eq!(host.hardware_mtu, Some(HOST_USB_HW_MTU));
        assert_eq!(device.hardware_mtu, Some(DEVICE_USB_HW_MTU));

        let expected = MAX_LINK_MTU
            .min(host.hardware_mtu.unwrap())
            .min(MAX_LINK_MTU.min(device.hardware_mtu.unwrap()));

        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &[host],
                &mut request,
            )
            .dispatched();
        let parsed = parse_link_request(&request[..dispatch.wire_len]).unwrap();
        assert_eq!(
            parsed.mtu,
            MAX_LINK_MTU.min(host.hardware_mtu.unwrap()),
            "the host side signals its declared tier up to the engine ceiling",
        );

        let mut responder = personal_node_announcer();
        let (proofs, _, _) = reactions_of_on(
            &mut responder,
            &request[..dispatch.wire_len],
            1_100,
            0x99,
            &[device],
        );
        let (rtts, _, _) = reactions_of_on(&mut initiator, &proofs[0], 1_250, 0xA5, &[host]);
        let (_, _, _) = reactions_of_on(&mut responder, &rtts[0], 1_600, 0xB5, &[device]);

        for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
            let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id)
            else {
                panic!("the {name} must be active over the real descriptors");
            };
            assert_eq!(
                *mtu, expected,
                "the {name} settled at the min of both real ceilings and the knob",
            );
        }
    }

    #[test]
    fn a_repeated_entropy_draw_is_refused_as_a_duplicate_link() {
        let mut state = neighbor_with_a_route();
        let mut buf = [0u8; BROADCAST_MTU];
        let _ = state
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
                &arrival_view(),
                &mut buf,
            )
            .dispatched();

        let outcome = state.write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(2_000),
            vector_establish_entropy(),
            &arrival_view(),
            &mut buf,
        );
        assert!(matches!(
            outcome,
            EstablishLinkWriteOutcome::Failed {
                failure: WriteEstablishLinkRejection::DuplicateLinkId,
            },
        ));
        assert_eq!(state.links.len(), 1, "the original establishment stands");
    }
}
