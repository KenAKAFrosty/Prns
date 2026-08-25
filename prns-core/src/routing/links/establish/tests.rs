use super::*;
use crate::engine::test_support::*;
use crate::engine::IngestIo;
use crate::engine::{
    AnnounceAppData, AnnounceIngest, AnnounceNow, AnnounceTarget, DeferredCrypto, Directive,
    EngineReaction, EngineState, IgnoreReason, IngestPacketOutcome, IssuedCommand, Journaled,
    LinkEstablished, PacketReceiptDelivered, PrnsCommand, SendToLinkFailure, Settlement,
    WakeSchedule,
};
use crate::engine::{EstablishLinkFailure, WakeSchedules};
use crate::interfaces::{InboundPacket, InterfaceDescriptor};
use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
use crate::routing::dedup::PacketHash;
use crate::routing::links::handshake::parse_link_request;
use crate::routing::links::maintenance::{KEEPALIVE_ECHO, KEEPALIVE_REQUEST};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::table::LinkRole;
use crate::routing::upstream_app_destinations::LinkRequestPolicy;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::routing::RouteResponsiveness;
use crate::storage::TestFixedStorage;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, PropagationType, TransportId, WirePacketHeader};

impl EstablishLinkWriteOutcome {
    #[track_caller]
    fn dispatched(self) -> LinkRequestDispatch {
        match self {
            Self::Written(dispatch) => dispatch,
            Self::Rejected { rejection } => panic!("expected Written, got Rejected({rejection:?})"),
        }
    }
}

const PEER_DESTINATION_HEX: &str = "c3cfae69b36bb6e3bbfd96a3b5867a59";
const RESPONDER_TRANSPORT_ID: TransportId = TransportId::new([0x3C; 16]);

fn peer_destination() -> DestinationHash {
    DestinationHash::new(bytes_from_hex(PEER_DESTINATION_HEX).try_into().unwrap())
}

fn arrival() -> InterfaceId {
    InterfaceId::new([0xA1; 8])
}

fn arrival_interfaces() -> [InterfaceDescriptor; 1] {
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

fn hear_announce(state: &mut EngineState<TestStorageLayout>, wire: &[u8]) {
    let mut raw = wire.to_vec();
    let outcome = state.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(500),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert!(
        matches!(
            outcome,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ),
        "the announce fixture must take a route before linking",
    );
}

fn neighbor_with_a_route() -> EngineState<TestStorageLayout> {
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
            &mut test_fill_entropy,
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    assert_eq!(dispatch.fire_on, arrival());
    let parsed = parse_link_request(&buf[..dispatch.wire_bytes]).unwrap();
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
        WakeSchedule::At(InstantMillis(13_000)),
        "one direct hop arms first-hop + one per-hop increment",
    );
}

#[test]
fn an_establish_link_needs_a_known_route_and_takes_relayed_ones() {
    let mut state = EngineState::<TestStorageLayout>::new(second_secret_key());
    assert_eq!(
        state.ingest_command(
            IssuedCommand {
                id: CommandId(7),
                command: PrnsCommand::EstablishLink(establish()),
            },
            AttachedInterfaces::new(&arrival_interfaces()),
        ),
        CommandOutcome::EstablishLinkRejected {
            id: CommandId(7),
            rejection: EstablishLinkRejection::NoRouteToDestination,
        },
    );

    hear_announce(
        &mut state,
        &bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE),
    );
    let outcome = state.ingest_command(
        IssuedCommand {
            id: CommandId(8),
            command: PrnsCommand::EstablishLink(establish()),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
            command: PrnsCommand::EstablishLink(establish()),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        delta.link_deadlines,
        WakeSchedule::At(InstantMillis(13_000)),
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
            AttachedInterfaces::new(&arrival_interfaces()),
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
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |bytes: &mut [u8]| bytes.fill(0xE1),
        &mut |reaction| settled.extend(settled_of(reaction)),
    );
    assert!(settled.is_empty(), "the deadline has not passed yet");
    assert_eq!(
        early.link_deadlines,
        WakeSchedule::At(InstantMillis(13_000)),
    );

    let after = state.fire_due_link_deadlines(
        InstantMillis(13_000),
        AttachedInterfaces::new(&arrival_interfaces()),
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
    assert_eq!(after.link_deadlines, WakeSchedule::Idle);
    assert!(state.links.is_empty());
    assert_eq!(
        after.scheduled_announces,
        WakeSchedules::UNCHANGED.scheduled_announces,
        "only the link schedule moves",
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();
    assert_eq!(
        state
            .routing_table
            .existing_route_for(
                &peer_destination(),
                AttachedInterfaces::new(&arrival_interfaces())
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "the route is unconfirmed until a proof returns",
    );

    let _ = state.fire_due_link_deadlines(
        InstantMillis(13_000),
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |bytes: &mut [u8]| bytes.fill(0xE1),
        &mut |_| {},
    );

    assert_eq!(
        state
            .routing_table
            .existing_route_for(
                &peer_destination(),
                AttachedInterfaces::new(&arrival_interfaces())
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unresponsive,
        "our own link request that never established marks its destination unresponsive",
    );
}

#[test]
fn a_failed_parallel_link_attempt_yields_to_newer_inbound_evidence() {
    let mut state = neighbor_with_a_route();
    let mut first_request = [0u8; BROADCAST_MTU];
    let first = state
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut first_request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(
        &mut responder,
        &first_request[..first.wire_bytes],
        1_100,
        0x99,
    );
    let _ = reactions_of(&mut state, &proofs[0], 1_250, 0xA5);

    let mut second_entropy = [0x66; EstablishLinkEntropy::LEN];
    second_entropy[32..].fill(0x55);
    let mut second_request = [0u8; BROADCAST_MTU];
    let second = state
        .write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(2_000),
            EstablishLinkEntropy::new(second_entropy),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut second_request,
        )
        .dispatched();
    assert_ne!(first.link_id, second.link_id);

    state
        .links
        .note_inbound(&first.link_id, InstantMillis(3_000));
    let _ = state.fire_due_link_deadlines(
        InstantMillis(14_000),
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |bytes: &mut [u8]| bytes.fill(0),
        &mut |_| {},
    );

    let row = state.routing_table.path_row(&peer_destination()).unwrap();
    assert_eq!(row.last_route_activity_at, InstantMillis(3_000));
    assert_eq!(
        row.responsiveness,
        RouteResponsiveness::Responsive,
        "a failed parallel attempt cannot overwrite stronger evidence received after it began",
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    assert_eq!(
        initiator
            .routing_table
            .existing_route_for(
                &peer_destination(),
                AttachedInterfaces::new(&arrival_interfaces())
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
    );

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let _ = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);

    assert_eq!(
        initiator
            .routing_table
            .existing_route_for(
                &peer_destination(),
                AttachedInterfaces::new(&arrival_interfaces())
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Responsive,
        "the initiator's link reaching active confirms its destination's route",
    );
    assert_eq!(
        initiator
            .routing_table
            .path_row(&peer_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(1_250),
        "the proof arrival, rather than later completion work, is the route observation",
    );
}

#[test]
fn route_culling_reconciles_a_links_inbound_burst_before_deciding() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let _ = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);

    initiator
        .links
        .note_inbound(&dispatch.link_id, InstantMillis(4_000));
    initiator
        .links
        .note_inbound(&dispatch.link_id, InstantMillis(5_000));
    assert_eq!(
        initiator
            .routing_table
            .path_row(&peer_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(1_250),
        "hot traffic remains coalesced in Link state until an engine decision boundary",
    );

    let would_have_expired_without_reconciliation =
        InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS + 2_000);
    let _ = initiator.cull_expired_routes(
        would_have_expired_without_reconciliation,
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
    );
    assert_eq!(
        initiator
            .routing_table
            .path_row(&peer_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(5_000),
        "culling promotes the newest observation before it evaluates expiry",
    );
    assert_eq!(initiator.reconcile_pending_link_route_evidence(), 0);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let identity = responder.held_identity_hashes()[0];
    let mut raw = buf[..dispatch.wire_bytes].to_vec();
    let outcome = responder.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::OwesLinkProof(AcceptedLinkRequest {
            request: parse_link_request(&buf[..dispatch.wire_bytes]).unwrap(),
            identity,
            proof_strategy: crate::routing::upstream_app_destinations::ProofStrategy::ProveNone,
            received_hops: 1,
            arrived_at: InstantMillis(2_000),
        }),
    );

    let mut replay = buf[..dispatch.wire_bytes].to_vec();
    let replayed = responder.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_100),
            source_interface: arrival(),
            bytes: &mut replay,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        replayed,
        IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
        "a replayed request deduplicates away",
    );
}

#[test]
fn a_full_responder_link_table_withholds_the_proof_and_preserves_its_row() {
    type OneLinkStorage = TestFixedStorage<64, 64, 4096, 8, 8, 128, 8, 8, 8, 8, 16, 1>;

    let mut initiator = neighbor_with_a_route();
    let make_request =
        |initiator: &mut EngineState<TestStorageLayout>, command_id, entropy_fill: u8| {
            let mut entropy = [entropy_fill; EstablishLinkEntropy::LEN];
            entropy[32..].fill(entropy_fill.wrapping_add(1));
            let mut wire = [0u8; BROADCAST_MTU];
            let dispatch = initiator
                .write_commanded_link_request(
                    CommandId(command_id),
                    &establish(),
                    InstantMillis(1_000 + command_id),
                    EstablishLinkEntropy::new(entropy),
                    AttachedInterfaces::new(&arrival_interfaces()),
                    &mut wire,
                )
                .dispatched();
            (wire[..dispatch.wire_bytes].to_vec(), dispatch.link_id)
        };

    let mut responder = EngineState::<OneLinkStorage>::new(fixed_secret_key());
    let identity = responder.held_identity_hashes()[0];
    responder
        .register_single_destination(
            &identity,
            "personal",
            &["node"],
            b"hello-personal",
            ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptAll,
            crate::engine::RatchetPolicy::NoRatchets,
        )
        .unwrap();

    let accept = |responder: &mut EngineState<OneLinkStorage>, wire: &mut [u8], arrived_at| {
        let outcome = responder.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(arrived_at),
                source_interface: arrival(),
                bytes: wire,
            },
            &mut |_| {},
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut |_| {},
            None,
        );
        let IngestPacketOutcome::OwesLinkProof(accepted) = outcome else {
            panic!("the local destination must accept this link request");
        };
        accepted
    };

    let (mut first_wire, first_link_id) = make_request(&mut initiator, 7, 0x71);
    let first = accept(&mut responder, &mut first_wire, 2_000);
    let mut proof = [0u8; BROADCAST_MTU];
    responder
        .write_owed_link_proof(
            &first,
            X25519SecretKey::new([0x81; X25519SecretKey::LEN]),
            BROADCAST_MTU,
            &mut proof,
        )
        .unwrap();
    assert_eq!(responder.links.len(), 1);

    let (mut second_wire, second_link_id) = make_request(&mut initiator, 8, 0x72);
    let second = accept(&mut responder, &mut second_wire, 3_000);
    assert_eq!(
        responder.write_owed_link_proof(
            &second,
            X25519SecretKey::new([0x82; X25519SecretKey::LEN]),
            BROADCAST_MTU,
            &mut proof,
        ),
        Err(WriteLinkProofError::LinkTableFull),
    );
    assert_eq!(responder.links.len(), 1);
    assert!(responder.links.phase_for(&first_link_id).is_some());
    assert!(responder.links.phase_for(&second_link_id).is_none());
    let (mut third_wire, third_link_id) = make_request(&mut initiator, 9, 0x73);
    let mut sent = std::vec::Vec::new();
    responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(4_000),
            source_interface: arrival(),
            bytes: &mut third_wire,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(4_000),
            fill_entropy: &mut |bytes| bytes.fill(0x83),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    sent.push(bytes.to_vec());
                }
            },
        },
    );

    assert!(sent.is_empty(), "a proof cannot escape without a link row");
    assert_eq!(responder.links.len(), 1);
    assert!(responder.links.phase_for(&first_link_id).is_some());
    assert!(responder.links.phase_for(&third_link_id).is_none());
}

#[test]
fn a_foreign_stamped_link_request_is_not_delivered_to_a_local_destination() {
    let mut initiator = neighbor_with_a_route();
    let mut direct = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut direct,
        )
        .dispatched();
    let direct = &direct[..dispatch.wire_bytes];
    let (header, payload) = WirePacketHeader::parse(direct).unwrap();
    let foreign_header = WirePacketHeader {
        propagation: PropagationType::Transport,
        transport_id: Some(TEST_TRANSPORT_ID),
        ..header
    };
    let mut foreign = [0u8; BROADCAST_MTU];
    let foreign_header_len = foreign_header.write(&mut foreign).unwrap();
    foreign[foreign_header_len..foreign_header_len + payload.len()].copy_from_slice(payload);
    let foreign_len = foreign_header_len + payload.len();

    let mut responder = personal_node_announcer();
    pin_transport_id(&mut responder, RESPONDER_TRANSPORT_ID);
    let outcome = responder.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: arrival(),
            bytes: &mut foreign[..foreign_len],
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::OtherInstance),
    );

    let mut direct = direct.to_vec();
    let outcome = responder.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_100),
            source_interface: arrival(),
            bytes: &mut direct,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert!(
        matches!(outcome, IngestPacketOutcome::OwesLinkProof(_)),
        "the foreign copy must not consume dedup before the direct copy arrives",
    );
}

#[test]
fn an_accept_none_destination_announces_but_refuses_the_link_request() {
    let mut initiator = neighbor_with_a_route();
    let mut buf = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    let mut responder = EngineState::<TestStorageLayout>::new(fixed_secret_key());
    let node = responder.held_identity_hashes()[0];
    responder
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            b"hello-personal",
            crate::routing::upstream_app_destinations::ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptNone,
            crate::crypto::ratchets::RatchetPolicy::NoRatchets,
        )
        .unwrap();

    let mut raw = buf[..dispatch.wire_bytes].to_vec();
    let outcome = responder.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::LinkRequestsRefused),
        "the destination is registered and held, yet answers no link request",
    );
    assert!(responder.links.is_empty());
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    let mut bystander = EngineState::<TestStorageLayout>::new(second_secret_key());
    let mut raw = buf[..dispatch.wire_bytes].to_vec();
    let outcome = bystander.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::NotForUs)
    );
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let mut sent = std::vec::Vec::new();
    let mut raw = buf[..dispatch.wire_bytes].to_vec();
    let delta = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_000),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0x99),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
    assert_eq!(
        delta.link_deadlines,
        WakeSchedule::At(InstantMillis(368_000)),
    );
}

#[test]
fn deferred_link_proof_sign_and_verify_resume_the_handshake() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let mut raw_request = request[..dispatch.wire_bytes].to_vec();
    let mut deferred_sign = None;
    let mut deferred = DeferredCrypto::default();
    responder.ingest_packet_into_deferring(
        InboundPacket {
            arrived_at: InstantMillis(1_100),
            source_interface: arrival(),
            bytes: &mut raw_request,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(1_100),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0x99),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |_| {},
        },
        &mut deferred_sign,
        Some(&mut deferred),
    );
    let DeferredCrypto::LinkProofSign(owed) = deferred else {
        panic!("the responder captures the proof signature for the pool");
    };
    let responder_encryption = x25519_public_key(&owed.ephemeral_secret);
    let shared = x25519_diffie_hellman(&owed.ephemeral_secret, &owed.request.initiator_encryption);
    let signed_data = crate::routing::links::handshake::link_proof_signed_data(
        &owed.request.link_id,
        &responder_encryption,
        owed.responder_signing.as_ed25519(),
        owed.mtu,
        owed.request.mode,
    );
    let signature = crate::crypto::ed25519_sign(&owed.signing_secret, &signed_data);
    let mut proofs = std::vec::Vec::new();
    let sign_wake = responder.resume_link_proof_sign(
        owed,
        responder_encryption,
        shared,
        signature,
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                proofs.push((target, bytes.to_vec()));
            }
        },
    );
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].0, arrival());
    assert!(matches!(sign_wake.link_deadlines, WakeSchedule::At(_)));

    let mut raw_proof = proofs[0].1.clone();
    raw_proof[1] = 2;
    let mut deferred_sign = None;
    let mut deferred = DeferredCrypto::default();
    initiator.ingest_packet_into_deferring(
        InboundPacket {
            arrived_at: InstantMillis(1_250),
            source_interface: arrival(),
            bytes: &mut raw_proof,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(1_250),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |_| {},
        },
        &mut deferred_sign,
        Some(&mut deferred),
    );
    let DeferredCrypto::LinkProofVerify(mut owed) = deferred else {
        panic!("the initiator captures proof verification for the pool");
    };
    assert_eq!(owed.received_hops, 3);
    assert!(crate::routing::links::handshake::link_proof_signature_valid(&owed));
    owed.signed_data[0] ^= 0x01;
    assert!(!crate::routing::links::handshake::link_proof_signature_valid(&owed));
    owed.signed_data[0] ^= 0x01;
    let shared = x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption);
    let mut rtts = std::vec::Vec::new();
    let mut settlements = std::vec::Vec::new();
    let verify_wake = initiator.resume_link_proof(
        owed,
        shared,
        AttachedInterfaces::new(&arrival_interfaces()),
        InstantMillis(5_000),
        &mut |bytes: &mut [u8]| bytes.fill(0xA5),
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                rtts.push((target, bytes.to_vec()));
            }
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                settlements.push((id, settlement));
            }
            _ => {}
        },
    );
    assert_eq!(rtts.len(), 1);
    assert_eq!(rtts[0].0, arrival());
    assert_eq!(
        initiator
            .routing_table
            .stored_announce_for(&personal_node_destination())
            .unwrap()
            .hops,
        3,
    );
    assert_eq!(
        initiator
            .routing_table
            .path_row(&personal_node_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(1_250),
        "deferred completion credits the proof's arrival, not worker latency",
    );
    assert_eq!(
        settlements,
        std::vec![(
            CommandId(7),
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: dispatch.link_id,
                rtt_millis: 250,
            })),
        )],
    );
    assert!(matches!(verify_wake.link_deadlines, WakeSchedule::At(_)));
}

fn reactions_of(
    engine: &mut EngineState<TestStorageLayout>,
    bytes: &[u8],
    arrived_at: u64,
    iv_fill: u8,
) -> (
    std::vec::Vec<std::vec::Vec<u8>>,
    std::vec::Vec<(CommandId, Settlement)>,
    WakeSchedules,
) {
    reactions_of_on(
        engine,
        bytes,
        arrived_at,
        iv_fill,
        AttachedInterfaces::new(&arrival_interfaces()),
    )
}

fn reactions_of_on(
    engine: &mut EngineState<TestStorageLayout>,
    bytes: &[u8],
    arrived_at: u64,
    iv_fill: u8,
    interfaces: AttachedInterfaces<'_>,
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
        IngestIo {
            interfaces,
            now: InstantMillis(arrived_at),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = personal_node_announcer();
    let (proofs, journaled, _) =
        reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
                rtt_millis: 250,
            })),
        )],
        "the command settles established with the measured round trip",
    );
    assert!(matches!(
        initiator.links.phase_for(&link_id),
        Some(LinkPhase::Active {
            role: LinkRole::Initiator { .. },
            rtt,
            ..
        }) if *rtt == RttMillis::new(250),
    ));
    assert_eq!(
        delta.link_deadlines,
        WakeSchedule::At(InstantMillis(1_250 + 51_428)),
        "activation swaps the establishment deadline for the keepalive one",
    );

    let (replay_sent, replay_journaled, _) = reactions_of(&mut initiator, &proofs[0], 1_300, 0xA5);
    assert!(replay_sent.is_empty() && replay_journaled.is_empty());

    let (responder_sent, established, delta) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);
    assert!(responder_sent.is_empty(), "activation answers nothing back");
    assert_eq!(
        established,
        std::vec![(
            CommandId(u64::MAX),
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id,
                rtt_millis: 500,
            })),
        )],
        "the responder journals the link up at max(measured, reported)",
    );
    assert_eq!(
        delta.link_deadlines,
        WakeSchedule::At(InstantMillis(1_600 + 205_714 + 7_000)),
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
        rtt,
        ..
    }) = responder.links.phase_for(&link_id)
    else {
        panic!("the responder must be active");
    };
    assert_eq!(
        *rtt,
        RttMillis::new(500),
        "the responder settles at the measured rtt",
    );
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
fn a_signed_hop_change_rebalances_an_initiated_link_and_its_route() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (mut proofs, _, _) =
        reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    proofs[0][1] = 2;

    let (rtts, settled, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);

    assert_eq!(rtts.len(), 1);
    assert!(matches!(
        settled.as_slice(),
        [(CommandId(7), Settlement::EstablishLink(Ok(_)))]
    ));
    assert!(matches!(
        initiator.links.phase_for(&dispatch.link_id),
        Some(LinkPhase::Active { .. })
    ));
    assert_eq!(
        initiator
            .routing_table
            .stored_announce_for(&personal_node_destination())
            .unwrap()
            .hops,
        3,
    );
}

#[test]
fn untrusted_initiated_hop_changes_leave_the_path_unchanged() {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let mut proof = proofs[0].clone();
    proof[1] = 2;

    let mut wrong_mode = proof.clone();
    let signalling = wrong_mode.len() - 3;
    wrong_mode[signalling] = (wrong_mode[signalling] & 0x1F) | 0x40;
    let outcome = initiator.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_240),
            source_interface: arrival(),
            bytes: &mut wrong_mode,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );

    let payload_offset = {
        let (_, payload) = crate::wire::WirePacketHeader::parse(&proof).unwrap();
        proof.len() - payload.len()
    };
    proof[payload_offset] ^= 0x01;

    let outcome = initiator.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_250),
            source_interface: arrival(),
            bytes: &mut proof,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );

    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );
    assert!(matches!(
        initiator.links.phase_for(&dispatch.link_id),
        Some(LinkPhase::Pending {
            expected_hops: 1,
            ..
        })
    ));
    assert_eq!(
        initiator
            .routing_table
            .stored_announce_for(&personal_node_destination())
            .unwrap()
            .hops,
        1,
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);

    let mut bystander = EngineState::<TestStorageLayout>::new(second_secret_key());
    let mut raw = proofs[0].clone();
    let outcome = bystander.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_250),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::UnknownLink)
    );
}

fn responder_awaiting_lrrtt() -> (EngineState<TestStorageLayout>, LinkRequestDispatch) {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let (_, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    (responder, dispatch)
}

fn encrypted_lrrtt_frame(
    responder: &EngineState<TestStorageLayout>,
    link_id: LinkId,
    plaintext: &[u8],
) -> std::vec::Vec<u8> {
    let Some(LinkPhase::Handshake { key, .. }) = responder.links.phase_for(&link_id) else {
        panic!("the responder must be awaiting its LRRTT");
    };
    let mut frame = std::vec![0x0Cu8, 0x00];
    frame.extend_from_slice(link_id.as_bytes());
    frame.push(0xFE);
    let mut sealed = [0u8; 64];
    let n = key
        .seal(&test_entropy_bytes::<16>(0xB5), plaintext, &mut sealed)
        .unwrap();
    frame.extend_from_slice(&sealed[..n]);
    frame
}

struct AuthenticatedNumericLrrttCase {
    plaintext: &'static [u8],
    expected_rtt_millis: u64,
}

const FLOAT32_THREE_QUARTER_SECOND_LRRTT: AuthenticatedNumericLrrttCase =
    AuthenticatedNumericLrrttCase {
        plaintext: &[0xCA, 0x3F, 0x40, 0x00, 0x00],
        expected_rtt_millis: 750,
    };
const POSITIVE_FIXINT_ONE_SECOND_LRRTT: AuthenticatedNumericLrrttCase =
    AuthenticatedNumericLrrttCase {
        plaintext: &[0x01],
        expected_rtt_millis: 1_000,
    };

fn assert_numeric_lrrtt_activates(case: &AuthenticatedNumericLrrttCase) {
    let (mut responder, dispatch) = responder_awaiting_lrrtt();
    let frame = encrypted_lrrtt_frame(&responder, dispatch.link_id, case.plaintext);

    let (sent, established, _) = reactions_of(&mut responder, &frame, 1_600, 0xB5);
    assert!(sent.is_empty(), "activation answers nothing back");
    assert_eq!(
        established,
        std::vec![(
            CommandId(u64::MAX),
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: dispatch.link_id,
                rtt_millis: case.expected_rtt_millis,
            })),
        )],
    );
    assert!(matches!(
        responder.links.phase_for(&dispatch.link_id),
        Some(LinkPhase::Active {
            role: LinkRole::Responder { .. },
            rtt,
            ..
        }) if *rtt == RttMillis::new(case.expected_rtt_millis),
    ));
}

#[test]
fn an_authenticated_float32_lrrtt_activates_the_responding_link() {
    assert_numeric_lrrtt_activates(&FLOAT32_THREE_QUARTER_SECOND_LRRTT);
}

#[test]
fn an_authenticated_integer_lrrtt_activates_the_responding_link() {
    assert_numeric_lrrtt_activates(&POSITIVE_FIXINT_ONE_SECOND_LRRTT);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
    use crate::engine::LinkClosedReason;
    use crate::wire::WirePacketHeader;

    let (mut responder, dispatch) = responder_awaiting_lrrtt();
    let mut not_msgpack = [0xC1u8; 9];
    not_msgpack[1..].fill(0x55);
    let frame = encrypted_lrrtt_frame(&responder, dispatch.link_id, &not_msgpack);

    let mut closes = std::vec::Vec::new();
    let mut journaled = std::vec::Vec::new();
    let mut raw = frame.clone();
    let _ = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(1_600),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(1_600),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xB6),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
        std::vec![(dispatch.link_id, LinkClosedReason::MalformedRtt)],
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
    assert_eq!(header.address.as_bytes(), dispatch.link_id.as_bytes());
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

    let mut sent = std::vec::Vec::new();
    let mut settled = std::vec::Vec::new();
    let _ = initiator.ingest_command_into(
        IssuedCommand {
            id: CommandId(9),
            command: PrnsCommand::SendToLink(SendToLink {
                link_id,
                payload: SendToLinkPayload::from_slice(b"hello over the link").unwrap(),
            }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_100),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD2),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_200),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD3),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::Delivered(_)) = reaction {
                    replayed.push(());
                }
            },
        },
    );
    assert!(replayed.is_empty(), "a replayed frame deduplicates away");
}

fn proving_node_announcer(strategy: ProofStrategy) -> EngineState<TestStorageLayout> {
    use crate::engine::RatchetPolicy;

    let mut state: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
    let node = state.held_identity_hashes()[0];
    state
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            b"hello-personal",
            strategy,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .unwrap();
    state
}

fn commanded_link_data(
    engine: &mut EngineState<TestStorageLayout>,
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
            command: PrnsCommand::SendToLink(SendToLink {
                link_id,
                payload: SendToLinkPayload::from_slice(payload).unwrap(),
            }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

    let data = commanded_link_data(&mut initiator, link_id, b"prove this", 2_000, 0xD1);
    let (answers, _, _) = reactions_of(&mut responder, &data, 2_100, 0xD2);
    assert_eq!(answers.len(), 1, "the ProveAll responder answers a proof");

    let (header, payload) = WirePacketHeader::parse(&answers[0]).unwrap();
    assert_eq!(header.packet_type, PacketType::Proof);
    assert_eq!(header.destination_type, DestinationType::Link);
    assert_eq!(header.address.as_bytes(), link_id.as_bytes());
    assert_eq!(header.context, WireContext::None);
    assert_eq!(header.hops, 0);
    assert_eq!(payload.len(), EXPLICIT_PROOF_PAYLOAD_LEN);

    let (data_header, data_payload) = WirePacketHeader::parse(&data).unwrap();
    let expected_hash = PacketHash::of_fields(
        DestinationType::Link,
        PacketType::Data,
        &data_header.address,
        data_header.context,
        data_payload,
    );
    assert_eq!(
        &payload[..32],
        expected_hash.as_bytes(),
        "the proof names the ciphertext frame's packet hash",
    );

    let Some(LinkPhase::Active { peer_signing, .. }) = initiator.links.phase_for(&link_id) else {
        panic!("the initiator holds the active link");
    };
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&payload[32..]);
    ed25519_verify(peer_signing, &payload[..32], &Ed25519Signature(signature))
        .expect("the proof validates against the announced identity the initiator already holds");

    let proof_frame = answers[0].clone();
    let proof_packet_hash = PacketHash::of_wire_packet(&proof_frame).unwrap();
    let (echoes, journaled, _) = reactions_of(&mut initiator, &proof_frame, 2_200, 0xF1);
    assert!(echoes.is_empty(), "a proof is an ending, not a beginning");
    assert_eq!(
        journaled,
        std::vec![(
            CommandId(9),
            Settlement::SendToLink(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(200),
                evidence: crate::engine::DeliveryEvidence::Proof(
                    crate::engine::DeliveryProof::Explicit(proof_packet_hash),
                ),
            })),
        )],
        "the receipt settles the send with the proof's round trip",
    );
    assert!(
        initiator.receipts.is_empty(),
        "settlement removes the receipt — a replayed proof finds nothing",
    );
    let Some(LinkPhase::Active { last_inbound, .. }) = initiator.links.phase_for(&link_id) else {
        panic!("the proven link remains active");
    };
    assert_eq!(
        *last_inbound,
        InstantMillis(2_200),
        "a delivery proof is inbound link traffic and refreshes its liveness clock",
    );
    let mut observed = None;
    initiator
        .links
        .reconcile_pending_route_evidence(|_, at| observed = Some(at));
    assert_eq!(
        observed,
        Some(InstantMillis(2_200)),
        "a verified Link receipt is also pending route evidence",
    );
    assert_eq!(
        initiator.links.pop_stale(InstantMillis(17_199)),
        None,
        "the link cannot expire on the pre-proof liveness deadline",
    );
}

#[test]
fn a_deferred_link_proof_updates_inbound_only_after_verification() {
    use crate::crypto::Ed25519Verifier;
    use crate::routing::proof::ProofIngest;
    use crate::wire::WirePacketHeader;

    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);
    initiator.links.reconcile_pending_route_evidence(|_, _| {});

    let data = commanded_link_data(&mut initiator, link_id, b"prove this", 2_000, 0xD1);
    let (answers, _, _) = reactions_of(&mut responder, &data, 2_100, 0xD2);
    let proof = &answers[0];
    let (header, payload) = WirePacketHeader::parse(proof).unwrap();
    let deferred = initiator
        .settle_receipt_proof_deferred(
            payload,
            &DestinationHash::from_address(header.address),
            PacketHash::of_wire_packet(proof).unwrap(),
            InstantMillis(2_200),
        )
        .expect("the explicit proof resolves the link receipt");

    let Some(LinkPhase::Active { last_inbound, .. }) = initiator.links.phase_for(&link_id) else {
        panic!("the initiator link remains active");
    };
    assert_eq!(
        *last_inbound,
        InstantMillis(1_250),
        "resolving an unverified proof cannot touch Link liveness",
    );
    assert_eq!(initiator.receipts.len(), 1);
    let mut observed = None;
    initiator
        .links
        .reconcile_pending_route_evidence(|_, at| observed = Some(at));
    assert_eq!(observed, None, "unverified traffic is not route evidence");

    assert!(Ed25519Verifier::new(deferred.signing_key.as_ed25519())
        .unwrap()
        .verify(deferred.packet_hash.as_bytes(), &deferred.signature)
        .is_ok(),);
    let ProofIngest::SendToLinkDelivered { id, .. } = deferred.ingest else {
        panic!("the deferred proof belongs to the Link send");
    };
    assert_eq!(
        initiator.settle_resolved_receipt_proof(id, &deferred.packet_hash, deferred.arrived_at,),
        crate::engine::ResolvedReceiptSettlement::Settled,
    );

    let Some(LinkPhase::Active { last_inbound, .. }) = initiator.links.phase_for(&link_id) else {
        panic!("the proven link remains active");
    };
    assert_eq!(*last_inbound, InstantMillis(2_200));
    let mut observed = None;
    initiator
        .links
        .reconcile_pending_route_evidence(|_, at| observed = Some(at));
    assert_eq!(observed, Some(InstantMillis(2_200)));
    assert!(initiator.receipts.is_empty());
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

    let _ = commanded_link_data(&mut initiator, link_id, b"never proven", 2_000, 0xD1);
    assert_eq!(
        initiator.receipt_timeouts_wake(),
        WakeSchedule::At(InstantMillis(4_500)),
        "the deadline is max(rtt × 6, 5 ms) plus the reference's receipt-cull slack past the send: 2_000 + 250 × 6 + 1_000",
    );

    let mut settled = std::vec::Vec::new();
    let mut collect = |reaction: EngineReaction<'_>| {
        if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) = reaction {
            settled.push((id, settlement));
        }
    };
    let pending = initiator.settle_timed_out_receipts(InstantMillis(4_499), &mut collect);
    assert_eq!(
        pending.receipt_timeouts,
        WakeSchedule::At(InstantMillis(4_500)),
    );
    let expired = initiator.settle_timed_out_receipts(InstantMillis(4_500), &mut collect);
    assert_eq!(expired.receipt_timeouts, WakeSchedule::Idle);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = proving_node_announcer(ProofStrategy::ProveAll);
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = proving_node_announcer(ProofStrategy::ProveIf);
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
            IngestIo {
                interfaces: AttachedInterfaces::new(&arrival_interfaces()),
                now: InstantMillis(arrived_at),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xD2),
                should_prove: &mut |request: &ProofRequest| {
                    requests.push((request.destination, request.plaintext.to_vec()));
                    agree
                },
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
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

fn relay_that_routes_to_the_responder(
    iface_to_b: InterfaceId,
) -> (
    EngineState<TestStorageLayout>,
    EngineState<TestStorageLayout>,
) {
    let mut relay = EngineState::<TestStorageLayout>::new(fixed_secret_key());
    pin_transport_id(&mut relay, TEST_TRANSPORT_ID);
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
            &mut test_fill_entropy,
            &mut announce_buf,
        )
        .written_len();
    let relay_view = [
        routable_descriptor(arrival()),
        routable_descriptor(iface_to_b),
    ];
    let mut raw = announce_buf[..announce_len].to_vec();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(500),
            source_interface: iface_to_b,
            bytes: &mut raw,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert!(
        matches!(
            outcome,
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_))
        ),
        "the responder's announce must teach the relay its route",
    );
    (relay, responder)
}

fn transported_request_wire(initiator: &mut EngineState<TestStorageLayout>) -> std::vec::Vec<u8> {
    hear_announce(initiator, &bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE));
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    request[..dispatch.wire_bytes].to_vec()
}

#[test]
fn a_duplicate_transported_link_request_is_dropped_as_a_duplicate() {
    let iface_to_b = InterfaceId::new([0xB7; 8]);
    let relay_view = [
        routable_descriptor(arrival()),
        routable_descriptor(iface_to_b),
    ];
    let (mut relay, _responder) = relay_that_routes_to_the_responder(iface_to_b);
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let request = transported_request_wire(&mut initiator);

    let mut first = request.clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_100),
            source_interface: arrival(),
            bytes: &mut first,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert!(
        matches!(outcome, IngestPacketOutcome::TransportedLinkRequest { .. }),
        "the first request rides transport",
    );

    let mut second = request.clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_150),
            source_interface: arrival(),
            bytes: &mut second,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::Duplicate),
        "RNS 1.4.2 remembers transported link requests; the echo is a duplicate, not a capacity event",
    );
}

#[test]
fn a_transported_proof_needs_the_destinations_signing_key() {
    let iface_to_b = InterfaceId::new([0xB7; 8]);
    let relay_view = [
        routable_descriptor(arrival()),
        routable_descriptor(iface_to_b),
    ];
    let (mut relay, mut responder) = relay_that_routes_to_the_responder(iface_to_b);
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let request = transported_request_wire(&mut initiator);

    let mut inbound = request.clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_100),
            source_interface: arrival(),
            bytes: &mut inbound,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    let IngestPacketOutcome::TransportedLinkRequest { header, body, .. } = outcome else {
        panic!("the request must ride transport");
    };
    let mut forwarded = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut forwarded).unwrap();
    forwarded[header_len..header_len + body.as_bytes().len()].copy_from_slice(body.as_bytes());
    let forwarded_len = header_len + body.as_bytes().len();

    let (proofs, _, _) = reactions_of(&mut responder, &forwarded[..forwarded_len], 1_200, 0x99);

    let one_week_on = InstantMillis(1_000 + 7 * 24 * 60 * 60 * 1_000 + 1);
    let _ = relay.cull_expired_routes(
        one_week_on,
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
    );
    assert!(
        relay
            .routing_table
            .stored_announce_for(&personal_node_destination())
            .is_none(),
        "the relay's announce for the destination is gone mid-establishment",
    );

    let mut proof = proofs[0].clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_300),
            source_interface: iface_to_b,
            bytes: &mut proof,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::UnknownIdentity),
        "Prns does not unlock opaque transported Link traffic from proof shape alone",
    );
    assert!(
        !relay
            .transported_links
            .entry_for(&parse_link_request(&request).unwrap().link_id)
            .unwrap()
            .validated_by_proof,
        "the unverified transported row stays proof-gated",
    );
}

#[test]
fn a_trusted_transported_hop_change_rebalances_the_link_and_route_once() {
    let iface_to_b = InterfaceId::new([0xB7; 8]);
    let relay_view = [
        routable_descriptor(arrival()),
        routable_descriptor(iface_to_b),
    ];
    let (mut relay, mut responder) = relay_that_routes_to_the_responder(iface_to_b);
    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    let request = transported_request_wire(&mut initiator);
    let link_id = parse_link_request(&request).unwrap().link_id;

    let mut inbound = request.clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_100),
            source_interface: arrival(),
            bytes: &mut inbound,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    let IngestPacketOutcome::TransportedLinkRequest { header, body, .. } = outcome else {
        panic!("the request must ride transport");
    };
    let mut forwarded = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut forwarded).unwrap();
    forwarded[header_len..header_len + body.as_bytes().len()].copy_from_slice(body.as_bytes());
    let forwarded_len = header_len + body.as_bytes().len();
    let (proofs, _, _) = reactions_of(&mut responder, &forwarded[..forwarded_len], 1_200, 0x99);
    let mut valid = proofs[0].clone();
    valid[1] = 2;

    let assert_unchanged = |relay: &EngineState<TestStorageLayout>| {
        let entry = relay.transported_links.entry_for(&link_id).unwrap();
        assert_eq!((entry.remaining_hops, entry.validated_by_proof), (1, false));
        assert_eq!(
            relay
                .routing_table
                .stored_announce_for(&personal_node_destination())
                .unwrap()
                .hops,
            1,
        );
    };

    let mut malformed = valid.clone();
    malformed.pop();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_250),
            source_interface: iface_to_b,
            bytes: &mut malformed,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::Malformed)
    );
    assert_unchanged(&relay);

    let mut wrong_mode = valid.clone();
    let signalling = wrong_mode.len() - 3;
    wrong_mode[signalling] = (wrong_mode[signalling] & 0x1F) | 0x40;
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_260),
            source_interface: iface_to_b,
            bytes: &mut wrong_mode,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );
    assert_unchanged(&relay);

    let mut invalid_signature = valid.clone();
    let payload_offset = {
        let (_, payload) = crate::wire::WirePacketHeader::parse(&invalid_signature).unwrap();
        invalid_signature.len() - payload.len()
    };
    invalid_signature[payload_offset] ^= 0x01;
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_270),
            source_interface: iface_to_b,
            bytes: &mut invalid_signature,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );
    assert_unchanged(&relay);

    let mut wrong_ingress = valid.clone();
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_280),
            source_interface: arrival(),
            bytes: &mut wrong_ingress,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );
    assert_unchanged(&relay);

    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_300),
            source_interface: iface_to_b,
            bytes: &mut valid,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    let IngestPacketOutcome::Forward(forwarded) = outcome else {
        panic!("the trusted proof must return toward the initiator");
    };
    assert_eq!(forwarded.fire_on, arrival());
    let entry = relay.transported_links.entry_for(&link_id).unwrap();
    assert_eq!((entry.remaining_hops, entry.validated_by_proof), (3, true));
    assert_eq!(
        relay
            .routing_table
            .stored_announce_for(&personal_node_destination())
            .unwrap()
            .hops,
        3,
    );

    let mut second = valid.clone();
    second[1] = 3;
    let outcome = relay.ingest_packet_with(
        InboundPacket {
            arrived_at: InstantMillis(1_350),
            source_interface: iface_to_b,
            bytes: &mut second,
        },
        &mut |_| {},
        AttachedInterfaces::new(&relay_view),
        &mut |_| {},
        None,
    );
    assert_eq!(
        outcome,
        IngestPacketOutcome::Ignored(IgnoreReason::ProofInvalid)
    );
    assert_eq!(
        relay
            .transported_links
            .entry_for(&link_id)
            .unwrap()
            .remaining_hops,
        3,
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

    let mut relay = EngineState::<TestStorageLayout>::new(fixed_secret_key());
    pin_transport_id(&mut relay, TEST_TRANSPORT_ID);
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
            &mut test_fill_entropy,
            &mut announce_buf,
        )
        .written_len();
    let ingest_via = |engine: &mut EngineState<TestStorageLayout>,
                      bytes: &[u8],
                      iface: InterfaceId,
                      now: u64,
                      iv_fill: u8,
                      interfaces: AttachedInterfaces<'_>| {
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
            IngestIo {
                interfaces,
                now: InstantMillis(now),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::Send { target, bytes }) => {
                        sent.push((target, bytes.to_vec()));
                    }
                    EngineReaction::Directive(Directive::EmitFrame { target, fill, .. }) => {
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
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(
        relay
            .routing_table
            .existing_route_for(
                &personal_node_destination(),
                AttachedInterfaces::new(&relay_view)
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "the relay's freshly heard route to B is unconfirmed",
    );

    let mut initiator = EngineState::<TestStorageLayout>::new(second_secret_key());
    hear_announce(
        &mut initiator,
        &bytes_from_hex(RNS_1_4_2_RETRANSMITTED_ANNOUNCE),
    );

    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let (switched, _, _, _) = ingest_via(
        &mut relay,
        &request[..dispatch.wire_bytes],
        iface_to_a,
        1_100,
        0x20,
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched.len(), 1, "the relay forwards the link request");
    assert_eq!(switched[0].0, iface_to_b);
    assert!(
        relay.transported_links.entry_for(&link_id).is_some(),
        "the relay carries the pending link",
    );

    let (proofs, _, _) = reactions_of(&mut responder, &switched[0].1, 1_200, 0x99);
    let mut forged_proof = proofs[0].clone();
    let payload_offset = {
        let (_, payload) = crate::wire::WirePacketHeader::parse(&forged_proof).unwrap();
        forged_proof.len() - payload.len()
    };
    forged_proof[payload_offset] ^= 0x01;
    let (forged_return, _, _, _) = ingest_via(
        &mut relay,
        &forged_proof,
        iface_to_b,
        1_250,
        0x2F,
        AttachedInterfaces::new(&relay_view),
    );
    assert!(
        forged_return.is_empty(),
        "a forged exact-hop proof is not switched"
    );
    assert!(
        !relay
            .transported_links
            .entry_for(&link_id)
            .unwrap()
            .validated_by_proof,
        "a forged proof cannot unlock later opaque Link traffic",
    );
    assert_eq!(
        relay
            .routing_table
            .path_row(&personal_node_destination())
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unknown,
        "forged traffic is not route evidence",
    );
    let (returned, _, _, _) = ingest_via(
        &mut relay,
        &proofs[0],
        iface_to_b,
        1_300,
        0x30,
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(returned.len(), 1, "the relay returns the validated proof");
    assert_eq!(returned[0].0, iface_to_a);
    assert!(
        relay
            .transported_links
            .entry_for(&link_id)
            .unwrap()
            .validated_by_proof,
        "the proof validated the transported row",
    );
    assert_eq!(
        relay
            .routing_table
            .existing_route_for(
                &personal_node_destination(),
                AttachedInterfaces::new(&relay_view)
            )
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Responsive,
        "validating the transported proof confirms the relay's route to B",
    );
    assert_eq!(
        relay
            .routing_table
            .path_row(&personal_node_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(1_300),
        "the signed establishment proof is the transported route observation",
    );

    let (rtts, _, _) = reactions_of(&mut initiator, &returned[0].1, 1_400, 0xA5);
    let (switched_rtt, _, _, _) = ingest_via(
        &mut relay,
        &rtts[0],
        iface_to_a,
        1_500,
        0x40,
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched_rtt.len(), 1, "the relay switches the sealed LRRTT");
    assert_eq!(switched_rtt[0].0, iface_to_b);
    let (_, _, _) = reactions_of(&mut responder, &switched_rtt[0].1, 1_600, 0xB5);
    assert!(matches!(
        responder.links.phase_for(&link_id),
        Some(LinkPhase::Active { .. }),
    ));

    let data = commanded_link_data(&mut initiator, link_id, b"across the mesh", 2_000, 0xD1);
    let (switched_data, _, _, _) = ingest_via(
        &mut relay,
        &data,
        iface_to_a,
        2_100,
        0x50,
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched_data.len(), 1);
    let (proof_answers, delivered, _, _) = ingest_via(
        &mut responder,
        &switched_data[0].1,
        arrival(),
        2_200,
        0x60,
        AttachedInterfaces::new(&arrival_interfaces()),
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
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched_proof.len(), 1);
    assert_eq!(switched_proof[0].0, iface_to_a);
    let proof_packet_hash = PacketHash::of_wire_packet(&switched_proof[0].1).unwrap();
    let (_, _, settled, _) = ingest_via(
        &mut initiator,
        &switched_proof[0].1,
        arrival(),
        2_400,
        0x62,
        AttachedInterfaces::new(&arrival_interfaces()),
    );
    assert_eq!(
        settled,
        std::vec![(
            CommandId(9),
            Settlement::SendToLink(Ok(crate::engine::PacketReceiptDelivered {
                rtt: RttMillis::new(400),
                evidence: crate::engine::DeliveryEvidence::Proof(
                    crate::engine::DeliveryProof::Explicit(proof_packet_hash),
                ),
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
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched_keepalive.len(), 1);
    assert_eq!(switched_keepalive[0].0, iface_to_b);
    let (echoes, _, _, _) = ingest_via(
        &mut responder,
        &switched_keepalive[0].1,
        arrival(),
        2_600,
        0x64,
        AttachedInterfaces::new(&arrival_interfaces()),
    );
    assert!(
        echoes.is_empty(),
        "a responder with recent outbound traffic suppresses the keepalive echo",
    );

    let (keepalive_again, _, _, _) = ingest_via(
        &mut relay,
        &keepalive[..n],
        iface_to_a,
        2_800,
        0x66,
        AttachedInterfaces::new(&relay_view),
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
            AttachedInterfaces::new(&relay_view),
        );
        assert_eq!(
            switched_part.len(),
            1,
            "a byte-identical resource part switches on send and on resend",
        );
    }
    let (data_replay, _, _, _) = ingest_via(
        &mut relay,
        &data,
        iface_to_a,
        2_900,
        0x68,
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(
        data_replay.len(),
        1,
        "a byte-identical retry switches through again: RNS 1.4.2 never remembers a transported link's packets in the duplicate filter",
    );
    assert_eq!(
        relay
            .routing_table
            .path_row(&personal_node_destination())
            .unwrap()
            .last_route_activity_at,
        InstantMillis(1_300),
        "later opaque switched traffic cannot repeatedly extend the attributed route",
    );
    let mut close_frames = std::vec::Vec::new();
    let _ = initiator.ingest_command_into(
        IssuedCommand {
            id: CommandId(10),
            command: PrnsCommand::CloseLink(crate::engine::CloseLink { link_id }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        AttachedInterfaces::new(&relay_view),
    );
    assert_eq!(switched_close.len(), 1);
    let (_, _, _, closed) = ingest_via(
        &mut responder,
        &switched_close[0].1,
        arrival(),
        3_000,
        0x68,
        AttachedInterfaces::new(&arrival_interfaces()),
    );
    assert_eq!(
        closed,
        std::vec![crate::engine::LinkClosedReason::PeerClosed],
        "the goodbye crossed the mesh",
    );
    assert!(
        responder.links.phase_for(&link_id).is_none(),
        "the responder's session is gone",
    );
}

#[test]
fn an_identified_request_policy_passes_packets_only_after_the_peer_identifies() {
    use crate::engine::{
        Identify, PacketReceiptDelivered, Respond, RespondData, SendRequest, SendRequestData,
        SendRequestFailure,
    };
    use crate::routing::links::data::link_data_frame_ceiling;
    use crate::routing::links::request::{
        response_data_wire_len, RequestId, REQUEST_WIRE_OVERHEAD, RESPONSE_WIRE_OVERHEAD,
    };
    use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};

    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

    let asker = initiator.held_identity_hashes()[0];
    responder
        .register_request_handler(
            &personal_node_destination(),
            "/status",
            RequestPolicy::RequireIdentified,
        )
        .unwrap();

    let command = |engine: &mut EngineState<TestStorageLayout>,
                   id: u64,
                   command: PrnsCommand,
                   now: u64,
                   iv_fill: u8| {
        let mut sent = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();
        let mut size_hints = std::vec::Vec::new();
        let _ = engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(id),
                command,
            },
            AttachedInterfaces::new(&arrival_interfaces()),
            InstantMillis(now),
            &mut |bytes: &mut [u8]| bytes.fill(iv_fill),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    sent.push(bytes.to_vec());
                }
                EngineReaction::Directive(Directive::EmitFrame {
                    size_hint, fill, ..
                }) => {
                    size_hints.push(size_hint);
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
        (sent, settled, size_hints)
    };
    let ask = SendRequest {
        link_id,
        path_hash: RequestPathHash::of("/status"),
        data: SendRequestData::from_slice(&[0xC4, 0x03, b'a', b's', b'k']).unwrap(),
        response_timeout: Default::default(),
        maximum_response_bytes: Default::default(),
    };

    let expected_request_hint = link_data_frame_ceiling(REQUEST_WIRE_OVERHEAD + ask.data.len());
    let (sent, settled, size_hints) = command(
        &mut initiator,
        20,
        PrnsCommand::SendRequest(ask.clone()),
        2_000,
        0xD1,
    );
    assert_eq!(sent.len(), 1);
    assert_eq!(size_hints, std::vec![expected_request_hint]);
    assert!(settled.is_empty(), "the request awaits its response");
    let mut heard = std::vec::Vec::new();
    let mut raw = sent[0].clone();
    let _ = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_100),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_100),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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

    let (identify_frames, _, size_hints) = command(
        &mut initiator,
        21,
        PrnsCommand::Identify(Identify {
            link_id,
            identity: asker,
        }),
        2_200,
        0xE1,
    );
    assert!(size_hints.is_empty());
    let mut raw = identify_frames[0].clone();
    let _ = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_300),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_300),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
    let (sent, _, size_hints) = command(
        &mut initiator,
        22,
        PrnsCommand::SendRequest(ask),
        2_400,
        0xF1,
    );
    assert_eq!(size_hints, std::vec![expected_request_hint]);
    let mut received: std::vec::Vec<(RequestId, std::vec::Vec<u8>)> = std::vec::Vec::new();
    let mut raw = sent[0].clone();
    let _ = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_500),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_500),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RequestReceived {
                    destination,
                    link_id: heard_link,
                    request_id,
                    requester,
                    path_hash,
                    data,
                    ..
                }) = reaction
                {
                    assert_eq!(destination, personal_node_destination());
                    assert_eq!(heard_link, link_id);
                    assert_eq!(requester, Some(asker));
                    assert_eq!(path_hash, RequestPathHash::of("/status"));
                    received.push((request_id, data.to_vec()));
                }
            },
        },
    );
    assert_eq!(received.len(), 1, "the identified peer's request lands");
    assert_eq!(received[0].1, &[0xC4, 0x03, b'a', b's', b'k']);
    let request_id = received[0].0;

    let response_data_len = 4;
    let (responses, settled, size_hints) = command(
        &mut responder,
        23,
        PrnsCommand::Respond(Respond {
            link_id,
            request_id,
            payload: crate::engine::RespondPayload::Packed(
                RespondData::from_slice(&[0xC4, 0x02, b'o', b'k']).unwrap(),
            ),
        }),
        2_600,
        0xA9,
    );
    assert_eq!(
        size_hints,
        std::vec![link_data_frame_ceiling(
            RESPONSE_WIRE_OVERHEAD + response_data_wire_len(response_data_len),
        )],
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_700),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
                rtt: RttMillis::new(300),
                evidence: crate::engine::DeliveryEvidence::Response,
            })),
        )],
        "the response settles the request with the measured round trip",
    );

    let mut expired = std::vec::Vec::new();
    let mut collect = |reaction: EngineReaction<'_>| {
        if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) = reaction {
            expired.push((id, settlement));
        }
    };
    let _ = initiator.settle_timed_out_receipts(InstantMillis(15_749), &mut collect);
    let _ = initiator.settle_timed_out_receipts(InstantMillis(15_750), &mut collect);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let link_id = dispatch.link_id;

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
    let (rtts, _, _) = reactions_of(&mut initiator, &proofs[0], 1_250, 0xA5);
    let (_, _, _) = reactions_of(&mut responder, &rtts[0], 1_600, 0xB5);

    let revealed = initiator.held_identity_hashes()[0];
    let mut sent = std::vec::Vec::new();
    let mut settled = std::vec::Vec::new();
    let _ = initiator.ingest_command_into(
        IssuedCommand {
            id: CommandId(11),
            command: PrnsCommand::Identify(Identify {
                link_id,
                identity: revealed,
            }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_100),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::PeerIdentified { link_id, identity }) =
                    reaction
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_200),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_300),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
            command: PrnsCommand::Identify(Identify {
                link_id,
                identity: responder.held_identity_hashes()[0],
            }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        command: PrnsCommand::SendToLink(SendToLink {
            link_id,
            payload: SendToLinkPayload::from_slice(b"too early").unwrap(),
        }),
    };

    assert_eq!(
        initiator.ingest_command(
            send(LinkId::new([0x77; 16])),
            AttachedInterfaces::new(&arrival_interfaces())
        ),
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    assert_eq!(
        initiator.ingest_command(
            send(dispatch.link_id),
            AttachedInterfaces::new(&arrival_interfaces())
        ),
        CommandOutcome::SendToLinkRejected {
            id: CommandId(9),
            rejection: SendToLinkRejection::LinkNotActive,
        },
    );
}

fn established_pair() -> (
    EngineState<TestStorageLayout>,
    EngineState<TestStorageLayout>,
    LinkId,
) {
    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    let mut responder = personal_node_announcer();
    let opened_gate = ResourceStrategy::Accept {
        max_uncompressed_bytes: 1 << 20,
        accept_compressed: false,
    };
    assert!(responder.set_default_resource_strategy(&personal_node_destination(), opened_gate));

    let (proofs, _, _) = reactions_of(&mut responder, &request[..dispatch.wire_bytes], 1_100, 0x99);
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
    state: &mut EngineState<TestStorageLayout>,
    now: u64,
) -> (
    std::vec::Vec<std::vec::Vec<u8>>,
    std::vec::Vec<(LinkId, crate::engine::LinkClosedReason)>,
) {
    let mut sent = std::vec::Vec::new();
    let mut closed = std::vec::Vec::new();
    let _ = state.fire_due_link_deadlines(
        InstantMillis(now),
        AttachedInterfaces::new(&arrival_interfaces()),
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
    use crate::engine::LinkClosedReason;
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(52_690),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xE8),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    echoes.push(bytes.to_vec());
                }
            },
        },
    );
    assert!(
        echoes.is_empty(),
        "the responder suppresses an echo while its own outbound side is still fresh",
    );

    let (sent, closed) = fire_deadlines(&mut initiator, 104_106);
    assert!(closed.is_empty());
    assert_eq!(
        sent.len(),
        1,
        "the unanswered first keepalive leaves the initiator's cadence armed",
    );
    let mut raw = sent[0].clone();
    let _ = responder.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(104_118),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(104_118),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xE8),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    echoes.push(bytes.to_vec());
                }
            },
        },
    );
    assert_eq!(
        echoes.len(),
        1,
        "the responder echoes once its outbound side has been silent for a full interval",
    );
    let (header, payload) = WirePacketHeader::parse(&echoes[0]).unwrap();
    assert_eq!(header.context, WireContext::KeepAlive);
    assert_eq!(payload, &[KEEPALIVE_ECHO]);

    let mut raw = echoes[0].clone();
    let _ = initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(104_128),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(104_128),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xE9),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |_| {},
        },
    );

    let (sent, closed) = fire_deadlines(&mut initiator, 155_534);
    assert!(closed.is_empty(), "the echo postponed staleness");
    assert_eq!(sent.len(), 1, "a second keepalive rides the new cadence");

    let (sent, closed) = fire_deadlines(&mut initiator, 104_128 + 102_856);
    assert!(
        closed.is_empty(),
        "reaching the stale boundary sends a final keepalive, not a teardown",
    );
    assert_eq!(sent.len(), 1, "the stale link pings its peer one last time");
    let (header, payload) = WirePacketHeader::parse(&sent[0]).unwrap();
    assert_eq!(header.context, WireContext::KeepAlive);
    assert_eq!(payload, &[KEEPALIVE_REQUEST]);

    let (sent, closed) = fire_deadlines(&mut initiator, 104_128 + 102_856 + 6_000);
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
    use crate::engine::LinkClosedReason;
    use crate::engine::{CloseLink, CloseLinkRejection};

    let (mut initiator, mut responder, link_id) = established_pair();

    assert_eq!(
        initiator.ingest_command(
            IssuedCommand {
                id: CommandId(11),
                command: PrnsCommand::CloseLink(CloseLink {
                    link_id: LinkId::new([0x77; 16]),
                }),
            },
            AttachedInterfaces::new(&arrival_interfaces()),
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
            command: PrnsCommand::CloseLink(CloseLink { link_id }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_100),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xEB),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_200),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xEC),
            should_prove: &mut |_: &crate::engine::ProofRequest| false,
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
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
fn a_valid_peer_close_commits_final_route_evidence_before_removal() {
    use crate::engine::CloseLink;

    let (mut initiator, mut responder, link_id) = established_pair();
    let mut closes = std::vec::Vec::new();
    let _ = responder.ingest_command_into(
        IssuedCommand {
            id: CommandId(12),
            command: PrnsCommand::CloseLink(CloseLink { link_id }),
        },
        AttachedInterfaces::new(&arrival_interfaces()),
        InstantMillis(2_000),
        &mut |bytes: &mut [u8]| bytes.fill(0xEA),
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                closes.push(bytes.to_vec());
            }
            EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                if let Some(bytes) = filled_frame(fill) {
                    closes.push(bytes);
                }
            }
            _ => {}
        },
    );
    assert_eq!(closes.len(), 1);

    let mut raw = closes.remove(0);
    let _ = initiator.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_200),
            source_interface: arrival(),
            bytes: &mut raw,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&arrival_interfaces()),
            now: InstantMillis(2_200),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |_| {},
        },
    );

    assert!(initiator.links.is_empty());
    let row = initiator
        .routing_table
        .path_row(&peer_destination())
        .unwrap();
    assert_eq!(row.last_route_activity_at, InstantMillis(2_200));
    assert_eq!(row.responsiveness, RouteResponsiveness::Responsive);
}

#[test]
fn a_narrow_interface_negotiates_the_link_mtu_down_end_to_end() {
    use crate::engine::{SendToLink, SendToLinkFailure, SendToLinkPayload};
    use crate::routing::links::data::LinkDataError;

    fn narrow_view() -> [InterfaceDescriptor; 1] {
        let mut descriptor = routable_descriptor(arrival());
        descriptor.hardware_mtu = Some(300);
        [descriptor]
    }

    let mut initiator = neighbor_with_a_route();
    let mut request = [0u8; BROADCAST_MTU];
    let dispatch = initiator
        .write_commanded_link_request(
            CommandId(7),
            &establish(),
            InstantMillis(1_000),
            vector_establish_entropy(),
            AttachedInterfaces::new(&narrow_view()),
            &mut request,
        )
        .dispatched();
    let parsed = parse_link_request(&request[..dispatch.wire_bytes]).unwrap();
    assert_eq!(
        parsed.mtu, 300,
        "the initiator signals its interface's ceiling"
    );

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of_on(
        &mut responder,
        &request[..dispatch.wire_bytes],
        1_100,
        0x99,
        AttachedInterfaces::new(&narrow_view()),
    );
    let (rtts, _, _) = reactions_of_on(
        &mut initiator,
        &proofs[0],
        1_250,
        0xA5,
        AttachedInterfaces::new(&narrow_view()),
    );
    let (_, _, _) = reactions_of_on(
        &mut responder,
        &rtts[0],
        1_600,
        0xB5,
        AttachedInterfaces::new(&narrow_view()),
    );

    for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
        let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id) else {
            panic!("the {name} must be active");
        };
        assert_eq!(*mtu, 300, "the {name} settled on the narrow mtu");
    }

    let mut settled = std::vec::Vec::new();
    let _ = initiator.ingest_command_into(
        IssuedCommand {
            id: CommandId(9),
            command: PrnsCommand::SendToLink(SendToLink {
                link_id: dispatch.link_id,
                payload: SendToLinkPayload::from_slice(&[0x42; 250]).unwrap(),
            }),
        },
        AttachedInterfaces::new(&narrow_view()),
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
            command: PrnsCommand::SendToLink(SendToLink {
                link_id: dispatch.link_id,
                payload: SendToLinkPayload::from_slice(&[0x42; 200]).unwrap(),
            }),
        },
        AttachedInterfaces::new(&narrow_view()),
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

    fn fat_view() -> [InterfaceDescriptor; 1] {
        let mut descriptor = routable_descriptor(arrival());
        descriptor.hardware_mtu = Some(1_064);
        [descriptor]
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
            AttachedInterfaces::new(&fat_view()),
            &mut request,
        )
        .dispatched();
    let parsed = parse_link_request(&request[..dispatch.wire_bytes]).unwrap();
    assert_eq!(
        parsed.mtu, negotiable,
        "a fat interface signals up to the engine ceiling, never past it",
    );

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of_on(
        &mut responder,
        &request[..dispatch.wire_bytes],
        1_100,
        0x99,
        AttachedInterfaces::new(&fat_view()),
    );
    let (rtts, _, _) = reactions_of_on(
        &mut initiator,
        &proofs[0],
        1_250,
        0xA5,
        AttachedInterfaces::new(&fat_view()),
    );
    let (_, _, _) = reactions_of_on(
        &mut responder,
        &rtts[0],
        1_600,
        0xB5,
        AttachedInterfaces::new(&fat_view()),
    );

    for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
        let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id) else {
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
    use crate::interfaces::usb_auto::{
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
            AttachedInterfaces::new(&[host]),
            &mut request,
        )
        .dispatched();
    let parsed = parse_link_request(&request[..dispatch.wire_bytes]).unwrap();
    assert_eq!(
        parsed.mtu,
        MAX_LINK_MTU.min(host.hardware_mtu.unwrap()),
        "the host side signals its declared tier up to the engine ceiling",
    );

    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of_on(
        &mut responder,
        &request[..dispatch.wire_bytes],
        1_100,
        0x99,
        AttachedInterfaces::new(&[device]),
    );
    let (rtts, _, _) = reactions_of_on(
        &mut initiator,
        &proofs[0],
        1_250,
        0xA5,
        AttachedInterfaces::new(&[host]),
    );
    let (_, _, _) = reactions_of_on(
        &mut responder,
        &rtts[0],
        1_600,
        0xB5,
        AttachedInterfaces::new(&[device]),
    );

    for (name, engine) in [("initiator", &initiator), ("responder", &responder)] {
        let Some(LinkPhase::Active { mtu, .. }) = engine.links.phase_for(&dispatch.link_id) else {
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
            AttachedInterfaces::new(&arrival_interfaces()),
            &mut buf,
        )
        .dispatched();

    let outcome = state.write_commanded_link_request(
        CommandId(8),
        &establish(),
        InstantMillis(2_000),
        vector_establish_entropy(),
        AttachedInterfaces::new(&arrival_interfaces()),
        &mut buf,
    );
    assert!(matches!(
        outcome,
        EstablishLinkWriteOutcome::Rejected {
            rejection: WriteEstablishLinkRejection::DuplicateLinkId,
        },
    ));
    assert_eq!(state.links.len(), 1, "the original establishment stands");
}
