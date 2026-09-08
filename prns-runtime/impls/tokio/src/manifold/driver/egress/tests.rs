use super::*;

#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::InstantMillis;
#[cfg(feature = "runtime-metrics")]
use crate::interfaces::InterfaceKind;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConnectionState, ConnectionView, InterfaceId,
};
#[cfg(feature = "runtime-metrics")]
use crate::interfaces::{IfacContext, IfacSize, InterfaceIfac};
use crate::manifold::grant_lane::tokio_grant_lane;
use crate::manifold::interface_seam::MAX_WIRE_FRAME_LEN;
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{
    AnnounceBackpressureEvent, AnnounceEgressOutcome, EgressLaneMetricsSnapshot,
    EgressMetricsSnapshot,
};
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireAddress,
};

use super::super::interface_status::TokioInterfaceStatus;

fn forwarded_header(hops: u8) -> WirePacketHeader {
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops,
        transport_id: None,
        address: WireAddress::new([0x44; 16]),
        context: WireContext::Resource,
    }
}

#[test]
fn same_shape_forwarding_moves_the_ingress_allocation_into_egress() {
    let target = InterfaceId::new([0x71; 8]);
    let payload = [0xA5; 96];
    let mut wire = vec![0u8; HEADER_MAX_LEN + payload.len()];
    let header_len = forwarded_header(0).write(&mut wire).unwrap();
    wire[header_len..header_len + payload.len()].copy_from_slice(&payload);
    wire.truncate(header_len + payload.len());

    let (mut inbound_producer, mut inbound_consumer) = tokio_grant_lane(256, 1);
    inbound_producer.try_grant().unwrap().fill(&wire);
    inbound_producer.commit();
    inbound_consumer.try_peek().unwrap();
    let source = inbound_consumer.take_peeked().unwrap();
    let source_allocation = source.bytes.as_ptr();
    let payload_range = header_len..wire.len();

    let (outbound_producer, mut outbound_consumer) = tokio_grant_lane(256, 1);
    let mut egress = Egress::new(vec![(target, outbound_producer)]);
    let ForwardedSlotOutcome::Moved { vacated } =
        egress.try_move_forwarded_slot(target, forwarded_header(1), payload_range, source)
    else {
        panic!("same-shape forwarding moves the slot");
    };

    assert!(vacated.bytes.is_empty());
    let forwarded = outbound_consumer.try_peek().unwrap();
    assert_eq!(forwarded.bytes.as_ptr(), source_allocation);
    let (header, forwarded_payload) = WirePacketHeader::parse(forwarded.frame()).unwrap();
    assert_eq!(
        (header, forwarded_payload),
        (forwarded_header(1), payload.as_slice())
    );
}

#[test]
fn changed_header_shape_keeps_the_source_for_the_copy_fallback() {
    let target = InterfaceId::new([0x72; 8]);
    let mut incoming_header = forwarded_header(0);
    incoming_header.transport_id = Some(crate::wire::TransportId::new([0x55; 16]));
    let payload = [0x5A; 96];
    let mut wire = vec![0u8; HEADER_MAX_LEN + payload.len()];
    let header_len = incoming_header.write(&mut wire).unwrap();
    wire[header_len..header_len + payload.len()].copy_from_slice(&payload);
    wire.truncate(header_len + payload.len());

    let (mut inbound_producer, mut inbound_consumer) = tokio_grant_lane(256, 1);
    inbound_producer.try_grant().unwrap().fill(&wire);
    inbound_producer.commit();
    inbound_consumer.try_peek().unwrap();
    let source = inbound_consumer.take_peeked().unwrap();
    let payload_range = header_len..wire.len();

    let (outbound_producer, mut outbound_consumer) = tokio_grant_lane(256, 1);
    let mut egress = Egress::new(vec![(target, outbound_producer)]);
    let ForwardedSlotOutcome::CopyRequired { source } =
        egress.try_move_forwarded_slot(target, forwarded_header(1), payload_range.clone(), source)
    else {
        panic!("a changed header shape retains the source slot");
    };

    assert_eq!(&source.frame()[payload_range], &payload);
    assert!(outbound_consumer.try_peek().is_none());
}

#[test]
fn link_traffic_overtakes_buffered_resource_parts() {
    let id = InterfaceId::new([0x90; 8]);
    let (producer, mut consumer) = tokio_grant_lane(64, 2);
    let mut egress = Egress::new(std::vec![(id, producer)]);
    let mut resource_part = [0u8; 19];
    resource_part[18] = WireContext::Resource.to_byte();
    let mut link_traffic = [0u8; 19];
    link_traffic[18] = WireContext::None.to_byte();

    assert_eq!(
        egress.enqueue(id, &resource_part),
        EgressEnqueueOutcome::Enqueued
    );
    assert_eq!(
        egress.enqueue(id, &link_traffic),
        EgressEnqueueOutcome::Enqueued
    );

    assert_eq!(
        WirePacketHeader::parse(consumer.try_peek().unwrap().frame())
            .unwrap()
            .0
            .context,
        WireContext::None
    );
    consumer.release();
    assert_eq!(
        WirePacketHeader::parse(consumer.try_peek().unwrap().frame())
            .unwrap()
            .0
            .context,
        WireContext::Resource
    );
}

#[test]
fn deferred_link_traffic_overtakes_bulk_with_the_same_bounded_streak() {
    let source = InterfaceId::new([0x80; 8]);
    let target = InterfaceId::new([0x8F; 8]);
    let (producer, mut consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(target, producer)]);
    let mut resource_part = [0u8; 20];
    resource_part[18] = WireContext::Resource.to_byte();
    resource_part[19] = 0xFE;

    assert_eq!(
        egress.enqueue_from_ingress(source, target, b"occupy"),
        EgressEnqueueOutcome::Enqueued
    );
    assert_eq!(
        egress.enqueue_from_ingress(source, target, &resource_part),
        EgressEnqueueOutcome::Deferred
    );
    for index in 0..9u8 {
        let mut link_traffic = [0u8; 20];
        link_traffic[18] = WireContext::None.to_byte();
        link_traffic[19] = index;
        assert_eq!(
            egress.enqueue_from_ingress(source, target, &link_traffic),
            EgressEnqueueOutcome::Deferred
        );
    }

    consumer.try_peek().unwrap();
    consumer.release();
    for expected in 0..8u8 {
        assert_eq!(egress.flush_pending(1), 1);
        assert_eq!(consumer.try_peek().unwrap().frame()[19], expected);
        consumer.release();
    }
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(consumer.try_peek().unwrap().frame(), &resource_part);
    consumer.release();
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(consumer.try_peek().unwrap().frame()[19], 8);
}

#[tokio::test]
async fn saturated_egress_defers_its_source_without_blocking_an_independent_lane() {
    let source = InterfaceId::new([0x81; 8]);
    let independent_source = InterfaceId::new([0x82; 8]);
    let target = InterfaceId::new([0x83; 8]);
    let independent_target = InterfaceId::new([0x84; 8]);
    let (release_notify, releases) = super::super::manifold_wake();
    let (target_producer, mut target_consumer) = tokio_grant_lane(64, 1);
    target_consumer.notify_releases_to(release_notify);
    let (independent_producer, mut independent_consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![
        (target, target_producer),
        (independent_target, independent_producer),
    ]);

    assert_eq!(
        egress.enqueue_from_ingress(source, target, b"first"),
        EgressEnqueueOutcome::Enqueued
    );
    assert_eq!(
        egress.enqueue_from_ingress(source, target, b"deferred"),
        EgressEnqueueOutcome::Deferred
    );
    assert!(egress.blocks_source(source));
    assert!(!egress.blocks_source(independent_source));
    assert_eq!(
        egress.enqueue_from_ingress(independent_source, independent_target, b"independent"),
        EgressEnqueueOutcome::Enqueued
    );
    assert_eq!(
        independent_consumer.try_peek().unwrap().frame(),
        b"independent"
    );
    assert_eq!(
        egress.enqueue_from_ingress(independent_source, target, b"second deferred"),
        EgressEnqueueOutcome::Deferred
    );
    assert!(egress.blocks_source(independent_source));

    assert_eq!(target_consumer.try_peek().unwrap().frame(), b"first");
    releases.arm();
    target_consumer.release();
    tokio::time::timeout(std::time::Duration::from_millis(20), releases.wait())
        .await
        .expect("the manifold wakes for released egress capacity");
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(target_consumer.try_peek().unwrap().frame(), b"deferred");
    assert!(!egress.blocks_source(source));
    assert!(egress.blocks_source(independent_source));
    releases.arm();
    target_consumer.release();
    tokio::time::timeout(std::time::Duration::from_millis(20), releases.wait())
        .await
        .expect("the manifold wakes for the next release");
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(
        target_consumer.try_peek().unwrap().frame(),
        b"second deferred"
    );
    assert!(!egress.blocks_source(independent_source));
}

#[test]
fn removing_a_lane_retires_its_pending_continuation() {
    let source = InterfaceId::new([0x85; 8]);
    let target = InterfaceId::new([0x86; 8]);
    let (producer, _consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(target, producer)]);

    assert_eq!(
        egress.enqueue_from_ingress(source, target, b"first"),
        EgressEnqueueOutcome::Enqueued
    );
    assert_eq!(
        egress.enqueue_from_ingress(source, target, b"deferred"),
        EgressEnqueueOutcome::Deferred
    );
    assert!(egress.has_pending());

    egress.remove_lane(target);

    assert!(!egress.has_pending());
    assert!(!egress.blocks_source(source));
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn egress_metrics_capture_backpressure_and_missing_lanes() {
    let id = InterfaceId::new([0x91; 8]);
    let missing = InterfaceId::new([0x92; 8]);
    let (producer, mut consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(id, producer)]);

    egress.enqueue(id, b"first");
    egress.enqueue(id, b"full");
    egress.enqueue(missing, b"missing");

    assert_eq!(
        egress.metrics_snapshot(&[], InstantMillis(0)),
        EgressMetricsSnapshot {
            enqueued_frames: 1,
            backpressured_frames: 1,
            pending_frames: 1,
            maximum_pending_frames: 1,
            missing_lane_drops: 1,
            announces: crate::runtime::AnnounceEgressMetricsSnapshot {
                interfaces: std::vec![crate::runtime::InterfaceAnnounceEgressMetricsSnapshot {
                    interface: id,
                    outcomes: Default::default(),
                    backpressure: Default::default(),
                    enqueued_bytes_by_origin: Default::default(),
                    pacer_queue_depth: 0,
                    pacer_deferred_depth: 0,
                    pacer_oldest_deferred_age_ms: 0,
                },],
                ..Default::default()
            },
            lanes: std::vec![EgressLaneMetricsSnapshot {
                physical_interface: id,
                logical_interface: id,
                capacity: 1,
                occupancy: 1,
                pending: 1,
            }],
            ..Default::default()
        }
    );
    assert_eq!(consumer.try_peek().unwrap().frame(), b"first");
    consumer.release();
    assert_eq!(egress.flush_pending(1), 1);
    assert_eq!(consumer.try_peek().unwrap().frame(), b"full");
    let snapshot = egress.metrics_snapshot(&[], InstantMillis(0));
    assert_eq!(snapshot.enqueued_frames, 2);
    assert_eq!(snapshot.pending_frames, 0);
    assert_eq!(snapshot.maximum_pending_frames, 1);
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn egress_metrics_distinguish_ifac_rejection_from_successful_masking() {
    let id = InterfaceId::new([0x93; 8]);
    let (producer, mut consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(id, producer)]);
    let ifacs = [InterfaceIfac {
        id,
        context: IfacContext::derive(Some("metrics"), Some("ifac"), IfacSize::NARROW).unwrap(),
    }];
    let clean = [0u8; 3];
    let mut masked = [0u8; 64];

    enqueue_for_wire(
        &mut egress,
        &ifacs,
        id,
        &clean,
        &mut masked,
        EgressOrigin::Internal,
    );
    enqueue_for_wire(
        &mut egress,
        &ifacs,
        id,
        &clean,
        &mut masked[..clean.len()],
        EgressOrigin::Internal,
    );

    assert_eq!(consumer.try_peek().unwrap().frame().len(), 11);
    assert_eq!(
        egress.metrics_snapshot(&[], InstantMillis(0)),
        EgressMetricsSnapshot {
            enqueued_frames: 1,
            ifac_rejected_frames: 1,
            announces: crate::runtime::AnnounceEgressMetricsSnapshot {
                interfaces: std::vec![crate::runtime::InterfaceAnnounceEgressMetricsSnapshot {
                    interface: id,
                    outcomes: Default::default(),
                    backpressure: Default::default(),
                    enqueued_bytes_by_origin: Default::default(),
                    pacer_queue_depth: 0,
                    pacer_deferred_depth: 0,
                    pacer_oldest_deferred_age_ms: 0,
                }],
                ..Default::default()
            },
            lanes: std::vec![EgressLaneMetricsSnapshot {
                physical_interface: id,
                logical_interface: id,
                capacity: 1,
                occupancy: 1,
                pending: 0,
            }],
            ..Default::default()
        }
    );
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn announce_egress_metrics_preserve_origin_outcome_kind_and_bytes() {
    let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"announce-egress");
    let missing = InterfaceId::from_channel_tag(InterfaceKind::Udp, b"missing-egress");
    let (producer, _consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(id, producer)]);
    let mut masked = [0u8; 64];

    enqueue_pacerless_announce_for_wire(
        &mut egress,
        &[],
        id,
        b"accepted",
        &mut masked,
        AnnounceOrigin::Local,
    );
    enqueue_pacerless_announce_for_wire(
        &mut egress,
        &[],
        id,
        b"full",
        &mut masked,
        AnnounceOrigin::Relay,
    );
    enqueue_announce_for_wire(
        &mut egress,
        &[],
        missing,
        b"missing",
        &mut masked,
        AnnounceOrigin::SharedClient,
    );

    let announces = egress.metrics_snapshot(&[], InstantMillis(0)).announces;
    assert_eq!(
        announces
            .outcomes
            .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
        1
    );
    assert_eq!(
        announces
            .outcomes
            .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::LaneFull),
        1
    );
    assert_eq!(
        announces.outcomes.get(
            AnnounceOrigin::SharedClient,
            AnnounceEgressOutcome::LaneMissing
        ),
        1
    );
    assert_eq!(
        announces
            .enqueued_by_interface_kind
            .get(InterfaceKind::TcpClient),
        1
    );
    assert_eq!(
        announces
            .enqueued_bytes_by_origin
            .get(AnnounceOrigin::Local),
        b"accepted".len() as u64
    );
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn announce_egress_metrics_roll_fleet_members_into_their_logical_interface() {
    let logical = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, b"server");
    let first = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"first");
    let second = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"second");
    let (first_producer, _first_consumer) = tokio_grant_lane(64, 1);
    let (second_producer, _second_consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![]);
    egress.add_lane(first, logical, first_producer, None);
    egress.add_lane(second, logical, second_producer, None);
    let mut masked = [0u8; 64];

    enqueue_announce_for_wire(
        &mut egress,
        &[],
        first,
        b"first",
        &mut masked,
        AnnounceOrigin::Relay,
    );
    enqueue_announce_for_wire(
        &mut egress,
        &[],
        second,
        b"second",
        &mut masked,
        AnnounceOrigin::Relay,
    );

    let announces = egress.metrics_snapshot(&[], InstantMillis(0)).announces;
    assert_eq!(announces.interfaces.len(), 1);
    assert_eq!(announces.interfaces[0].interface, logical);
    assert_eq!(
        announces.interfaces[0]
            .outcomes
            .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
        2
    );
    assert_eq!(
        announces
            .enqueued_by_interface_kind
            .get(InterfaceKind::TcpServer),
        2
    );
}

#[test]
fn the_pacer_wiring_holds_then_releases_a_capped_burst() {
    let id = InterfaceId::new([0x5a; 8]);
    let mut pacers = std::vec![InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(
            AnnounceBandwidthCap::RNS_DEFAULT,
            BitrateBps::guess(5_000),
            TOKIO_ANNOUNCE_RETRY_POLICY,
        ),
    }];
    let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let mut egress = Egress::new(std::vec![(id, tx)]);

    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[1; 10],
            hops: 1,
            #[cfg(feature = "runtime-metrics")]
            origin: AnnounceOrigin::Local,
        },
        InstantMillis(1_000),
        &mut egress,
        &[],
    );
    assert_eq!(rx.try_peek().unwrap().frame(), [1u8; 10].as_slice());
    rx.release();

    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[2; 10],
            hops: 1,
            #[cfg(feature = "runtime-metrics")]
            origin: AnnounceOrigin::Relay,
        },
        InstantMillis(1_200),
        &mut egress,
        &[],
    );
    assert!(rx.try_peek().is_none(), "the second is held, not sent");
    assert_eq!(soonest_pacer_release(&pacers), Some(InstantMillis(1_800)));
    #[cfg(feature = "runtime-metrics")]
    {
        let announces = egress
            .metrics_snapshot(&pacers, InstantMillis(1_200))
            .announces;
        assert_eq!(announces.pacer_queue_depth, 1);
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
            1
        );
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
            0
        );
    }

    flush_due_pacers(&mut pacers, InstantMillis(1_799), &mut egress, &[]);
    assert!(
        rx.try_peek().is_none(),
        "nothing releases before the window"
    );

    flush_due_pacers(&mut pacers, InstantMillis(1_800), &mut egress, &[]);
    assert_eq!(rx.try_peek().unwrap().frame(), [2u8; 10].as_slice());
    rx.release();
    assert_eq!(soonest_pacer_release(&pacers), None);
    #[cfg(feature = "runtime-metrics")]
    {
        let announces = egress
            .metrics_snapshot(&pacers, InstantMillis(1_800))
            .announces;
        assert_eq!(announces.pacer_queue_depth, 0);
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
            1
        );
    }
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn a_full_lane_defers_in_place_then_recovers_with_truthful_metrics() {
    let id = InterfaceId::new([0x5d; 8]);
    let mut pacers = std::vec![InterfacePacer {
        id,
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(
            AnnounceBandwidthCap::RNS_DEFAULT,
            BitrateBps::guess(5_000),
            TOKIO_ANNOUNCE_RETRY_POLICY,
        ),
    }];
    let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 1);
    let mut egress = Egress::new(std::vec![(id, tx)]);
    assert_eq!(
        egress.enqueue(id, b"occupy"),
        EgressEnqueueOutcome::Enqueued
    );

    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[0xa5; 10],
            hops: 1,
            origin: AnnounceOrigin::Relay,
        },
        InstantMillis(1_000),
        &mut egress,
        &[],
    );
    let pressure = egress.metrics_snapshot(&pacers, InstantMillis(1_025));
    assert_eq!(pressure.full_lane_drops, 0);
    assert_eq!(pressure.announces.pacer_queue_depth, 1);
    assert_eq!(pressure.announces.pacer_deferred_depth, 1);
    assert_eq!(pressure.announces.pacer_oldest_deferred_age_ms, 25);
    assert_eq!(pressure.lanes[0].capacity, 1);
    assert_eq!(pressure.lanes[0].occupancy, 1);
    assert_eq!(
        pressure
            .announces
            .backpressure
            .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Deferred),
        1
    );

    flush_due_pacers(&mut pacers, InstantMillis(1_050), &mut egress, &[]);
    let retried = egress.metrics_snapshot(&pacers, InstantMillis(1_050));
    assert_eq!(retried.full_lane_drops, 0);
    assert_eq!(
        retried
            .announces
            .backpressure
            .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Retry),
        1
    );

    assert_eq!(rx.try_peek().expect("occupied slot").frame(), b"occupy");
    rx.release();
    flush_due_pacers(&mut pacers, InstantMillis(1_150), &mut egress, &[]);

    let recovered = egress.metrics_snapshot(&pacers, InstantMillis(1_150));
    assert_eq!(recovered.full_lane_drops, 0);
    assert_eq!(recovered.announces.pacer_queue_depth, 0);
    assert_eq!(recovered.announces.pacer_deferred_depth, 0);
    assert_eq!(
        recovered
            .announces
            .backpressure
            .get(AnnounceOrigin::Relay, AnnounceBackpressureEvent::Recovered),
        1
    );
    assert_eq!(
        recovered
            .announces
            .outcomes
            .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
        1
    );
    assert_eq!(
        rx.try_peek().expect("recovered announce").frame(),
        &[0xa5; 10]
    );
}

#[test]
fn clearing_announce_queues_counts_every_pacer_entry() {
    let first = InterfaceId::new([0x5B; 8]);
    let second = InterfaceId::new([0x5C; 8]);
    let mut pacers = [first, second].map(|id| InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(
            AnnounceBandwidthCap::RNS_DEFAULT,
            BitrateBps::guess(5_000),
            TOKIO_ANNOUNCE_RETRY_POLICY,
        ),
    });
    let (first_tx, _first_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (second_tx, _second_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let mut egress = Egress::new(std::vec![(first, first_tx), (second, second_tx)]);

    for (target, tag) in [(first, 1), (first, 2), (first, 3), (second, 4), (second, 5)] {
        let bytes = [tag; 10];
        offer_to_pacer(
            &mut pacers,
            target,
            PacedAnnounce {
                bytes: &bytes,
                hops: 1,
                #[cfg(feature = "runtime-metrics")]
                origin: AnnounceOrigin::Relay,
            },
            InstantMillis(1_000),
            &mut egress,
            &[],
        );
    }

    assert_eq!(clear_announce_queues(&mut pacers), 3);
    assert_eq!(soonest_pacer_release(&pacers), None);
}

#[test]
fn an_unavailable_interface_never_enters_the_pacer_or_lane() {
    let id = InterfaceId::new([0x6a; 8]);
    let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Disconnected);
    let mut pacers = std::vec![InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(
            AnnounceBandwidthCap::RNS_DEFAULT,
            BitrateBps::guess(5_000),
            TOKIO_ANNOUNCE_RETRY_POLICY,
        ),
    }];
    let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let mut egress = Egress::new(std::vec![]);
    egress.add_lane(id, id, tx, Some(ConnectionView::of(status.clone())));

    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[1; 10],
            hops: 1,
            #[cfg(feature = "runtime-metrics")]
            origin: AnnounceOrigin::Relay,
        },
        InstantMillis(1_000),
        &mut egress,
        &[],
    );

    assert!(rx.try_peek().is_none());
    assert_eq!(soonest_pacer_release(&pacers), None);

    status.set_connection(ConnectionState::Connected);
    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[2; 10],
            hops: 1,
            #[cfg(feature = "runtime-metrics")]
            origin: AnnounceOrigin::Relay,
        },
        InstantMillis(1_100),
        &mut egress,
        &[],
    );
    assert_eq!(rx.try_peek().unwrap().frame(), [2u8; 10].as_slice());
    rx.release();

    offer_to_pacer(
        &mut pacers,
        id,
        PacedAnnounce {
            bytes: &[3; 10],
            hops: 1,
            #[cfg(feature = "runtime-metrics")]
            origin: AnnounceOrigin::Relay,
        },
        InstantMillis(1_200),
        &mut egress,
        &[],
    );
    status.set_connection(ConnectionState::Disconnected);
    flush_due_pacers(&mut pacers, InstantMillis(10_000), &mut egress, &[]);
    assert!(rx.try_peek().is_none());
    assert_eq!(soonest_pacer_release(&pacers), None);

    #[cfg(feature = "runtime-metrics")]
    {
        let snapshot = egress.metrics_snapshot(&pacers, InstantMillis(10_000));
        assert_eq!(snapshot.unavailable_frame_skips, 2);
        assert_eq!(snapshot.announces.pacer_queue_depth, 0);
        assert_eq!(
            snapshot.announces.outcomes.get(
                AnnounceOrigin::Relay,
                AnnounceEgressOutcome::InterfaceUnavailable
            ),
            2
        );
        assert_eq!(
            snapshot
                .announces
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
            1
        );
    }
}

#[test]
fn online_only_directives_skip_disconnected_interfaces() {
    let id = InterfaceId::new([0x6b; 8]);
    let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Disconnected);
    let (tx, mut rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let mut egress = Egress::new(std::vec![]);
    egress.add_lane(id, id, tx, Some(ConnectionView::of(status.clone())));
    let mut pacers = std::vec::Vec::new();
    let mut scratch = WireScratch::new(MAX_WIRE_FRAME_LEN);
    let mut sent = 0;

    {
        let mut on_send = || sent += 1;
        let mut directive_egress = TokioDirectiveEgress {
            egress: &mut egress,
            ifacs: &[],
            pacers: &mut pacers,
            scratch: &mut scratch,
            now: InstantMillis(1_000),
            origin: EgressOrigin::Internal,
        };
        directive_egress.send_if_online(id, b"disconnected", &mut on_send);
    }
    assert!(rx.try_peek().is_none());
    assert_eq!(sent, 0);

    status.set_connection(ConnectionState::Connected);
    {
        let mut on_send = || sent += 1;
        let mut directive_egress = TokioDirectiveEgress {
            egress: &mut egress,
            ifacs: &[],
            pacers: &mut pacers,
            scratch: &mut scratch,
            now: InstantMillis(1_100),
            origin: EgressOrigin::Internal,
        };
        directive_egress.send_if_online(id, b"connected", &mut on_send);
    }
    assert_eq!(rx.try_peek().unwrap().frame(), b"connected");
    assert_eq!(sent, 1);
}
