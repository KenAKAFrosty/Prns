use super::*;
use crate::engine::test_support::{bytes_from_hex, RNS_1_3_5_ANNOUNCE};
use crate::engine::FanTarget;
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::reactor::driver::{leaked_grant_lane, EmbassyHost};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::runtime::Diagnostic;
use crate::storage::GrowableHeap;
use crate::units::RttMillis;
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};
use std::rc::Rc;

type Pool<const N: usize> = CompletionPool<CriticalSectionRawMutex, N>;

fn delivered(ms: u64) -> Settlement {
    Settlement::SendSinglePacket(Ok(PacketReceiptDelivered {
        rtt: RttMillis::new(ms),
    }))
}

#[test]
fn the_pool_mints_a_distinct_id_each_time() {
    let pool: Pool<2> = CompletionPool::new();
    assert_eq!(pool.mint(), CommandId(0));
    assert_eq!(pool.mint(), CommandId(1));
    assert_eq!(pool.mint(), CommandId(2));
}

#[test]
fn the_pool_bounds_concurrent_awaited_sends() {
    let pool: Pool<2> = CompletionPool::new();
    let first = pool.claim(CommandId(0));
    let second = pool.claim(CommandId(1));
    assert!(first.is_some() && second.is_some());
    assert_ne!(first, second);
    assert_eq!(
        pool.claim(CommandId(2)),
        None,
        "a full pool refuses a claim"
    );
}

#[test]
fn settle_wakes_only_the_slot_awaiting_that_id() {
    let pool: Pool<3> = CompletionPool::new();
    pool.claim(CommandId(10));
    pool.claim(CommandId(11));
    pool.claim(CommandId(12));
    assert!(
        !pool.settle(CommandId(99), delivered(1)),
        "no slot awaits 99"
    );
    assert!(pool.settle(CommandId(11), delivered(1)));
    assert!(pool.settle(CommandId(10), delivered(1)));
    assert!(pool.settle(CommandId(12), delivered(1)));
}

#[test]
fn a_settled_slot_frees_for_reuse() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    assert!(pool.claim(id).is_some());
    assert_eq!(pool.claim(CommandId(1)), None, "full while id awaits");
    assert!(pool.settle(id, delivered(1)));
    assert!(
        pool.claim(CommandId(1)).is_some(),
        "the slot frees once settled"
    );
}

#[test]
fn a_cancelled_await_releases_its_slot_and_ignores_a_late_settlement() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    let slot = pool.claim(id).expect("a slot");
    pool.release(slot, id);
    assert!(
        !pool.settle(id, delivered(1)),
        "a settlement for a released await fires nothing"
    );
    assert!(
        pool.claim(CommandId(1)).is_some(),
        "the released slot is reusable"
    );
}

#[test]
fn a_late_release_never_clobbers_a_newer_claimant() {
    let pool: Pool<1> = CompletionPool::new();
    let first = CommandId(0);
    let slot = pool.claim(first).expect("a slot");
    assert!(pool.settle(first, delivered(1)));

    let second = CommandId(1);
    assert_eq!(pool.claim(second), Some(slot), "the same slot is reused");
    pool.release(slot, first);
    assert!(
        pool.settle(second, delivered(2)),
        "the stale release left the new claimant intact"
    );
}

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

#[test]
fn next_outbound_releases_the_copied_grant_so_the_depth_one_lane_refills() {
    use crate::reactor::grant::AnyGrantProducer;

    let (inbound, _inbound_rx) = leaked_grant_lane::<SLOT>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<SLOT>(1);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, SLOT, 1, 1> = Fleet::new(
        MemberWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );

    assert!(outbound_tx.try_fill_frame_fan(FanTarget::All, b"one"));
    let (target, frame) = block_on(fleet.next_outbound::<SLOT>());
    assert_eq!(target, FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.as_slice(), b"one");

    assert!(
        outbound_tx.try_fill_frame_fan(FanTarget::All, b"two"),
        "the depth-1 lane must accept the next frame the instant next_outbound copied the last"
    );
    let (target, frame) = block_on(fleet.next_outbound::<SLOT>());
    assert_eq!(target, FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.as_slice(), b"two");
}

#[test]
fn an_outbound_commit_wakes_the_supervisor_and_try_next_outbound_drains() {
    use crate::reactor::grant::AnyGrantProducer;

    let (inbound, _inbound_rx) = leaked_grant_lane::<SLOT>(1);
    let (mut outbound_tx, outbound) = leaked_grant_lane::<SLOT>(1);
    let wake: &'static Signal<Mtx, ()> = leak(Signal::new());
    outbound_tx.set_outbound_wake(wake);
    let notify: &'static Channel<Mtx, InterfaceId, 1> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 1> = leak(Channel::new());
    let mut fleet: Fleet<Mtx, SLOT, 1, 1> = Fleet::new(
        MemberWire {
            inbound,
            outbound,
            notify: notify.sender(),
            outbound_wake: wake,
        },
        lifecycle.sender(),
    );

    assert!(
        fleet.try_next_outbound::<SLOT>().is_none(),
        "an empty lane drains to nothing"
    );

    assert!(outbound_tx.try_fill_frame_fan(FanTarget::All, b"hi"));
    block_on(with_timeout(
        Duration::from_millis(50),
        fleet.outbound_ready(),
    ))
    .expect("the commit must signal the outbound wake");

    let (target, frame) = fleet
        .try_next_outbound::<SLOT>()
        .expect("the committed frame drains after the wake");
    assert_eq!(target, FrameTarget::Fan(FanTarget::All));
    assert_eq!(frame.as_slice(), b"hi");
    assert!(
        fleet.try_next_outbound::<SLOT>().is_none(),
        "the depth-1 lane is empty once drained"
    );
}

#[test]
fn a_recipe_node_hears_an_ifac_announce_a_supervisor_stands_a_peer_up_for() {
    use crate::interfaces::ifac::{IfacContext, IfacSize};

    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

    let (in_producer, in_consumer) = leaked_grant_lane::<SLOT>(4);
    let (out_producer, out_consumer) = leaked_grant_lane::<SLOT>(4);

    let free = InterfaceId::new([0xff; 8]);
    let mut inbound: HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, Mtx, SLOT>), 1> =
        HeaplessVec::new();
    let _ = inbound.push((free, in_consumer));
    let mut egress_lanes: HeaplessVec<(InterfaceId, EmbassyGrantProducer<'static, Mtx, SLOT>), 1> =
        HeaplessVec::new();
    let _ = egress_lanes.push((free, out_producer));

    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        notify.receiver(),
        commands.receiver(),
        lifecycle.receiver(),
        handle,
    );

    let fleet: Fleet<Mtx, SLOT, 4, 4> = Fleet::new(
        MemberWire {
            inbound: in_producer,
            outbound: out_consumer,
            notify: notify.sender(),
            outbound_wake: leak(Signal::new()),
        },
        lifecycle.sender(),
    );

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
    assert!(node.activate_fleet_with_ifac(0, supervisor, network.clone()));

    let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut masked = [0u8; SLOT];
    let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();
    let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"test-peer-medium");

    let drive = async move {
        let mut fleet = fleet;
        assert!(
            fleet.register_member(descriptor(peer)),
            "the lifecycle lane accepts the add"
        );
        Timer::after(Duration::from_millis(40)).await;

        assert!(
            fleet.deliver_inbound(peer, &masked[..masked_len]),
            "the shared lane carries the peer's frame"
        );
        Timer::after(Duration::from_millis(80)).await;

        assert!(
            fleet.deregister_member(peer),
            "the lifecycle lane accepts the remove"
        );
        Timer::after(Duration::from_millis(20)).await;
    };

    let _ = block_on(with_timeout(Duration::from_millis(600), node.run(drive)));
    assert_eq!(
        *heard.borrow(),
        1,
        "the node heard the announce the supervisor's peer carried in"
    );
}
