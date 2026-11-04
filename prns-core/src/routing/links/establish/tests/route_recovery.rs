use super::*;
use crate::engine::RouteRemovalCause;
use crate::routing::announce::{
    write_path_response_announce_wire_packet, write_relayed_path_response_wire_packet, Announce,
};
use crate::routing::path_requests::PATH_REQUEST_DESTINATION;
use crate::storage::DirtyInterfaceSet;

struct TimeoutCapture {
    sent: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
    removed: std::vec::Vec<(DestinationHash, RouteRemovalCause)>,
    settled: std::vec::Vec<(CommandId, Settlement)>,
    wakes: WakeSchedules,
}

fn endpoint_with_a_route() -> EngineState<TestStorageLayout> {
    let mut state = neighbor_with_a_route();
    let identity = state.held_identity_hashes()[0];
    state.set_non_routing_identity(&identity).unwrap();
    state
}

fn expire(state: &mut EngineState<TestStorageLayout>, now: u64) -> TimeoutCapture {
    let mut capture = TimeoutCapture {
        sent: std::vec::Vec::new(),
        removed: std::vec::Vec::new(),
        settled: std::vec::Vec::new(),
        wakes: WakeSchedules::UNCHANGED,
    };
    capture.wakes = state.fire_due_link_deadlines(
        InstantMillis(now),
        AttachedInterfaces::new(&both_arrival_interfaces()),
        &mut |bytes: &mut [u8]| bytes.fill(0xA5),
        &mut |reaction| match reaction {
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                capture.sent.push((target, bytes.to_vec()));
            }
            EngineReaction::Journaled(Journaled::RouteRemoved {
                destination, cause, ..
            }) => capture.removed.push((destination, cause)),
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                capture.settled.push((id, settlement));
            }
            _ => {}
        },
    );
    capture
}

fn start(state: &mut EngineState<TestStorageLayout>, id: u64, now: u64) -> LinkRequestDispatch {
    let mut wire = [0; BROADCAST_MTU];
    state
        .write_commanded_link_request(
            CommandId(id),
            &establish(),
            InstantMillis(now),
            EstablishLinkEntropy::new([id as u8; EstablishLinkEntropy::LEN]),
            AttachedInterfaces::new(&both_arrival_interfaces()),
            &mut wire,
        )
        .dispatched()
}

fn announce(now: u64) -> std::vec::Vec<u8> {
    let mut announcer = personal_node_announcer();
    let mut wire = [0; BROADCAST_MTU];
    let length = announcer
        .write_commanded_announce(
            &AnnounceNow {
                destination: personal_node_destination(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            },
            InstantMillis(now),
            &mut test_fill_entropy,
            &mut wire,
        )
        .written_len();
    wire[..length].to_vec()
}

fn learn(state: &mut EngineState<TestStorageLayout>, wire: &[u8], source: InterfaceId, now: u64) {
    let mut wire = wire.to_vec();
    assert!(matches!(
        state.ingest_for_test(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: source,
                bytes: &mut wire,
            },
            AttachedInterfaces::new(&both_arrival_interfaces()),
        ),
        IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_))
    ));
}

fn assert_one_discovery(capture: &TimeoutCapture) {
    assert_eq!(
        capture.sent.len(),
        2,
        "one request fans out to two interfaces"
    );
    assert_eq!(capture.sent[0].0, arrival());
    assert_eq!(capture.sent[1].0, other_arrival());
    assert_eq!(capture.sent[0].1, capture.sent[1].1);
    let (header, payload) = WirePacketHeader::parse(&capture.sent[0].1).unwrap();
    assert_eq!(header.address, PATH_REQUEST_DESTINATION.to_address());
    assert_eq!(&payload[..16], peer_destination().as_bytes());
    assert_eq!(
        payload.len(),
        32,
        "endpoint discovery carries no transport identity"
    );
}

#[test]
fn endpoint_timeout_retires_the_failed_route_and_publishes_recovery_effects() {
    let mut state = endpoint_with_a_route();
    start(&mut state, 7, 1_000);
    let _ = state.scheduled_announces.schedule_directed(
        peer_destination(),
        InstantMillis(20_000),
        other_arrival(),
        1,
    );
    state.take_dirty_interfaces().drain(|_| {});

    let capture = expire(&mut state, 13_001);
    assert_eq!(
        capture.removed,
        [(peer_destination(), RouteRemovalCause::Expired)]
    );
    assert!(!state.routing_table.has_route(&peer_destination()));
    assert_eq!(state.scheduled_announce_count(), 0);
    assert_eq!(capture.wakes.expired_routes, WakeSchedule::Idle);
    assert_eq!(capture.wakes.scheduled_announces, WakeSchedule::Idle);
    assert_ne!(
        capture.wakes.expired_destination_identities,
        WakeSchedule::Unchanged
    );
    assert_eq!(
        capture.settled,
        [(
            CommandId(7),
            Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout))
        )]
    );
    let mut dirty = std::vec::Vec::new();
    state
        .take_dirty_interfaces()
        .drain(|interface| dirty.push(interface));
    assert_eq!(dirty, [arrival()]);
    assert_one_discovery(&capture);
}

#[test]
fn endpoint_rediscovers_the_same_cached_announce_on_a_new_path_and_links() {
    let original = announce(100);
    let (header, payload) = WirePacketHeader::parse(&original).unwrap();
    let parsed = Announce::from_wire(&header, payload).unwrap();
    let mut relayed = [0; BROADCAST_MTU];
    let relayed_len =
        write_relayed_path_response_wire_packet(&parsed, 1, RESPONDER_TRANSPORT_ID, &mut relayed)
            .unwrap();
    let mut state = EngineState::new(second_secret_key());
    let identity = state.held_identity_hashes()[0];
    state.set_non_routing_identity(&identity).unwrap();
    learn(&mut state, &relayed[..relayed_len], arrival(), 500);
    start(&mut state, 7, 1_000);
    let capture = expire(&mut state, 20_000);
    assert_one_discovery(&capture);

    let mut reply = [0; BROADCAST_MTU];
    let reply_len = write_path_response_announce_wire_packet(&parsed, 0, &mut reply).unwrap();
    learn(&mut state, &reply[..reply_len], other_arrival(), 20_100);
    let recovered = state.routing_table.path_row(&peer_destination()).unwrap();
    assert_eq!(recovered.receiving_interface, other_arrival());
    assert_eq!(recovered.hops, 1);

    let mut request = [0; BROADCAST_MTU];
    let dispatch = state
        .write_commanded_link_request(
            CommandId(8),
            &establish(),
            InstantMillis(20_200),
            vector_establish_entropy(),
            AttachedInterfaces::new(&both_arrival_interfaces()),
            &mut request,
        )
        .dispatched();
    assert_eq!(dispatch.fire_on, other_arrival());
    let mut responder = personal_node_announcer();
    let (proofs, _, _) = reactions_of(
        &mut responder,
        &request[..dispatch.wire_bytes],
        20_300,
        0x99,
    );
    // The initiator may observe the proof on a different local interface from the responder.
    let mut rtts = std::vec::Vec::new();
    let mut settled = std::vec::Vec::new();
    let mut proof = proofs[0].clone();
    crate::engine::drive_packet_to_quiescence(
        &mut state,
        InboundPacket {
            arrived_at: InstantMillis(20_450),
            source_interface: other_arrival(),
            bytes: &mut proof,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&both_arrival_interfaces()),
            now: InstantMillis(20_450),
            fill_random: &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    assert_eq!(target, other_arrival());
                    rtts.push(bytes.to_vec());
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement))
                }
                _ => {}
            },
        },
    );
    assert_eq!(
        settled,
        [(
            CommandId(8),
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: dispatch.link_id,
                rtt_millis: 250
            }))
        )]
    );
    assert_eq!(rtts.len(), 1);
    let _ = reactions_of(&mut responder, &rtts[0], 20_500, 0xB5);
    assert!(matches!(
        responder.links.phase_for(&dispatch.link_id),
        Some(LinkPhase::Active { .. })
    ));
}

#[test]
fn endpoint_timeout_preserves_newer_announces_and_replacement_paths() {
    for source in [arrival(), other_arrival()] {
        let mut state = endpoint_with_a_route();
        start(&mut state, 7, 1_000);
        learn(&mut state, &announce(2_000), source, 2_000);
        let newer = state.routing_table.path_row(&peer_destination()).unwrap();
        let capture = expire(&mut state, 13_001);
        assert_eq!(
            state.routing_table.path_row(&peer_destination()),
            Some(newer)
        );
        assert!(capture.removed.is_empty());
        assert!(
            capture.sent.is_empty(),
            "newer evidence needs no forced discovery"
        );
        assert_eq!(capture.settled.len(), 1);
    }
}

#[test]
fn endpoint_timeout_preserves_authenticated_activity_from_a_parallel_link() {
    let mut state = endpoint_with_a_route();
    let mut request = [0; BROADCAST_MTU];
    let active = state
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
    let (proofs, _, _) = reactions_of(&mut responder, &request[..active.wire_bytes], 1_100, 0x99);
    let _ = reactions_of(&mut state, &proofs[0], 1_250, 0xA5);
    start(&mut state, 8, 2_000);
    state
        .links
        .note_inbound(&active.link_id, InstantMillis(3_000));

    let capture = expire(&mut state, 14_001);
    let route = state.routing_table.path_row(&peer_destination()).unwrap();
    assert_eq!(route.last_route_activity_at, InstantMillis(3_000));
    assert_eq!(route.responsiveness, RouteResponsiveness::Responsive);
    assert!(capture.removed.is_empty());
    assert!(capture.sent.is_empty());
    assert_eq!(
        capture.settled,
        [(
            CommandId(8),
            Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout))
        )]
    );
}

#[test]
fn transport_timeout_keeps_the_route_unresponsive_without_endpoint_rediscovery() {
    let mut state = neighbor_with_a_route();
    assert!(state.network_transport_enabled());
    start(&mut state, 7, 1_000);
    let capture = expire(&mut state, 13_001);
    assert_eq!(
        state
            .routing_table
            .path_row(&peer_destination())
            .unwrap()
            .responsiveness,
        RouteResponsiveness::Unresponsive
    );
    assert!(capture.removed.is_empty());
    assert!(capture.sent.is_empty());
    assert_eq!(capture.wakes.expired_routes, WakeSchedule::Unchanged);
    assert_eq!(capture.wakes.scheduled_announces, WakeSchedule::Unchanged);
}

#[test]
fn endpoint_recovery_coalesces_parallel_failures_and_throttles_rediscovery() {
    let mut state = endpoint_with_a_route();
    start(&mut state, 7, 1_000);
    start(&mut state, 8, 1_000);
    let first = expire(&mut state, 13_001);
    assert_eq!(first.removed.len(), 1);
    assert_eq!(first.settled.len(), 2);
    assert_one_discovery(&first);

    learn(&mut state, &announce(17_000), arrival(), 17_000);
    start(&mut state, 9, 18_000);
    let throttled = expire(&mut state, 30_001);
    assert_eq!(
        throttled.removed.len(),
        1,
        "discovery throttling must not pin a failed route"
    );
    assert!(throttled.sent.is_empty());
    assert!(!state.routing_table.has_route(&peer_destination()));

    learn(&mut state, &announce(33_000), arrival(), 33_000);
    start(&mut state, 10, 34_000);
    let later = expire(&mut state, 46_001);
    assert_eq!(later.removed.len(), 1);
    assert_one_discovery(&later);
}
