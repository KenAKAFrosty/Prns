use crate::crypto::{x25519_diffie_hellman, x25519_public_key, X25519PublicKey, X25519SecretKey};
use crate::engine::commands::{CommandId, CommandOutcome, EstablishLink, EstablishLinkError};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentityHash, IdentitySigner, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::delivery::send_single::{
    DEFAULT_FIRST_HOP_TIMEOUT_MS, DEFAULT_PER_HOP_TIMEOUT_MS,
};
use crate::routing::links::handshake::{
    write_link_proof, write_link_request, write_link_rtt, LinkRequest,
};
use crate::routing::links::table::{
    InitiatedLink, LinkPhase, OverdueLink, RespondingLink, TrackLinkError,
};
use crate::routing::links::{LinkId, LinkKey, LinkMode};
use crate::routing::storage::EngineStorage;
use crate::routing::NextHop;
use crate::wire::BROADCAST_MTU;

pub const ESTABLISH_LINK_ENTROPY_LEN: usize = IDENTITY_SECRET_KEY_LEN;

/// RNS 1.3.1 `Link.KEEPALIVE` (360s): the responder's establishment timeout
/// rides on it (Link.py:207), and the keepalive cadence itself arrives with
/// the link maintenance arc.
pub const LINK_KEEPALIVE_MS: u64 = 360_000;

/// One establishment's worth of ephemeral key material: a fresh X25519
/// (encryption) secret followed by a fresh Ed25519 (signing) secret, the same
/// layout an identity persists. Move-only and never shown. Consuming it keys
/// exactly one link request, so one draw can never key two.
pub struct EstablishLinkEntropy([u8; ESTABLISH_LINK_ENTROPY_LEN]);

impl EstablishLinkEntropy {
    pub const LEN: usize = ESTABLISH_LINK_ENTROPY_LEN;

    pub const fn new(bytes: [u8; ESTABLISH_LINK_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    fn into_parts(self) -> (X25519SecretKey, InMemoryNodeIdentity) {
        let ephemeral = InMemoryNodeIdentity::from_secret_key_bytes(&self.0);
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&self.0[..32]);
        (X25519SecretKey::new(scalar), ephemeral)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRequestDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteEstablishLinkError {
    RouteVanished,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}

impl From<TrackLinkError> for WriteEstablishLinkError {
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
    Failed { failure: WriteEstablishLinkError },
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

impl<S: EngineStorage> EngineState<S> {
    pub fn ingest_establish_link(&self, id: CommandId, establish: EstablishLink) -> CommandOutcome {
        let Some(retained) = self
            .routing_table
            .retained_announce_for(&establish.destination)
        else {
            return CommandOutcome::EstablishLinkRejected {
                id,
                error: EstablishLinkError::NoRouteToDestination,
            };
        };
        if retained.hops > 1 || retained.next_hop != NextHop::Direct {
            return CommandOutcome::EstablishLinkRejected {
                id,
                error: EstablishLinkError::NotDirectlyReachable,
            };
        }
        CommandOutcome::OwesLinkRequest { id, establish }
    }

    /// Mint the initiator's ephemeral keypair from `entropy`, frame the
    /// LINKREQUEST directly into `buf` (RNS 1.3.1 `Link.__init__`, which
    /// always signals the default MTU and mode), and track the pending
    /// establishment that `id` settles through.
    pub fn write_commanded_link_request(
        &mut self,
        id: CommandId,
        establish: &EstablishLink,
        now: InstantMillis,
        entropy: EstablishLinkEntropy,
        buf: &mut [u8],
    ) -> EstablishLinkWriteOutcome {
        use EstablishLinkWriteOutcome::{Failed, Written};

        let Some(retained) = self
            .routing_table
            .retained_announce_for(&establish.destination)
        else {
            return Failed {
                failure: WriteEstablishLinkError::RouteVanished,
            };
        };
        let hops = retained.hops;
        let fire_on = retained.receiving_interface;

        let (initiator_secret, ephemeral) = entropy.into_parts();
        let encryption_public = *ephemeral.encryption_public_key().as_x25519();
        let signing_public = *ephemeral.signing_public_key().as_ed25519();
        let link_id = LinkId::derive(&establish.destination, &encryption_public, &signing_public);

        let Ok(wire_len) = write_link_request(
            &establish.destination,
            &encryption_public,
            &signing_public,
            BROADCAST_MTU,
            LinkMode::Aes256Cbc,
            buf,
        ) else {
            return Failed {
                failure: WriteEstablishLinkError::Serialize,
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

    /// Answer an inbound LINKREQUEST the way RNS 1.3.1 `Link.validate_request`
    /// does: derive the session key from a fresh ephemeral against the
    /// initiator's public, frame the identity-signed LRPROOF (echoing the
    /// negotiated MTU and mode) directly into `buf`, and track the responding
    /// link awaiting its LRRTT.
    pub fn write_owed_link_proof(
        &mut self,
        request: &LinkRequest,
        identity: &IdentityHash,
        received_hops: u8,
        arrived_at: InstantMillis,
        ephemeral_secret: X25519SecretKey,
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkProofError> {
        let held = self
            .held_identities
            .get(identity)
            .ok_or(WriteLinkProofError::IdentityNotHeld)?;
        let responder_encryption = x25519_public_key(&ephemeral_secret);
        let shared = x25519_diffie_hellman(&ephemeral_secret, &request.initiator_encryption);
        let key = LinkKey::derive(&request.link_id, &shared);

        let mtu = if request.mtu == 0 {
            BROADCAST_MTU
        } else {
            request.mtu
        };
        let written = write_link_proof(
            &request.link_id,
            &responder_encryption,
            &held,
            mtu,
            request.mode,
            buf,
        )
        .map_err(|_| WriteLinkProofError::Serialize)?;

        let timeout_at = InstantMillis(
            arrived_at
                .0
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(received_hops.max(1))),
                )
                .saturating_add(LINK_KEEPALIVE_MS),
        );
        match self.links.track_responding(RespondingLink {
            link_id: request.link_id,
            key,
            requested_at: arrived_at,
            timeout_at,
        }) {
            Ok(()) => Ok(written),
            Err(TrackLinkError::TableFull) => Err(WriteLinkProofError::LinkTableFull),
            Err(TrackLinkError::AlreadyTracked) => Err(WriteLinkProofError::DuplicateLinkId),
        }
    }

    /// Pay the validated LRPROOF the way RNS 1.3.1 `Link.validate_proof`
    /// finishes: the pending secret's ECDH against the responder's ephemeral
    /// derives the session key, the measured RTT rides out encrypted under it,
    /// and the link flips ACTIVE as initiator.
    pub fn write_owed_link_rtt(
        &mut self,
        link_id: &LinkId,
        responder_encryption: &X25519PublicKey,
        rtt_ms: u64,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        let Some(LinkPhase::Pending {
            initiator_secret, ..
        }) = self.links.phase_for(link_id)
        else {
            return Err(WriteLinkRttError::NotPending);
        };
        let shared = x25519_diffie_hellman(initiator_secret, responder_encryption);
        let key = LinkKey::derive(link_id, &shared);
        let written = write_link_rtt(link_id, &key, rtt_ms, iv, buf)
            .map_err(|_| WriteLinkRttError::Serialize)?;
        self.links
            .activate_initiated(link_id, key, rtt_ms)
            .map_err(|_| WriteLinkRttError::NotPending)?;
        Ok(written)
    }

    /// Drain one establishment whose handshake never completed. Call
    /// repeatedly until `None` to fully drain. An initiated pop is that
    /// command's timeout settlement.
    pub fn pop_timed_out_link(&mut self, now: InstantMillis) -> Option<OverdueLink> {
        self.links.pop_overdue(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, Directive, EngineCommand,
        EngineReaction, EngineState, IngestPacketOutcome, IssuedCommand, Journaled, LaneWake,
        LinkEstablished, Settlement,
    };
    use crate::engine::{EstablishLinkFailure, WakeSchedules};
    use crate::interfaces::{InboundPacket, InterfaceConfig};
    use crate::routing::links::handshake::parse_link_request;
    use crate::routing::links::table::LinkPhase;
    use crate::routing::links::table::LinkRole;
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
        InterfaceId::new([0xA1; 16])
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
                &mut buf,
            )
            .dispatched();

        assert_eq!(dispatch.fire_on, arrival());
        let parsed = parse_link_request(&buf[..dispatch.wire_len]).unwrap();
        assert_eq!(parsed.destination, peer_destination());
        assert_eq!(parsed.link_id, dispatch.link_id);
        assert_eq!(parsed.mtu, BROADCAST_MTU);
        assert_eq!(parsed.mode, LinkMode::Aes256Cbc);

        let (_, ephemeral) = vector_establish_entropy().into_parts();
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
            state.link_establishment_timeout_wake(),
            LaneWake::At(InstantMillis(13_000)),
            "one direct hop arms first-hop + one per-hop increment",
        );
    }

    #[test]
    fn an_establish_link_needs_a_known_direct_route() {
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
                error: EstablishLinkError::NoRouteToDestination,
            },
        );

        hear_announce(&mut state, &hx(RNS_1_3_1_RETRANSMITTED_ANNOUNCE));
        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: CommandId(8),
                    command: EngineCommand::EstablishLink(establish()),
                },
                &arrival_view(),
            ),
            CommandOutcome::EstablishLinkRejected {
                id: CommandId(8),
                error: EstablishLinkError::NotDirectlyReachable,
            },
            "a route through a relay is not yet linkable",
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
        assert_eq!(
            delta.link_establishment_timeout,
            LaneWake::At(InstantMillis(13_000)),
        );
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
        let early = state
            .settle_timed_out_link_establishments(InstantMillis(12_999), &mut |reaction| {
                settled.extend(settled_of(reaction))
            });
        assert!(settled.is_empty(), "the deadline has not passed yet");
        assert_eq!(
            early.link_establishment_timeout,
            LaneWake::At(InstantMillis(13_000)),
        );

        let after = state
            .settle_timed_out_link_establishments(InstantMillis(13_000), &mut |reaction| {
                settled.extend(settled_of(reaction))
            });
        assert_eq!(
            settled,
            std::vec![(
                CommandId(7),
                Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
            )],
        );
        assert_eq!(after.link_establishment_timeout, LaneWake::Idle);
        assert!(state.links.is_empty());
        assert_eq!(
            after.scheduled_announces,
            WakeSchedules::UNCHANGED.scheduled_announces,
            "only the link lane moves",
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
            IngestPacketOutcome::OwesLinkProof {
                request: parse_link_request(&buf[..dispatch.wire_len]).unwrap(),
                identity,
                received_hops: 1,
                arrived_at: InstantMillis(2_000),
            },
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
            &arrival_view(),
            InstantMillis(2_000),
            &mut |bytes: &mut [u8]| bytes.fill(0x99),
            &mut |_: &crate::engine::ProofRequest| false,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                    sent.push((target, bytes.to_vec()));
                }
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
        assert_eq!(
            delta.link_establishment_timeout,
            LaneWake::At(InstantMillis(368_000)),
        );
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
            &arrival_view(),
            InstantMillis(arrived_at),
            &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
            &mut |_: &crate::engine::ProofRequest| false,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    assert_eq!(
                        target,
                        arrival(),
                        "every answer rides the arrival interface"
                    );
                    sent.push(bytes.to_vec());
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
                role: LinkRole::Initiator,
                rtt_ms: 250,
                ..
            }),
        ));
        assert_eq!(
            delta.link_establishment_timeout,
            LaneWake::Idle,
            "activation clears the initiator's establishment deadline",
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
        assert_eq!(delta.link_establishment_timeout, LaneWake::Idle);

        let Some(LinkPhase::Active {
            key: initiator_key,
            role: LinkRole::Initiator,
            ..
        }) = initiator.links.phase_for(&link_id)
        else {
            panic!("the initiator must be active");
        };
        let Some(LinkPhase::Active {
            key: responder_key,
            role: LinkRole::Responder,
            rtt_ms: 500,
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
    fn an_authenticated_but_malformed_lrrtt_keeps_waiting_where_the_reference_tears_down() {
        let mut initiator = neighbor_with_a_route();
        let mut request = [0u8; BROADCAST_MTU];
        let dispatch = initiator
            .write_commanded_link_request(
                CommandId(7),
                &establish(),
                InstantMillis(1_000),
                vector_establish_entropy(),
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

        let (sent, journaled, _) = reactions_of(&mut responder, &frame, 1_600, 0xB6);
        assert!(sent.is_empty() && journaled.is_empty());
        assert!(
            matches!(
                responder.links.phase_for(&dispatch.link_id),
                Some(LinkPhase::Handshake { .. }),
            ),
            "only the establishment deadline forgets a half-open link; \
             revisit for exact parity when the teardown arc lands",
        );
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
                &mut buf,
            )
            .dispatched();

        let outcome = state.write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(2_000),
            vector_establish_entropy(),
            &mut buf,
        );
        assert!(matches!(
            outcome,
            EstablishLinkWriteOutcome::Failed {
                failure: WriteEstablishLinkError::DuplicateLinkId,
            },
        ));
        assert_eq!(state.links.len(), 1, "the original establishment stands");
    }
}
