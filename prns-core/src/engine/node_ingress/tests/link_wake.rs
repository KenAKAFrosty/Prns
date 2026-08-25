use crate::engine::test_support::{routable_descriptor, test_entropy_bytes, TestStorageLayout};
use crate::engine::{CommandId, EngineState, IngestIo};
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_ECHO};
use crate::routing::links::table::{InitiatedLink, LinkActivation, LinkPhase};
use crate::routing::links::{LinkId, LinkKey};
use crate::units::{InstantMillis, RttMillis};
use crate::wire::{DestinationHash, WireContext, BROADCAST_MTU};

fn take_route_evidence(engine: &mut EngineState<TestStorageLayout>) -> Option<InstantMillis> {
    let mut observed = None;
    engine
        .links
        .reconcile_pending_route_evidence(|_, at| observed = Some(at));
    observed
}

fn engine_with_active_link() -> (EngineState<TestStorageLayout>, LinkId, InterfaceId) {
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey, X25519SecretKey,
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
            route_evidence: crate::routing::routes::RouteEvidenceHandle::new(
                crate::routing::routes::RouteEvidenceId::FIRST,
                0,
            ),
            expected_hops: 1,
            mode: crate::routing::links::LinkMode::Aes256Cbc,
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
                received_hops: 1,
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
fn a_keepalive_echo_records_inbound_without_postponing_the_silent_outbound_arm() {
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
            should_accept_resource: &mut |_: &crate::routing::links::resources::ResourceOffer| {
                false
            },
            sink: &mut |_| {},
        },
    );

    let truth = engine.link_deadlines_wake();
    assert_eq!(
        before, truth,
        "fresh inbound cannot mask the already-earlier outbound-silence wake",
    );
    let Some(LinkPhase::Active {
        last_inbound,
        last_outbound,
        ..
    }) = engine.links.phase_for(&link_id)
    else {
        panic!("the keepalive echo must leave the link active");
    };
    assert_eq!(*last_inbound, InstantMillis(2_000));
    assert_eq!(*last_outbound, InstantMillis(1_000));
    assert_eq!(
        take_route_evidence(&mut engine),
        Some(InstantMillis(2_000)),
        "the received maintenance frame is also pending route evidence",
    );
    assert_eq!(
        wake.link_deadlines, truth,
        "the ingest delta must still report the complete recomputed schedule",
    );
}

#[test]
fn authenticated_link_data_is_route_evidence() {
    let (mut engine, link_id, lane) = engine_with_active_link();
    let Some(LinkPhase::Active { key, .. }) = engine.links.phase_for(&link_id) else {
        panic!("the fixture must hold an active Link");
    };
    let mut frame = [0u8; BROADCAST_MTU];
    let written = crate::routing::links::data::write_link_packet(
        &link_id,
        key,
        BROADCAST_MTU,
        WireContext::None,
        b"valid return traffic",
        &test_entropy_bytes::<16>(0xA5),
        &mut frame,
    )
    .unwrap();
    let _ = engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(2_000),
            source_interface: lane,
            bytes: &mut frame[..written],
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(&[routable_descriptor(lane)]),
            now: InstantMillis(2_000),
            fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
            should_prove: &mut |_| false,
            should_accept_resource: &mut |_| false,
            sink: &mut |_| {},
        },
    );
    assert_eq!(take_route_evidence(&mut engine), Some(InstantMillis(2_000)));
}
