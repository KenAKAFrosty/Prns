use super::*;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::reactor::grant::{GrantConsumer, GrantProducer};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

type Mtx = CriticalSectionRawMutex;
const FRAME: usize = 64;
const DEPTH: usize = 2;

fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate: BitrateBps::guess(1_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

#[test]
fn static_pool_storage_can_only_be_taken_once() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    assert!(POOL.try_take().is_ok());
    assert_eq!(POOL.try_take().err(), Some(PoolTakeError::AlreadyTaken));
}

#[test]
fn a_lane_can_only_be_claimed_once() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    let first = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"first");
    let second = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"second");
    assert!(pool.claim_interface::<0>(descriptor(first)).is_ok());
    assert_eq!(
        pool.claim_interface::<0>(descriptor(second)).err(),
        Some(LaneClaimError::AlreadyClaimed { slot: 0 })
    );
}

#[test]
fn a_pool_lane_pairs_interface_and_reactor_traffic() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    let interface = InterfaceId::new(*b"pooltest");
    let InterfaceLane {
        id,
        mut inbound,
        mut outbound,
    } = pool.claim_interface::<0>(descriptor(interface)).unwrap();
    assert_eq!(id, interface);

    inbound.try_grant().unwrap().fill_for(interface, b"inbound");
    inbound.commit();
    let reactor_inbound = &mut pool.inbound[0].1;
    assert_eq!(reactor_inbound.try_peek().unwrap().frame(), b"inbound");
    reactor_inbound.release();

    let reactor_outbound = pool.egress.producer_mut(0).unwrap();
    reactor_outbound
        .try_grant()
        .unwrap()
        .fill_for(interface, b"outbound");
    reactor_outbound.commit();
    assert_eq!(outbound.try_peek().unwrap().frame(), b"outbound");
    outbound.release();
}

#[test]
fn a_supervisor_wakes_only_for_its_own_lane() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 2> = StaticReactorPool::new();
    static FIRST_WAKE: Signal<Mtx, ()> = Signal::new();
    static SECOND_WAKE: Signal<Mtx, ()> = Signal::new();
    let mut pool = POOL.try_take().unwrap();
    let interface = InterfaceId::new(*b"supervis");
    let _first = pool.claim_supervisor::<0>(interface, &FIRST_WAKE).unwrap();
    let _second = pool
        .claim_supervisor::<1>(InterfaceId::new(*b"second__"), &SECOND_WAKE)
        .unwrap();

    let producer = pool.egress.producer_mut(0).unwrap();
    producer.try_grant().unwrap().fill_for(interface, b"wake");
    producer.commit();

    assert!(FIRST_WAKE.signaled());
    assert!(!SECOND_WAKE.signaled());
}

#[test]
fn a_rejected_supervisor_claim_does_not_replace_its_wake_signal() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    static FIRST_WAKE: Signal<Mtx, ()> = Signal::new();
    static SECOND_WAKE: Signal<Mtx, ()> = Signal::new();
    let mut pool = POOL.try_take().unwrap();
    let interface = InterfaceId::new(*b"supervis");
    let _supervisor = pool.claim_supervisor::<0>(interface, &FIRST_WAKE).unwrap();
    assert_eq!(
        pool.claim_supervisor::<0>(interface, &SECOND_WAKE).err(),
        Some(LaneClaimError::AlreadyClaimed { slot: 0 })
    );

    let producer = pool.egress.producer_mut(0).unwrap();
    producer.try_grant().unwrap().fill_for(interface, b"wake");
    producer.commit();

    assert!(FIRST_WAKE.signaled());
    assert!(!SECOND_WAKE.signaled());
}
