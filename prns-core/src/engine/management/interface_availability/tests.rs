use super::*;
use crate::crypto::{
    Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey,
};
use crate::engine::test_support::{routable_descriptor, TestStorageLayout};
use crate::engine::{CommandId, Journaled, SendRequestFailure, WakeSchedule};
use crate::identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey};
use crate::routing::announce::{Announce, AnnounceId, DottedNameHash};
use crate::routing::links::resources::receive::tests_support::{
    engine_with_active_link, lane, link_id, track_pending_request,
};
use crate::routing::links::table::{InitiatedLink, RespondingLink};
use crate::routing::links::{LinkId, LinkKey, LinkMode};
use crate::routing::reverse_routes::ReverseRouteEntry;
use crate::routing::routes::{RouteEvidenceHandle, RouteEvidenceId};
use crate::routing::tunnel::TunnelId;
use crate::routing::warmth::RouteWarmth;
use crate::routing::{AnnounceArrival, NextHop};
use crate::wire::DestinationHash;

const OFF: InterfaceId = InterfaceId::new([0xAA; 8]);
const KEEP: InterfaceId = InterfaceId::new([0xBB; 8]);
fn dest(byte: u8) -> DestinationHash {
    DestinationHash::new([byte; 16])
}

fn add_route(
    engine: &mut EngineState<TestStorageLayout>,
    destination: DestinationHash,
    source: InterfaceId,
) {
    let announce = Announce {
        destination,
        public_keys: IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0x31; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0x41; 32])),
        },
        dotted_name_hash: DottedNameHash::new([0x51; 10]),
        announce_id: AnnounceId::from_wire([destination.as_bytes()[0]; 10]),
        ratchet: None,
        signature: Ed25519Signature([0x61; 64]),
        app_data: b"retained",
    };
    let evidence = engine.route_evidence_id_for_update(&destination, source, NextHop::Direct);
    let descriptors = [routable_descriptor(OFF), routable_descriptor(KEEP)];
    let _ = engine.routing_table.upsert_route(
        &AnnounceArrival {
            announce,
            hops: 1,
            arrived_at: InstantMillis(100),
            receiving_interface: source,
            next_hop: NextHop::Direct,
            is_path_response: false,
        },
        evidence,
        AttachedInterfaces::new(&descriptors),
        &mut |_| {},
    );
}

#[test]
fn administrative_off_bypasses_tunnel_warmth_and_does_not_resurrect_on_enable() {
    let mut engine = EngineState::<TestStorageLayout>::default();
    add_route(&mut engine, dest(1), OFF);
    add_route(&mut engine, dest(2), KEEP);
    engine
        .tunnels
        .observe_synthesize(TunnelId::new([1; 32]), OFF, InstantMillis(500_000));
    assert_eq!(engine.tunnels.warm_until(OFF), Some(InstantMillis(500_000)));
    let _ = engine
        .scheduled_announces
        .schedule(dest(1), InstantMillis(5_000), OFF, 1);
    let _ = engine
        .scheduled_announces
        .schedule(dest(2), InstantMillis(6_000), KEEP, 1);
    engine.take_dirty_interfaces().drain(|_| {});
    let descriptors = [routable_descriptor(OFF), routable_descriptor(KEEP)];
    // The descriptor roster deliberately still contains the administrative lane.
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut removed = std::vec::Vec::new();
    let wake = engine.interface_unavailable(OFF, InstantMillis(200), interfaces, &mut |reaction| {
        match reaction {
            EngineReaction::Journaled(Journaled::RouteRemoved { destination, .. }) => {
                removed.push(destination)
            }
            _ => panic!("route-only retirement must not emit wire work"),
        }
    });
    assert_eq!(removed, [dest(1)]);
    assert!(engine.routing_table.stored_announce_for(&dest(1)).is_none());
    assert!(engine
        .routing_table
        .existing_route_for(&dest(1), interfaces)
        .is_none());
    assert_eq!(
        engine
            .routing_table
            .forwarding_route_for(&dest(2))
            .unwrap()
            .receiving_interface,
        KEEP
    );
    assert_eq!(wake, engine.wake_schedules(interfaces));
    assert_eq!(engine.scheduled_announce_count(), 1);
    assert!(engine.take_dirty_interfaces().contains(&OFF));
    engine.interface_attached(OFF, InstantMillis(201));
    assert!(engine
        .routing_table
        .forwarding_route_for(&dest(1))
        .is_none());
    let mut repeated_events = 0;
    engine.interface_unavailable(OFF, InstantMillis(202), interfaces, &mut |_| {
        repeated_events += 1
    });
    assert_eq!(repeated_events, 0);
    // Explicit new evidence, including a cached announce with the same announce ID, may return.
    add_route(&mut engine, dest(1), KEEP);
    assert_eq!(
        engine
            .routing_table
            .forwarding_route_for(&dest(1))
            .unwrap()
            .receiving_interface,
        KEEP
    );
}

#[test]
fn administrative_off_closes_all_endpoint_phases_and_settles_each_waiter_once() {
    let mut engine = engine_with_active_link();
    let off = lane();
    let active = link_id();
    track_pending_request(&mut engine, CommandId(35), 2_100, 90_000);
    let pending = LinkId::new([0x31; 16]);
    engine
        .links
        .track_initiated(InitiatedLink {
            attached_interface: off,
            link_id: pending,
            destination: dest(4),
            route_evidence: RouteEvidenceHandle::new(RouteEvidenceId::FIRST, 0),
            expected_hops: 1,
            mode: LinkMode::Aes256Cbc,
            initiator_secret: X25519SecretKey::new([0x41; 32]),
            link_signing: Ed25519SecretKey::new([0x42; 32]),
            requested_at: InstantMillis(1_000),
            timeout_at: InstantMillis(90_000),
            command_id: CommandId(36),
        })
        .unwrap();
    let handshake = LinkId::new([0x32; 16]);
    let unrelated = LinkId::new([0x33; 16]);
    for (id, interface) in [(handshake, off), (unrelated, KEEP)] {
        let shared = crate::crypto::x25519_diffie_hellman(
            &X25519SecretKey::new([0x43; 32]),
            &X25519PublicKey([0x44; 32]),
        );
        engine
            .links
            .track_responding(RespondingLink {
                attached_interface: interface,
                link_id: id,
                key: LinkKey::derive(&id, &shared),
                requested_at: InstantMillis(1_000),
                timeout_at: InstantMillis(90_000),
                mtu: 500,
                initiator_signing: Ed25519PublicKey([0x45; 32]),
                destination: dest(5),
                identity: crate::identity::IdentityHash::new([0x46; 16]),
                proof_strategy: crate::routing::upstream_app_destinations::ProofStrategy::ProveAll,
            })
            .unwrap();
    }
    let descriptors = [routable_descriptor(off), routable_descriptor(KEEP)];
    let mut settlements = std::vec::Vec::new();
    let mut closed = std::vec::Vec::new();
    let wake = engine.interface_unavailable(
        off,
        InstantMillis(2_200),
        AttachedInterfaces::new(&descriptors),
        &mut |reaction| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                settlements.push((id, settlement))
            }
            EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                assert_eq!(reason, LinkClosedReason::LocallyClosed);
                closed.push(link_id);
            }
            _ => panic!("no frame is sent over an administratively disabled lane"),
        },
    );
    assert_eq!(settlements.len(), 2);
    assert!(settlements.contains(&(
        CommandId(35),
        Settlement::SendRequest(Err(SendRequestFailure::LinkClosed))
    )));
    assert!(settlements.contains(&(
        CommandId(36),
        Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination
        )))
    )));
    assert_eq!(closed.len(), 3);
    for id in [active, pending, handshake] {
        assert!(closed.contains(&id));
        assert!(!engine.links.has_local_link(&id));
    }
    assert!(engine.links.has_local_link(&unrelated));
    assert_eq!(wake.receipt_timeouts, WakeSchedule::Idle);
    assert_eq!(
        wake,
        engine.wake_schedules(AttachedInterfaces::new(&descriptors))
    );
    engine.interface_unavailable(
        off,
        InstantMillis(2_201),
        AttachedInterfaces::new(&descriptors),
        &mut |_| panic!("second retirement must not resettle"),
    );
}

#[test]
fn administrative_off_retires_transit_and_recursive_work_in_both_directions() {
    exercise_transit_retirement(OFF, OFF);
    use crate::interfaces::InterfaceKind;
    exercise_transit_retirement(
        InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"controller"),
        InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"absent"),
    );
}

fn exercise_transit_retirement(scope: InterfaceId, off: InterfaceId) {
    let mut engine = EngineState::<TestStorageLayout>::default();
    for (byte, received, outbound) in [(1, off, KEEP), (2, KEEP, off), (3, KEEP, KEEP)] {
        engine.reverse_routes.remember(
            ReverseRouteEntry {
                proof_destination: dest(byte),
                received_interface: received,
                outbound_interface: outbound,
                expires_at: InstantMillis(50_000),
            },
            InstantMillis(1),
        );
        engine
            .transported_links
            .track(crate::routing::links::transported::TransportedLink {
                link_id: LinkId::new([byte; 16]),
                destination: dest(byte),
                route_evidence: RouteEvidenceHandle::new(RouteEvidenceId::FIRST, 0),
                mode: LinkMode::Aes256Cbc,
                next_hop: None,
                next_hop_interface: outbound,
                received_interface: received,
                taken_hops: 1,
                remaining_hops: 1,
                validated_by_proof: true,
                last_active: InstantMillis(1),
                proof_timeout: InstantMillis(50_000),
            })
            .unwrap();
    }
    engine
        .recursive_path_requests
        .begin(dest(1), off, InstantMillis(50_000));
    engine
        .recursive_path_requests
        .begin(dest(2), KEEP, InstantMillis(50_000));
    let descriptors = [routable_descriptor(OFF), routable_descriptor(KEEP)];
    engine.interface_unavailable(
        scope,
        InstantMillis(2),
        AttachedInterfaces::new(&descriptors),
        &mut |_| panic!("transit retirement emits no outbound wire"),
    );
    assert_eq!(engine.reverse_routes.len(), 1);
    assert_eq!(engine.transported_links.validated_count(), 1);
    assert_eq!(
        engine.recursive_path_requests.take_requester(&dest(1)),
        None
    );
    assert_eq!(
        engine.recursive_path_requests.take_requester(&dest(2)),
        Some(KEEP)
    );
    let dirty = engine.take_dirty_interfaces();
    assert!(dirty.contains(&off));
    assert!(dirty.contains(&KEEP));
}

#[test]
fn administrative_off_retires_absent_supervisor_children_without_a_descriptor() {
    use crate::interfaces::InterfaceKind;
    use crate::routing::warmth::Departure;
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"controller");
    let absent = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"persisted peer");
    let unrelated = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"other family");
    let mut engine = EngineState::<TestStorageLayout>::default();
    add_route(&mut engine, dest(1), absent);
    add_route(&mut engine, dest(2), unrelated);
    let stored = engine.routing_table.stored_announce_for(&dest(1)).unwrap();
    engine
        .held_announces
        .hold(1, absent, NextHop::Direct, false, &stored.announce);
    let stored = engine.routing_table.stored_announce_for(&dest(2)).unwrap();
    engine
        .held_announces
        .hold(1, unrelated, NextHop::Direct, false, &stored.announce);
    let pending = LinkId::new([0x71; 16]);
    engine
        .links
        .track_initiated(InitiatedLink {
            attached_interface: absent,
            link_id: pending,
            destination: dest(1),
            route_evidence: RouteEvidenceHandle::new(RouteEvidenceId::FIRST, 0),
            expected_hops: 1,
            mode: LinkMode::Aes256Cbc,
            initiator_secret: X25519SecretKey::new([0x41; 32]),
            link_signing: Ed25519SecretKey::new([0x42; 32]),
            requested_at: InstantMillis(1),
            timeout_at: InstantMillis(90_000),
            command_id: CommandId(70),
        })
        .unwrap();
    engine.interface_departed(absent, Departure::MayReturn, InstantMillis(1));
    assert!(engine.departed_interfaces.warm_until(absent).is_some());
    engine
        .recursive_path_requests
        .begin(dest(1), absent, InstantMillis(50_000));
    engine
        .recursive_path_requests
        .begin(dest(2), unrelated, InstantMillis(50_000));
    let _ = engine
        .scheduled_announces
        .schedule(dest(1), InstantMillis(500), absent, 1);
    let _ = engine
        .scheduled_announces
        .schedule(dest(2), InstantMillis(600), unrelated, 1);
    // Neither child's descriptor is currently attached; old state is independent of the roster.
    let descriptors = [routable_descriptor(supervisor)];
    let mut removed = std::vec::Vec::new();
    engine.interface_unavailable(
        supervisor,
        InstantMillis(2),
        AttachedInterfaces::new(&descriptors),
        &mut |event| {
            if let EngineReaction::Journaled(Journaled::RouteRemoved { destination, .. }) = event {
                removed.push(destination);
            }
        },
    );
    assert_eq!(removed, [dest(1)]);
    assert!(!engine.links.has_local_link(&pending));
    assert_eq!(engine.held_announces.len_for(absent), 0);
    assert_eq!(engine.held_announces.len_for(unrelated), 1);
    assert!(engine
        .routing_table
        .forwarding_route_for(&dest(1))
        .is_none());
    assert!(engine
        .routing_table
        .forwarding_route_for(&dest(2))
        .is_some());
    assert_eq!(engine.departed_interfaces.warm_until(absent), None);
    assert_eq!(
        engine.recursive_path_requests.take_requester(&dest(1)),
        None
    );
    assert_eq!(
        engine.recursive_path_requests.take_requester(&dest(2)),
        Some(unrelated)
    );
    assert_eq!(engine.scheduled_announce_count(), 1);
    assert!(engine.take_dirty_interfaces().contains(&absent));
    engine.interface_attached(absent, InstantMillis(3));
    assert!(engine.routing_table.stored_announce_for(&dest(1)).is_none());
}
