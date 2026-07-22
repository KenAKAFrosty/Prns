use super::super::{CompletionPool, Fleet, StaticReactorPool};
use super::*;
use crate::engine::test_support::{bytes_from_hex, RNS_1_4_0_ANNOUNCE};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::reactor::driver::{leaked_grant_lane, EmbassyHost};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::runtime::Diagnostic;
use crate::storage::GrowableHeap;
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use std::cell::RefCell;
use std::rc::Rc;

type Mtx = CriticalSectionRawMutex;
const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;

fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
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

type ActivationNode<const MAX_IFACES: usize> = PrnsNode<
    (),
    (),
    for<'a> fn(PrnsEvent<'a>, &()),
    GrowableHeap,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    SLOT,
    1,
    MAX_IFACES,
    4,
    4,
    4,
    4,
>;

fn free_id() -> InterfaceId {
    InterfaceId::new([0xff; 8])
}

fn ignore_event(_event: PrnsEvent<'_>, _state: &()) {}

fn zero_entropy(bytes: &mut [u8]) {
    bytes.fill(0);
}

fn activation_node<const MAX_IFACES: usize>() -> ActivationNode<MAX_IFACES> {
    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());
    let (_in_producer, in_consumer) = leaked_grant_lane::<SLOT>(1);
    let (out_producer, _out_consumer) = leaked_grant_lane::<SLOT>(1);
    let mut inbound = HeaplessVec::new();
    assert!(inbound.push((free_id(), in_consumer)).is_ok());
    let mut egress = HeaplessVec::new();
    assert!(egress.push((free_id(), out_producer)).is_ok());
    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress),
        notify.receiver(),
        commands.receiver(),
        lifecycle.receiver(),
        handle,
    );
    PrnsNode::new(
        PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            routes: crate::routes![],
            interfaces: crate::runtime::Manual,
            on_event: ignore_event as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        EmbassyHost::new(zero_entropy as fn(&mut [u8])),
        HeaplessVec::new(),
    )
}

#[test]
fn unavailable_lane_activation_does_not_change_the_node() {
    let mut node = activation_node::<1>();
    let interface = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"unavailable");
    assert_eq!(
        node.activate(1, descriptor(interface)),
        Err(InterfaceActivationError::LaneUnavailable { slot: 1 })
    );
    assert_eq!(node.inbound[0].0, free_id());
    assert!(node.initial.is_empty());
    assert!(node.ifacs.is_empty());
}

#[test]
fn interface_capacity_failure_does_not_change_the_lane() {
    let mut node = activation_node::<0>();
    let interface = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"full");
    assert_eq!(
        node.activate(0, descriptor(interface)),
        Err(InterfaceActivationError::InterfaceCapacity)
    );
    assert_eq!(node.inbound[0].0, free_id());
    assert!(node.initial.is_empty());
    assert!(node.ifacs.is_empty());
}

#[test]
fn ifac_capacity_failure_does_not_change_the_lane() {
    use crate::interfaces::{IfacContext, IfacSize};

    let mut node = activation_node::<1>();
    let occupied = InterfaceId::from_channel_tag(InterfaceKind::LoRa, b"occupied");
    let occupied_context = IfacContext::derive(Some("occupied"), None, IfacSize::NARROW).unwrap();
    assert!(node
        .ifacs
        .push(InterfaceIfac {
            id: occupied,
            context: occupied_context,
        })
        .is_ok());
    let interface = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"ifac-full");
    let context = IfacContext::derive(Some("new"), None, IfacSize::NARROW).unwrap();
    assert_eq!(
        node.activate_with_ifac(0, descriptor(interface), context),
        Err(InterfaceActivationError::IfacCapacity)
    );
    assert_eq!(node.inbound[0].0, free_id());
    assert!(node.initial.is_empty());
    assert_eq!(node.ifacs.len(), 1);
    assert_eq!(node.ifacs[0].id, occupied);
}

#[test]
fn reactivating_a_lane_replaces_its_descriptor() {
    let mut node = activation_node::<1>();
    let first = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"first");
    let second = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"second");
    assert_eq!(node.activate(0, descriptor(first)), Ok(()));
    assert_eq!(node.activate(0, descriptor(second)), Ok(()));
    assert_eq!(node.inbound[0].0, second);
    assert_eq!(node.initial.as_slice(), &[descriptor(second)]);
}

#[test]
fn supervisor_activation_replaces_an_independent_interface() {
    use crate::interfaces::{IfacContext, IfacSize};

    let mut node = activation_node::<1>();
    let interface = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"interface");
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"supervisor");
    let context = IfacContext::derive(Some("private"), None, IfacSize::NARROW).unwrap();
    assert_eq!(
        node.activate_with_ifac(0, descriptor(interface), context),
        Ok(())
    );
    assert_eq!(node.activate_supervisor(0, supervisor), Ok(()));
    assert_eq!(node.inbound[0].0, supervisor);
    assert!(node.initial.is_empty());
    assert!(node.ifacs.is_empty());
}

#[test]
fn a_recipe_node_hears_an_ifac_announce_a_supervisor_stands_a_peer_up_for() {
    use crate::interfaces::{IfacContext, IfacSize};

    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

    static POOL: StaticReactorPool<Mtx, SLOT, 4, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    let supervisor_lane = pool.take_supervisor::<0>(leak(Signal::new())).unwrap();

    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let plumbing = pool.into_plumbing(
        notify.receiver(),
        commands.receiver(),
        lifecycle.receiver(),
        handle,
    );
    let fleet: Fleet<Mtx, SLOT, 4, 4> =
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
        routes: crate::routes![],
        interfaces: crate::runtime::Manual,
        on_event: move |event: PrnsEvent<'_>, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { .. }) = event {
                *heard_sink.borrow_mut() += 1;
            }
        },
    };

    let mut node = PrnsNode::new(
        recipe,
        plumbing,
        EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
        HeaplessVec::<InterfaceDescriptor, 1>::new(),
    );
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"test-supervisor");
    let network = IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
    assert_eq!(
        node.activate_supervisor_with_ifac(0, supervisor, network.clone()),
        Ok(())
    );

    let raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut masked = [0u8; SLOT];
    let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();
    let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"test-peer-medium");

    let drive = async move {
        let mut fleet = fleet;
        fleet.register_member(descriptor(peer)).await;
        Timer::after(Duration::from_millis(40)).await;

        assert!(
            fleet.deliver_inbound(peer, &masked[..masked_len]),
            "the shared lane carries the peer's frame"
        );
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
