use super::super::{CompletionPool, Fleet, StaticReactorPool};
use super::*;
use crate::engine::test_support::{bytes_from_hex, RNS_1_4_0_ANNOUNCE};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::reactor::driver::EmbassyHost;
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
const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;

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
fn a_recipe_node_hears_an_ifac_announce_a_supervisor_stands_a_peer_up_for() {
    use crate::interfaces::{IfacContext, IfacSize};

    let notify: &'static Channel<Mtx, InterfaceId, 4> = leak(Channel::new());
    let commands: &'static Channel<Mtx, IssuedCommand, 4> = leak(Channel::new());
    let lifecycle: &'static Channel<Mtx, InterfaceLifecycle, 4> = leak(Channel::new());
    let completion: &'static CompletionPool<Mtx, 4> = leak(CompletionPool::new());

    static POOL: StaticReactorPool<Mtx, FRAME, 4, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"test-supervisor");
    let network = IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
    let supervisor_lane = pool
        .claim_supervisor_with_ifac::<0>(supervisor, network.clone(), leak(Signal::new()))
        .unwrap();

    let handle = PrnsNodeHandle::new(commands.sender(), completion);
    let reactor_wiring = pool.into_reactor_wiring(
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
        routes: crate::routes![],
        interfaces: crate::runtime::Manual,
        on_event: move |event: PrnsEvent<'_>, _state: &()| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { .. }) = event {
                *heard_sink.borrow_mut() += 1;
            }
        },
    };

    let node: PrnsNode<_, _, _, _, _, _, FRAME, 1, 1, 4, 4, 4, 4> = PrnsNode::new(
        recipe,
        reactor_wiring,
        EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
    );

    let raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
    let mut masked = [0u8; FRAME];
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
