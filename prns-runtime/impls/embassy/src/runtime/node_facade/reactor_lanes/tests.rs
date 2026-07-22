use super::*;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::reactor::grant::{FrameTarget, GrantConsumer, GrantProducer};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

type Mtx = CriticalSectionRawMutex;
const FRAME: usize = 64;
const DEPTH: usize = 2;

fn descriptor(id: InterfaceId, hardware_mtu: usize) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate: BitrateBps::guess(1_000_000),
        hardware_mtu: Some(hardware_mtu),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

#[test]
fn notification_capacity_covers_every_buffered_frame() {
    assert_eq!(minimum_reactor_notification_capacity(1, 1), 1);
    assert_eq!(minimum_reactor_notification_capacity(3, 1), 3);
    assert_eq!(minimum_reactor_notification_capacity(4, 2), 8);
}

#[test]
fn static_lane_storage_can_only_be_claimed_once() {
    static LANE: StaticReactorLane<Mtx, FRAME, DEPTH> = StaticReactorLane::new();
    let id = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"only");
    let mut first: ReactorLaneSet<Mtx, 1, DEPTH> = ReactorLaneSet::new();
    let mut second: ReactorLaneSet<Mtx, 1, DEPTH> = ReactorLaneSet::new();
    assert!(first.claim_interface(&LANE, descriptor(id, FRAME)).is_ok());
    assert_eq!(
        second.claim_interface(&LANE, descriptor(id, FRAME)).err(),
        Some(LaneClaimError::AlreadyClaimed)
    );
}

#[test]
fn a_lane_set_rejects_duplicate_interface_ids_without_consuming_storage() {
    static FIRST: StaticReactorLane<Mtx, FRAME, 1> = StaticReactorLane::new();
    static SECOND: StaticReactorLane<Mtx, FRAME, 1> = StaticReactorLane::new();
    let id = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"same");
    let mut lanes: ReactorLaneSet<Mtx, 2, 2> = ReactorLaneSet::new();
    assert!(lanes.claim_interface(&FIRST, descriptor(id, FRAME)).is_ok());
    assert_eq!(
        lanes.claim_interface(&SECOND, descriptor(id, FRAME)).err(),
        Some(LaneClaimError::DuplicateInterfaceId { id })
    );
    let second_id = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"second");
    assert!(lanes
        .claim_interface(&SECOND, descriptor(second_id, FRAME))
        .is_ok());
}

#[test]
fn heterogeneous_lanes_pair_interface_and_reactor_traffic() {
    const SMALL_FRAME: usize = 16;
    static LANE: StaticReactorLane<Mtx, SMALL_FRAME, 1> = StaticReactorLane::new();
    let interface = InterfaceId::new(*b"lanetest");
    let mut lanes: ReactorLaneSet<Mtx, 1, 1> = ReactorLaneSet::new();
    let InterfaceLane {
        id,
        mut inbound,
        mut outbound,
    } = lanes
        .claim_interface(&LANE, descriptor(interface, SMALL_FRAME))
        .unwrap();
    assert_eq!(id, interface);

    inbound.try_grant().unwrap().fill_for(interface, b"inbound");
    inbound.commit();
    let reactor_inbound = &mut lanes.inbound[0].1;
    assert_eq!(reactor_inbound.try_read().unwrap().2, b"inbound");
    reactor_inbound.release();

    let reactor_outbound = &mut lanes.egress.lanes[0].1;
    assert!(reactor_outbound.try_write(FrameTarget::Direct(interface), b"outbound"));
    assert_eq!(outbound.try_peek().unwrap().frame(), b"outbound");
    GrantConsumer::release(&mut outbound);
}

#[test]
fn supervisors_wake_only_for_their_own_heterogeneous_lane() {
    static FIRST: StaticReactorLane<Mtx, 16, 1> = StaticReactorLane::new();
    static SECOND: StaticReactorLane<Mtx, 32, 1> = StaticReactorLane::new();
    static FIRST_WAKE: Signal<Mtx, ()> = Signal::new();
    static SECOND_WAKE: Signal<Mtx, ()> = Signal::new();
    let first = InterfaceId::new(*b"first___");
    let second = InterfaceId::new(*b"second__");
    let mut lanes: ReactorLaneSet<Mtx, 2, 2> = ReactorLaneSet::new();
    let _first = lanes.claim_supervisor(&FIRST, first, &FIRST_WAKE).unwrap();
    let _second = lanes
        .claim_supervisor(&SECOND, second, &SECOND_WAKE)
        .unwrap();

    assert!(lanes.egress.lanes[0]
        .1
        .try_write(FrameTarget::Direct(first), b"wake"));
    assert!(FIRST_WAKE.signaled());
    assert!(!SECOND_WAKE.signaled());
}

#[test]
fn capacity_failures_do_not_consume_static_lane_storage() {
    static LANE: StaticReactorLane<Mtx, 8, 2> = StaticReactorLane::new();
    let id = InterfaceId::new(*b"capacity");
    let mut shallow: ReactorLaneSet<Mtx, 1, 1> = ReactorLaneSet::new();
    assert_eq!(
        shallow.claim_interface(&LANE, descriptor(id, 8)).err(),
        Some(LaneClaimError::NotificationCapacityExceeded {
            required: 2,
            capacity: 1,
        })
    );

    let mut enough: ReactorLaneSet<Mtx, 1, 2> = ReactorLaneSet::new();
    assert!(enough.claim_interface(&LANE, descriptor(id, 8)).is_ok());
}

#[test]
fn an_interface_cannot_claim_a_lane_smaller_than_its_frame() {
    static LANE: StaticReactorLane<Mtx, 8, 1> = StaticReactorLane::new();
    let id = InterfaceId::new(*b"toosmall");
    let mut lanes: ReactorLaneSet<Mtx, 1, 1> = ReactorLaneSet::new();
    assert_eq!(
        lanes.claim_interface(&LANE, descriptor(id, 9)).err(),
        Some(LaneClaimError::FrameCapacityExceeded {
            required: 9,
            capacity: 8,
        })
    );
}
