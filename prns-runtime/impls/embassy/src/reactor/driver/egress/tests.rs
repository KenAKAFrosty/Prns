use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use heapless::Vec as HeaplessVec;

use crate::engine::test_support::{bytes_from_hex, RNS_1_3_5_ANNOUNCE};
use crate::engine::FanTarget;
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::reactor::grant::{FrameTarget, GrantConsumer};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;

use super::super::{leaked_grant_lane, EmbassyGrantProducer};
use super::{enqueue_broadcast_for_wire, enqueue_for_wire, PooledEgress};

#[test]
fn pooled_egress_retag_relabels_a_lane_and_ignores_a_missing_id() {
    let old_id = InterfaceId::new([0x11; 8]);
    let new_id = InterfaceId::new([0x22; 8]);
    const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, _consumer) = leaked_grant_lane::<SLOT>(2);
    let mut lanes: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = lanes.push((old_id, producer));
    let mut egress = PooledEgress::new(lanes);

    egress.retag(old_id, new_id);
    assert_eq!(egress.lanes[0].0, new_id, "the lane carries the new id");
    egress.retag(old_id, new_id);
    assert_eq!(egress.lanes[0].0, new_id, "retagging a gone id is a no-op");
}

#[test]
fn a_fleet_lane_masks_direct_and_broadcast_frames_once() {
    use crate::interfaces::ifac::{IfacContext, IfacSize};

    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"private-fleet");
    let child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer");
    const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<SLOT>(2);
    let mut lanes: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = lanes.push((supervisor, producer));
    let mut egress = PooledEgress::new(lanes);
    let network = IfacContext::derive(Some("fleet-net"), Some("secret"), IfacSize::NARROW).unwrap();
    let ifacs = [InterfaceIfac {
        id: supervisor,
        context: network.clone(),
    }];
    let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);

    enqueue_for_wire(&mut egress, &ifacs, child, &clean);
    let direct = consumer.try_peek().unwrap();
    assert_eq!(direct.target, FrameTarget::Direct(child));
    let mut opened = [0u8; SLOT];
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
