use crate::engine::test_support::{bytes_from_hex, RNS_1_4_0_ANNOUNCE};
use crate::engine::FanTarget;
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::reactor::grant::{FrameTarget, GrantConsumer};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;

use super::super::leaked_grant_lane;
use super::{
    enqueue_broadcast_for_wire, enqueue_for_wire, EgressOutcome, PooledEgress, ReactorEgress,
};

#[test]
fn pooled_egress_retag_relabels_a_lane_and_ignores_a_missing_id() {
    let old_id = InterfaceId::new([0x11; 8]);
    let new_id = InterfaceId::new([0x22; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, _consumer) = leaked_grant_lane::<FRAME>(2);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        old_id,
        std::boxed::Box::leak(std::boxed::Box::new(producer)),
    );

    egress.retag(old_id, new_id);
    assert_eq!(egress.lanes[0].0, new_id, "the lane carries the new id");
    egress.retag(old_id, new_id);
    assert_eq!(egress.lanes[0].0, new_id, "retagging a gone id is a no-op");
}

#[test]
fn pooled_egress_distinguishes_a_full_lane_from_missing_topology() {
    let id = InterfaceId::new([0x33; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, _consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));

    assert_eq!(egress.enqueue(id, b"first"), EgressOutcome::Enqueued);
    assert_eq!(
        egress.enqueue(id, b"second"),
        EgressOutcome::LaneFull { lane: id }
    );
    assert_eq!(
        egress.enqueue(InterfaceId::new([0x44; 8]), b"missing"),
        EgressOutcome::NoLane
    );
}

#[test]
fn a_fleet_lane_masks_direct_and_broadcast_frames_once() {
    use crate::interfaces::{IfacContext, IfacSize};

    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"private-fleet");
    let child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer");
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(2);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        supervisor,
        std::boxed::Box::leak(std::boxed::Box::new(producer)),
    );
    let network = IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
    let ifacs = [InterfaceIfac {
        id: supervisor,
        context: network.clone(),
    }];
    let clean = bytes_from_hex(RNS_1_4_0_ANNOUNCE);

    enqueue_for_wire(&mut egress, &ifacs, child, &clean);
    let direct = consumer.try_peek().unwrap();
    assert_eq!(direct.target, FrameTarget::Direct(child));
    let mut opened = [0u8; FRAME];
    let opened_len = network.unmask_inbound(direct.frame(), &mut opened).unwrap();
    assert_eq!(&opened[..opened_len], clean.as_slice());
    consumer.release();

    enqueue_broadcast_for_wire(
        &mut egress,
        &ifacs,
        InterfaceKind::AutoWifi,
        FanTarget::All,
        &clean,
    );
    let broadcast = consumer.try_peek().unwrap();
    assert_eq!(broadcast.target, FrameTarget::Fan(FanTarget::All));
    let opened_len = network
        .unmask_inbound(broadcast.frame(), &mut opened)
        .unwrap();
    assert_eq!(&opened[..opened_len], clean.as_slice());
    consumer.release();
}
