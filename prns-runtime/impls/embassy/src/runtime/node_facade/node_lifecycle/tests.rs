use super::super::{CompletionPool, Fleet, ManifoldLaneSet, StaticManifoldLane};
use super::*;
use crate::engine::test_support::{
    bytes_from_hex, fixed_secret_key, personal_node_destination, sealed_single_packet,
    RNS_1_4_2_ANNOUNCE,
};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::manifold::driver::EmbassyHost;
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::routing::links::resources::ResourceStrategy;
use crate::runtime::{Diagnostic, NoPersistence, ServeMyRequestEndpoints};
use crate::storage::GrowableHeap;
use crate::wire::{PacketType, WirePacketHeader};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use std::cell::RefCell;
use std::rc::Rc;

type Mtx = CriticalSectionRawMutex;
const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;

fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: crate::interfaces::InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: crate::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

fn leak<T>(value: T) -> &'static T {
    std::boxed::Box::leak(std::boxed::Box::new(value))
}

#[test]
fn a_recipe_node_hears_an_ifac_announce_a_supervisor_stands_a_peer_up_for() {
    use crate::interfaces::{IfacContext, IfacSize};

    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

    static LANE: StaticManifoldLane<Mtx, FRAME, 4> = StaticManifoldLane::new();
    let mut lanes: ManifoldLaneSet<Mtx, 1, 4> = ManifoldLaneSet::new();
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"test-supervisor");
    let network = IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
    let supervisor_lane = lanes
        .claim_supervisor_with_ifac(&LANE, supervisor, network.clone(), leak(Signal::new()))
        .unwrap();

    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let manifold_wiring = lanes.into_manifold_wiring(
        notify.receiver(),
        commands.receiver(),
        lifecycle.receiver(),
        handle,
    );
    let fleet: Fleet<Mtx, FRAME, 4, 4> =
        supervisor_lane.into_fleet(notify.sender(), lifecycle.sender());

    let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let heard_sink = heard.clone();
    let recipe = PrnsNodeRecipe {
        transport_identity: Some(Zeroizing::new([0xC3; IDENTITY_SECRET_KEY_LEN])),
        pre_configured_destinations: [PreConfiguredDestination::Plain {
            app_name: "lxmf",
            aspects: &["delivery"],
        }],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: crate::runtime::ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event: PrnsEvent<'_>, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { .. }) = event {
                *heard_sink.borrow_mut() += 1;
            }
        },
    };

    let node: PrnsNode<_, _, _, _, _, _, 1, 1, 4, 4, 4, 4> = PrnsNode::new(
        recipe,
        manifold_wiring,
        EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
    );

    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    let mut masked = [0u8; FRAME];
    let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();
    let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"test-peer-medium");

    let drive = async move {
        let mut fleet = fleet;
        fleet.register_member(descriptor(peer)).await;
        Timer::after(Duration::from_millis(40)).await;

        fleet
            .try_deliver_inbound(peer, &masked[..masked_len])
            .expect("the shared lane carries the peer's frame");
        Timer::after(Duration::from_millis(80)).await;

        fleet.deregister_member(peer).await;
        Timer::after(Duration::from_millis(20)).await;
    };

    let _ = block_on(with_timeout(Duration::from_millis(600), node.run(drive)));
    assert_eq!(
        *heard.borrow(),
        1,
        "the node heard the announce the supervisor's peer carried in"
    );
}

#[test]
fn run_with_proof_decider_reaches_a_prove_if_recipe_destination() {
    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

    static LANE: StaticManifoldLane<Mtx, FRAME, 4> = StaticManifoldLane::new();
    let mut lanes: ManifoldLaneSet<Mtx, 1, 4> = ManifoldLaneSet::new();
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"proof-supervisor");
    let supervisor_lane = lanes
        .claim_supervisor(&LANE, supervisor, leak(Signal::new()))
        .unwrap();
    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let manifold_wiring = lanes.into_manifold_wiring(
        notify.receiver(),
        commands.receiver(),
        lifecycle.receiver(),
        handle,
    );
    let mut fleet: Fleet<Mtx, FRAME, 4, 4> =
        supervisor_lane.into_fleet(notify.sender(), lifecycle.sender());

    let recipe = PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: "personal",
            aspects: &["node"],
            identity: fixed_secret_key(),
            announce_app_data: &[],
            proof: crate::routing::ProofStrategy::ProveIf,
            link_requests: crate::routing::LinkRequestPolicy::AcceptAll,
            ratchet: crate::engine::RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        }],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: crate::runtime::ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event: PrnsEvent<'_>, _state: &()| {},
    };
    let node: PrnsNode<_, _, _, _, _, _, 1, 1, 4, 4, 4, 4> = PrnsNode::new(
        recipe,
        manifold_wiring,
        EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
    );

    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
    let raw = sealed_single_packet(
        &identity,
        personal_node_destination(),
        b"embedded facade decision",
    );
    let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"proof-peer");
    let proof = Rc::new(RefCell::new(None));
    let proof_sink = proof.clone();
    let drive = async move {
        fleet.register_member(descriptor(peer)).await;
        Timer::after(Duration::from_millis(40)).await;
        fleet
            .try_deliver_inbound(peer, &raw)
            .expect("the peer frame enters the shared lane");
        if let Ok(frame) = with_timeout(Duration::from_millis(300), fleet.next_outbound()).await {
            *proof_sink.borrow_mut() = Some(frame.bytes().to_vec());
        }
    };

    let decisions = Rc::new(RefCell::new(Vec::new()));
    let decision_sink = decisions.clone();
    let _ = block_on(with_timeout(
        Duration::from_millis(600),
        node.run_with_proof_decider(
            move |request| {
                decision_sink
                    .borrow_mut()
                    .extend_from_slice(request.plaintext);
                true
            },
            drive,
        ),
    ));

    assert_eq!(*decisions.borrow(), b"embedded facade decision");
    let proof = proof.borrow();
    let proof = proof.as_ref().expect("the accepted decision emits a proof");
    assert_eq!(
        WirePacketHeader::parse(proof).unwrap().0.packet_type,
        PacketType::Proof
    );
}
