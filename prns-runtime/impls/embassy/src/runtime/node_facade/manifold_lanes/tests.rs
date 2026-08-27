use super::super::interface_lifecycle::InboundDeliveryError;
use super::*;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceKind, InterfaceMode, TransportCapability,
};
use crate::manifold::grant::{FrameTarget, GrantConsumer, GrantProducer, LaneWriteOutcome};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

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
        gravity: crate::interfaces::InterfaceGravity::ZERO,
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
    assert_eq!(minimum_manifold_notification_capacity(1, 1), 1);
    assert_eq!(minimum_manifold_notification_capacity(3, 1), 3);
    assert_eq!(minimum_manifold_notification_capacity(4, 2), 8);
}

#[test]
fn static_lane_storage_can_only_be_claimed_once() {
    static LANE: StaticManifoldLane<Mtx, FRAME, DEPTH> = StaticManifoldLane::new();
    let id = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"only");
    let mut first: ManifoldLaneSet<Mtx, 1, DEPTH> = ManifoldLaneSet::new();
    let mut second: ManifoldLaneSet<Mtx, 1, DEPTH> = ManifoldLaneSet::new();
    assert!(first.claim_interface(&LANE, descriptor(id, FRAME)).is_ok());
    assert_eq!(
        second.claim_interface(&LANE, descriptor(id, FRAME)).err(),
        Some(LaneClaimError::AlreadyClaimed)
    );
}

#[test]
fn a_lane_set_rejects_duplicate_interface_ids_without_consuming_storage() {
    static FIRST: StaticManifoldLane<Mtx, FRAME, 1> = StaticManifoldLane::new();
    static SECOND: StaticManifoldLane<Mtx, FRAME, 1> = StaticManifoldLane::new();
    let id = InterfaceId::from_channel_tag(InterfaceKind::UsbAutoDevice, b"same");
    let mut lanes: ManifoldLaneSet<Mtx, 2, 2> = ManifoldLaneSet::new();
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
fn an_accounted_lane_rejects_a_mismatched_status_without_consuming_storage() {
    static LANE: StaticManifoldLane<Mtx, FRAME, 1> = StaticManifoldLane::new();
    static STATUS: EmbassyInterfaceStatus = EmbassyInterfaceStatus::new_accounted(
        InterfaceId::new([0x6B; 8]),
        crate::interfaces::ConnectionState::Initializing,
    );
    let descriptor_id = InterfaceId::new([0x5A; 8]);
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();

    assert_eq!(
        lanes
            .claim_accounted_interface(&LANE, descriptor(descriptor_id, FRAME), &STATUS)
            .err(),
        Some(LaneClaimError::FrameAccountingIdMismatch {
            descriptor: descriptor_id,
            status: InterfaceId::new([0x6B; 8]),
        })
    );
    assert!(lanes
        .claim_interface(&LANE, descriptor(descriptor_id, FRAME))
        .is_ok());
}

#[test]
fn heterogeneous_lanes_pair_interface_and_manifold_traffic() {
    const SMALL_FRAME: usize = 16;
    static LANE: StaticManifoldLane<Mtx, SMALL_FRAME, 1> = StaticManifoldLane::new();
    let interface = InterfaceId::new(*b"lanetest");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
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
    let manifold_inbound = &mut lanes.inbound[0].1;
    assert_eq!(manifold_inbound.try_read().unwrap().2, b"inbound");
    manifold_inbound.release();

    let manifold_outbound = &mut lanes.egress.lanes[0].1;
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(interface), b"outbound"),
        LaneWriteOutcome::Written
    );
    assert_eq!(outbound.try_peek().unwrap().frame(), b"outbound");
    GrantConsumer::release(&mut outbound);
}

#[test]
fn supervisors_wake_only_for_their_own_heterogeneous_lane() {
    static FIRST: StaticManifoldLane<Mtx, 16, 1> = StaticManifoldLane::new();
    static SECOND: StaticManifoldLane<Mtx, 32, 1> = StaticManifoldLane::new();
    static FIRST_WAKE: Signal<Mtx, ()> = Signal::new();
    static SECOND_WAKE: Signal<Mtx, ()> = Signal::new();
    let first = InterfaceId::new(*b"first___");
    let second = InterfaceId::new(*b"second__");
    let mut lanes: ManifoldLaneSet<Mtx, 2, 2> = ManifoldLaneSet::new();
    let _first = lanes.claim_supervisor(&FIRST, first, &FIRST_WAKE).unwrap();
    let _second = lanes
        .claim_supervisor(&SECOND, second, &SECOND_WAKE)
        .unwrap();

    assert_eq!(
        lanes.egress.lanes[0]
            .1
            .try_write(FrameTarget::Direct(first), b"wake"),
        LaneWriteOutcome::Written
    );
    assert!(FIRST_WAKE.signaled());
    assert!(!SECOND_WAKE.signaled());
}

#[test]
fn capacity_failures_do_not_consume_static_lane_storage() {
    static LANE: StaticManifoldLane<Mtx, 8, 2> = StaticManifoldLane::new();
    let id = InterfaceId::new(*b"capacity");
    let mut shallow: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    assert_eq!(
        shallow.claim_interface(&LANE, descriptor(id, 8)).err(),
        Some(LaneClaimError::NotificationCapacityExceeded {
            required: 2,
            capacity: 1,
        })
    );

    let mut enough: ManifoldLaneSet<Mtx, 1, 2> = ManifoldLaneSet::new();
    assert!(enough.claim_interface(&LANE, descriptor(id, 8)).is_ok());
}

#[test]
fn outbound_depth_can_grow_without_spending_inbound_notification_capacity() {
    static LANE: StaticManifoldLane<Mtx, 8, 1, 3> = StaticManifoldLane::new();
    let id = InterfaceId::new(*b"asym-lne");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let InterfaceLane { mut inbound, .. } =
        lanes.claim_interface(&LANE, descriptor(id, 8)).unwrap();

    inbound
        .try_grant()
        .expect("the one inbound slot is free")
        .fill_for(id, b"in");
    inbound.commit();
    assert!(
        inbound.try_grant().is_none(),
        "outbound depth does not silently grow inbound storage"
    );

    let manifold_outbound = &mut lanes.egress.lanes[0].1;
    for frame in [b"one".as_slice(), b"two", b"three"] {
        assert_eq!(
            manifold_outbound.try_write(FrameTarget::Direct(id), frame),
            LaneWriteOutcome::Written
        );
    }
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"four"),
        LaneWriteOutcome::Full
    );
}

#[test]
fn supervisor_can_keep_its_outbound_ring_in_external_storage() {
    static LANE: StaticManifoldLane<Mtx, 8, 1, 0> = StaticManifoldLane::new();
    static WAKE: Signal<Mtx, ()> = Signal::new();
    let external = std::boxed::Box::leak(
        std::vec![
            FrameSlot::<8>::empty(),
            FrameSlot::empty(),
            FrameSlot::empty()
        ]
        .into_boxed_slice(),
    );
    let id = InterfaceId::new(*b"external");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let _supervisor = lanes
        .claim_supervisor_with_outbound_buffer(&LANE, id, &WAKE, external)
        .unwrap();

    let manifold_outbound = &mut lanes.egress.lanes[0].1;
    for frame in [b"one".as_slice(), b"two", b"three"] {
        assert_eq!(
            manifold_outbound.try_write(FrameTarget::Direct(id), frame),
            LaneWriteOutcome::Written
        );
    }
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"four"),
        LaneWriteOutcome::Full
    );
}

#[test]
fn interface_can_keep_its_outbound_ring_in_external_storage() {
    static LANE: StaticManifoldLane<Mtx, 8, 1, 0> = StaticManifoldLane::new();
    let external = std::boxed::Box::leak(
        std::vec![FrameSlot::<8>::empty(), FrameSlot::empty()].into_boxed_slice(),
    );
    let id = InterfaceId::new(*b"ext-ifce");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let _interface = lanes
        .claim_interface_with_outbound_buffer(&LANE, descriptor(id, 8), external)
        .unwrap();

    let manifold_outbound = &mut lanes.egress.lanes[0].1;
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"one"),
        LaneWriteOutcome::Written
    );
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"two"),
        LaneWriteOutcome::Written
    );
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"full"),
        LaneWriteOutcome::Full
    );
}

#[test]
fn an_empty_external_ring_is_rejected_without_consuming_the_lane() {
    static LANE: StaticManifoldLane<Mtx, 8, 1, 0> = StaticManifoldLane::new();
    static WAKE: Signal<Mtx, ()> = Signal::new();
    let id = InterfaceId::new(*b"emptybuf");
    let mut first: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let empty = std::boxed::Box::leak(std::vec::Vec::new().into_boxed_slice());
    assert_eq!(
        first
            .claim_supervisor_with_outbound_buffer(&LANE, id, &WAKE, empty)
            .err(),
        Some(LaneClaimError::EmptyOutboundBuffer)
    );

    let mut second: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let external = std::boxed::Box::leak(std::vec![FrameSlot::<8>::empty()].into_boxed_slice());
    assert!(second
        .claim_supervisor_with_outbound_buffer(&LANE, id, &WAKE, external)
        .is_ok());
}

#[test]
fn an_interface_cannot_claim_a_lane_smaller_than_its_frame() {
    static LANE: StaticManifoldLane<Mtx, 8, 1> = StaticManifoldLane::new();
    let id = InterfaceId::new(*b"toosmall");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    assert_eq!(
        lanes.claim_interface(&LANE, descriptor(id, 9)).err(),
        Some(LaneClaimError::FrameCapacityExceeded {
            required: 9,
            capacity: 8,
        })
    );
}

#[test]
fn a_full_static_lane_retains_egress_pressure_evidence() {
    static LANE: StaticManifoldLane<Mtx, 8, 1> = StaticManifoldLane::new();
    let id = InterfaceId::new(*b"pressure");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let _interface = lanes.claim_interface(&LANE, descriptor(id, 8)).unwrap();
    let manifold_outbound = &mut lanes.egress.lanes[0].1;

    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"first"),
        LaneWriteOutcome::Written
    );
    assert_eq!(
        manifold_outbound.try_write(FrameTarget::Direct(id), b"second"),
        LaneWriteOutcome::Full
    );
    assert_eq!(LANE.egress_pressure_events(), 1);
}

#[test]
fn a_full_supervisor_lane_retains_ingress_pressure_evidence() {
    static LANE: StaticManifoldLane<Mtx, 8, 1> = StaticManifoldLane::new();
    static WAKE: Signal<Mtx, ()> = Signal::new();
    static NOTIFY: Channel<Mtx, InterfaceId, 1> = Channel::new();
    static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, 1> = Channel::new();
    let supervisor = InterfaceId::new(*b"super___");
    let member = InterfaceId::new(*b"member__");
    let mut lanes: ManifoldLaneSet<Mtx, 1, 1> = ManifoldLaneSet::new();
    let lane = lanes.claim_supervisor(&LANE, supervisor, &WAKE).unwrap();
    let mut fleet = lane.into_fleet(NOTIFY.sender(), LIFECYCLE.sender());

    assert_eq!(fleet.try_deliver_inbound(member, b"first"), Ok(()));
    assert_eq!(
        fleet.try_deliver_inbound(member, b"second"),
        Err(InboundDeliveryError::LaneFull)
    );
    assert_eq!(LANE.ingress_pressure_events(), 1);
}
