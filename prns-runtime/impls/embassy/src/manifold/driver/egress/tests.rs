use crate::engine::test_support::{bytes_from_hex, routable_descriptor, RNS_1_4_2_ANNOUNCE};
use crate::engine::{Directive, EngineReaction, FanTarget, InstantMillis};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, InterfaceDescriptor, InterfaceId, InterfaceIfac,
    InterfaceKind,
};
use crate::manifold::grant::{FrameTarget, GrantConsumer};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;

use super::super::leaked_grant_lane;
use super::{
    enqueue_broadcast_for_wire, enqueue_for_wire, flush_due_pacers, route_reaction,
    soonest_pacer_release, EgressOutcome, InterfacePacer, ManifoldEgress, PooledEgress,
    PACER_DEPTH,
};

fn paced_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        bitrate: BitrateBps::guess(5_000),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        ..routable_descriptor(id)
    }
}

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
    let clean = bytes_from_hex(RNS_1_4_2_ANNOUNCE);

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

#[test]
fn a_pacer_retains_back_to_back_announces_for_a_one_slot_direct_lane() {
    let id = InterfaceId::new([0x55; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    let mut pacers = [InterfacePacer::from_descriptor(id, &paced_descriptor(id))];

    for bytes in [b"delivery".as_slice(), b"node"] {
        route_reaction(
            EngineReaction::Directive(Directive::SendAnnounce {
                target: id,
                bytes,
                hops: 0,
            }),
            &mut egress,
            &[],
            &mut pacers,
            InstantMillis(0),
            &mut |_| {},
        );
    }

    let first = consumer
        .try_peek()
        .expect("the first announce enters egress");
    assert_eq!(first.target, FrameTarget::Direct(id));
    assert_eq!(first.frame(), b"delivery");
    consumer.release();
    assert!(
        consumer.try_peek().is_none(),
        "the second announce waits in the pacer"
    );

    let due = soonest_pacer_release(&pacers).expect("the retained announce has a deadline");
    flush_due_pacers(&mut pacers, due, &mut egress, &[]);
    let second = consumer
        .try_peek()
        .expect("the retained announce enters the released lane");
    assert_eq!(second.target, FrameTarget::Direct(id));
    assert_eq!(second.frame(), b"node");
    consumer.release();
}

#[test]
fn a_full_direct_lane_defers_on_the_embedded_retry_clock_then_recovers() {
    let id = InterfaceId::new([0x58; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    let mut pacers = [InterfacePacer::from_descriptor(id, &paced_descriptor(id))];
    assert_eq!(egress.enqueue(id, b"occupied"), EgressOutcome::Enqueued);

    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"retained",
            hops: 1,
        }),
        &mut egress,
        &[],
        &mut pacers,
        InstantMillis(0),
        &mut |_| {},
    );
    assert_eq!(pacers[0].pacer.queued_len(), 1);
    assert_eq!(pacers[0].pacer.deferred_len(), 1);
    assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(250)));

    flush_due_pacers(&mut pacers, InstantMillis(250), &mut egress, &[]);
    assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(750)));
    assert_eq!(
        consumer.try_peek().expect("occupied slot").frame(),
        b"occupied"
    );
    consumer.release();

    flush_due_pacers(&mut pacers, InstantMillis(750), &mut egress, &[]);
    assert_eq!(
        consumer.try_peek().expect("recovered announce").frame(),
        b"retained"
    );
    assert_eq!(pacers[0].pacer.queued_len(), 0);
    assert_eq!(pacers[0].pacer.deferred_len(), 0);
}

#[test]
fn a_pacer_retains_back_to_back_announces_for_a_one_slot_fleet_lane() {
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"announce-fleet");
    let peer = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x66, 0, 0, 0, 0, 0, 0]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        supervisor,
        std::boxed::Box::leak(std::boxed::Box::new(producer)),
    );
    let mut pacers = [InterfacePacer::from_descriptor(
        supervisor,
        &paced_descriptor(peer),
    )];

    for bytes in [b"delivery".as_slice(), b"node"] {
        route_reaction(
            EngineReaction::Directive(Directive::SendAnnounceToFleet {
                supervisor: InterfaceKind::BluetoothAuto,
                fan: FanTarget::All,
                bytes,
                hops: 0,
            }),
            &mut egress,
            &[],
            &mut pacers,
            InstantMillis(0),
            &mut |_| {},
        );
    }

    let first = consumer
        .try_peek()
        .expect("the first announce enters egress");
    assert_eq!(first.target, FrameTarget::Fan(FanTarget::All));
    assert_eq!(first.frame(), b"delivery");
    consumer.release();
    assert!(
        consumer.try_peek().is_none(),
        "the second fleet announce waits in the shared pacer"
    );

    let due = soonest_pacer_release(&pacers).expect("the retained announce has a deadline");
    flush_due_pacers(&mut pacers, due, &mut egress, &[]);
    let second = consumer
        .try_peek()
        .expect("the retained fleet announce enters the released lane");
    assert_eq!(second.target, FrameTarget::Fan(FanTarget::All));
    assert_eq!(second.frame(), b"node");
    consumer.release();
}

#[test]
fn a_full_fleet_lane_defers_then_recovers_without_losing_its_fan_target() {
    let supervisor =
        InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"pressured-fleet");
    let peer = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x67, 0, 0, 0, 0, 0, 0]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        supervisor,
        std::boxed::Box::leak(std::boxed::Box::new(producer)),
    );
    let mut pacers = [InterfacePacer::from_descriptor(
        supervisor,
        &paced_descriptor(peer),
    )];
    assert_eq!(
        egress.enqueue_broadcast(InterfaceKind::BluetoothAuto, FanTarget::All, b"occupied"),
        EgressOutcome::Enqueued
    );

    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor: InterfaceKind::BluetoothAuto,
            fan: FanTarget::AllExcept(peer),
            bytes: b"retained",
            hops: 1,
        }),
        &mut egress,
        &[],
        &mut pacers,
        InstantMillis(0),
        &mut |_| {},
    );
    assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(250)));
    assert_eq!(
        consumer.try_peek().expect("occupied slot").frame(),
        b"occupied"
    );
    consumer.release();

    flush_due_pacers(&mut pacers, InstantMillis(250), &mut egress, &[]);
    let recovered = consumer.try_peek().expect("recovered fleet announce");
    assert_eq!(recovered.frame(), b"retained");
    assert_eq!(
        recovered.target,
        FrameTarget::Fan(FanTarget::AllExcept(peer))
    );
    assert_eq!(pacers[0].pacer.queued_len(), 0);
}

#[test]
fn embedded_fixed_queue_sheds_the_worst_hops_under_lane_pressure() {
    let id = InterfaceId::new([0x59; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(1);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    let mut pacers = [InterfacePacer::from_descriptor(id, &paced_descriptor(id))];
    assert_eq!(egress.enqueue(id, b"occupied"), EgressOutcome::Enqueued);

    for (bytes, hops, at) in [
        (b"evict".as_slice(), 5, 0),
        (b"keep".as_slice(), 5, 1),
        (b"best".as_slice(), 1, 2),
    ] {
        route_reaction(
            EngineReaction::Directive(Directive::SendAnnounce {
                target: id,
                bytes,
                hops,
            }),
            &mut egress,
            &[],
            &mut pacers,
            InstantMillis(at),
            &mut |_| {},
        );
    }
    assert_eq!(pacers[0].pacer.queued_len(), PACER_DEPTH);
    consumer.try_peek().expect("occupied slot");
    consumer.release();

    let first_due = soonest_pacer_release(&pacers).unwrap();
    flush_due_pacers(&mut pacers, first_due, &mut egress, &[]);
    assert_eq!(consumer.try_peek().expect("best hops").frame(), b"best");
    consumer.release();
    let second_due = soonest_pacer_release(&pacers).unwrap();
    flush_due_pacers(&mut pacers, second_due, &mut egress, &[]);
    assert_eq!(consumer.try_peek().expect("survivor").frame(), b"keep");
}

#[test]
fn paced_oversize_ifac_rejection_and_missing_lane_are_terminal() {
    use crate::interfaces::{IfacContext, IfacSize};

    let id = InterfaceId::new([0x5a; 8]);
    let mut pacers = [InterfacePacer::from_descriptor(id, &paced_descriptor(id))];
    let (producer, mut consumer) = leaked_grant_lane::<4>(1);
    let mut small_egress: PooledEgress<1> = PooledEgress::new();
    let _ = small_egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"oversize",
            hops: 1,
        }),
        &mut small_egress,
        &[],
        &mut pacers,
        InstantMillis(0),
        &mut |_| {},
    );
    assert!(consumer.try_peek().is_none());
    assert!(pacers[0].pacer.is_idle());
    assert_eq!(soonest_pacer_release(&pacers), None);

    let (producer, mut consumer) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
    let mut ifac_egress: PooledEgress<1> = PooledEgress::new();
    let _ = ifac_egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    let ifacs = [InterfaceIfac {
        id,
        context: IfacContext::derive(Some("network"), Some("secret"), IfacSize::NARROW).unwrap(),
    }];
    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"x",
            hops: 1,
        }),
        &mut ifac_egress,
        &ifacs,
        &mut pacers,
        InstantMillis(1),
        &mut |_| {},
    );
    assert!(consumer.try_peek().is_none());
    assert!(pacers[0].pacer.is_idle());

    let mut missing_egress: PooledEgress<0> = PooledEgress::new();
    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"missing",
            hops: 1,
        }),
        &mut missing_egress,
        &[],
        &mut pacers,
        InstantMillis(2),
        &mut |_| {},
    );
    assert!(pacers[0].pacer.is_idle());
    assert_eq!(soonest_pacer_release(&pacers), None);
}

#[test]
fn a_locally_originated_announce_does_not_wait_for_rebroadcast_cooldown() {
    let id = InterfaceId::new([0x57; 8]);
    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (producer, mut consumer) = leaked_grant_lane::<FRAME>(2);
    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)));
    let mut pacers = [InterfacePacer::from_descriptor(id, &paced_descriptor(id))];

    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"forwarded",
            hops: 1,
        }),
        &mut egress,
        &[],
        &mut pacers,
        InstantMillis(0),
        &mut |_| {},
    );
    route_reaction(
        EngineReaction::Directive(Directive::SendAnnounce {
            target: id,
            bytes: b"origin",
            hops: 0,
        }),
        &mut egress,
        &[],
        &mut pacers,
        InstantMillis(1),
        &mut |_| {},
    );

    let first = consumer
        .try_peek()
        .expect("the forwarded announce enters egress");
    assert_eq!(first.frame(), b"forwarded");
    consumer.release();
    let second = consumer
        .try_peek()
        .expect("the origin announce does not wait for rebroadcast cooldown");
    assert_eq!(second.frame(), b"origin");
    consumer.release();
}
