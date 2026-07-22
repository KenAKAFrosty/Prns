use super::*;
use crate::reactor::grant::{GrantConsumer, GrantProducer};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

type Mtx = CriticalSectionRawMutex;
const FRAME: usize = 64;
const DEPTH: usize = 2;

#[test]
fn static_pool_storage_can_only_be_taken_once() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    assert!(POOL.try_take().is_ok());
    assert_eq!(POOL.try_take().err(), Some(ReactorPoolError::AlreadyTaken));
}

#[test]
fn a_lane_can_only_be_claimed_once() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    assert!(pool.take_interface::<0>().is_ok());
    assert_eq!(
        pool.take_interface::<0>().err(),
        Some(ReactorPoolError::LaneAlreadyTaken { slot: 0 })
    );
}

#[test]
fn a_pool_lane_pairs_interface_and_reactor_traffic() {
    static POOL: StaticReactorPool<Mtx, FRAME, DEPTH, 1> = StaticReactorPool::new();
    let mut pool = POOL.try_take().unwrap();
    let InterfaceLane {
        mut inbound,
        mut outbound,
    } = pool.take_interface::<0>().unwrap();
    let interface = InterfaceId::new(*b"pooltest");

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
    let _first = pool.take_supervisor::<0>(&FIRST_WAKE).unwrap();
    let _second = pool.take_supervisor::<1>(&SECOND_WAKE).unwrap();
    let interface = InterfaceId::new(*b"supervis");

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
    let _supervisor = pool.take_supervisor::<0>(&FIRST_WAKE).unwrap();
    assert_eq!(
        pool.take_supervisor::<0>(&SECOND_WAKE).err(),
        Some(ReactorPoolError::LaneAlreadyTaken { slot: 0 })
    );
    let interface = InterfaceId::new(*b"supervis");

    let producer = pool.egress.producer_mut(0).unwrap();
    producer.try_grant().unwrap().fill_for(interface, b"wake");
    producer.commit();

    assert!(FIRST_WAKE.signaled());
    assert!(!SECOND_WAKE.signaled());
}
