use super::*;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
#[cfg(feature = "runtime-metrics")]
use tokio::sync::oneshot;

use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_3_5_ANNOUNCE,
    RNS_1_3_5_RATCHETED_ANNOUNCE, TEST_TRANSPORT_ID,
};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{Departure, RouteRemovalCause};
#[cfg(feature = "runtime-metrics")]
use crate::interfaces::InterfaceKind;
use crate::interfaces::{
    AirtimeUtilization, AnnounceBandwidthCap, BitrateBps, ConnectionState, ConnectionView,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceMode, InterfaceStatus,
    RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb, TransportCapability,
};
use crate::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{AnnounceEgressOutcome, EgressMetricsSnapshot};
use crate::runtime::{PrnsNodeHandle, RoutingControl};
use crate::wire::{PacketType, WirePacketHeader};

#[tokio::test(start_paused = true)]
async fn logical_time_saturates_at_the_numeric_limit() {
    let host = TokioHost::start_at(InstantMillis(u64::MAX - 5));
    tokio::time::advance(Duration::from_millis(10)).await;
    assert_eq!(host.now(), InstantMillis(u64::MAX));
}

#[tokio::test(start_paused = true)]
async fn a_far_future_sleep_arms_without_overflowing_the_timer() {
    let host = TokioHost::new();
    let sleeping = host.sleep_until(InstantMillis(u64::MAX));
    tokio::pin!(sleeping);
    tokio::select! {
        () = &mut sleeping => panic!("the numeric limit is not immediately due"),
        () = tokio::time::sleep(Duration::from_millis(1)) => {}
    }
}

#[test]
fn packet_phy_reuses_the_classified_wire_stable_packet_hash() {
    let store = InterfaceStore::new();
    let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let expected = PacketHash::of_wire_packet(&raw).expect("the fixture is a wire packet");
    let packet = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: InterfaceId::new([0xC7; 8]),
        bytes: &mut raw,
    });
    let packet_hash = packet.packet_hash().expect("the packet was classified");
    let packet_phy = PacketPhyStats {
        rssi: Some(RssiDbm::new(-103)),
        snr: Some(SnrQuarterDb::new(-11)),
        quality: SignalQualityTenthsPercent::new(731),
    };

    retain_packet_phy(Some(&store), packet_hash, packet_phy);

    assert_eq!(packet_hash, expected);
    assert_eq!(store.packet_phy(packet_hash), Some(packet_phy));
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn egress_metrics_distinguish_enqueued_full_and_missing_lanes() {
    let id = InterfaceId::new([0x91; 8]);
    let missing = InterfaceId::new([0x92; 8]);
    let (producer, _consumer) = tokio_grant_lane(64, 1);
    let mut egress = Egress::new(std::vec![(id, producer)]);

    egress.enqueue(id, b"first");
    egress.enqueue(id, b"full");
    egress.enqueue(missing, b"missing");

    assert_eq!(
        egress.metrics_snapshot(&[]),
        EgressMetricsSnapshot {
            enqueued_frames: 1,
            full_lane_drops: 1,
            missing_lane_drops: 1,
            announces: crate::runtime::AnnounceEgressMetricsSnapshot {
                interfaces: std::vec![crate::runtime::InterfaceAnnounceEgressMetricsSnapshot {
                    interface: id,
                    outcomes: Default::default(),
                    enqueued_bytes_by_origin: Default::default(),
                    pacer_queue_depth: 0,
                },],
                ..Default::default()
            },
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

    enqueue_announce_for_wire(
        &mut egress,
        &[],
        id,
        b"accepted",
        &mut masked,
        AnnounceOrigin::Local,
    );
    enqueue_announce_for_wire(
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

    let announces = egress.metrics_snapshot(&[]).announces;
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

    let announces = egress.metrics_snapshot(&[]).announces;
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

#[tokio::test]
async fn the_seam_signals_a_synthesize_request_carrying_its_interface_id() {
    let id = InterfaceId::new([0xC7; 8]);
    let (in_producer, _in_consumer) = tokio_grant_lane(64, 2);
    let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
    let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<HostCommand>();
    let mut seam =
        TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer).with_commands(cmd_tx);

    seam.request_tunnel_synthesis().await;

    let got = cmd_rx
        .try_recv()
        .expect("a synthesize request reached the reactor");
    assert!(matches!(got, HostCommand::SynthesizeTunnel { interface } if interface == id));
}

#[tokio::test]
async fn a_seam_without_a_command_channel_drops_the_synthesize_request() {
    let id = InterfaceId::new([0xC8; 8]);
    let (in_producer, _in_consumer) = tokio_grant_lane(64, 2);
    let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
    let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
    let mut seam = TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer);

    seam.request_tunnel_synthesis().await;
}

#[tokio::test]
async fn packet_phy_crosses_the_tokio_ingress_seam_with_its_frame() {
    let id = InterfaceId::new([0xC9; 8]);
    let (in_producer, mut in_consumer) = tokio_grant_lane(64, 2);
    let (_out_producer, out_consumer) = tokio_grant_lane(64, 2);
    let (notify_tx, _notify_rx) = mpsc::unbounded_channel();
    let mut seam = TokioInterfaceSeam::new(id, in_producer, notify_tx, out_consumer);
    let packet_phy = PacketPhyStats {
        rssi: Some(RssiDbm::new(-91)),
        snr: Some(SnrQuarterDb::new(-7)),
        quality: SignalQualityTenthsPercent::new(812),
    };

    seam.next_inbound_with_phy(b"observed", packet_phy).await;

    let retained = in_consumer.try_peek().expect("the frame crossed the seam");
    assert_eq!(
        (retained.frame(), retained.packet_phy),
        (b"observed".as_slice(), packet_phy)
    );
}

use tokio::sync::mpsc;

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

#[test]
fn airtime_reads_none_until_published_then_round_trips() {
    let status =
        TokioInterfaceStatus::new(InterfaceId::new([0x5A; 8]), ConnectionState::Initializing);
    assert_eq!(status.airtime(), None);

    status.set_airtime(AirtimeUtilization {
        short_per_mille: 137,
        long_per_mille: 4,
    });
    assert_eq!(
        status.airtime(),
        Some(AirtimeUtilization {
            short_per_mille: 137,
            long_per_mille: 4,
        }),
    );
}

#[test]
fn the_pacer_wiring_holds_then_releases_a_capped_burst() {
    let id = InterfaceId::new([0x5a; 8]);
    let mut pacers = std::vec![InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(AnnounceBandwidthCap::RNS_DEFAULT, BitrateBps::guess(5_000),),
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
        let announces = egress.metrics_snapshot(&pacers).announces;
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
        let announces = egress.metrics_snapshot(&pacers).announces;
        assert_eq!(announces.pacer_queue_depth, 0);
        assert_eq!(
            announces
                .outcomes
                .get(AnnounceOrigin::Relay, AnnounceEgressOutcome::Enqueued),
            1
        );
    }
}

#[test]
fn clearing_announce_queues_counts_every_pacer_entry() {
    let first = InterfaceId::new([0x5B; 8]);
    let second = InterfaceId::new([0x5C; 8]);
    let mut pacers = [first, second].map(|id| InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(AnnounceBandwidthCap::RNS_DEFAULT, BitrateBps::guess(5_000)),
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
    let status = TokioInterfaceStatus::new(id, ConnectionState::Disconnected);
    let mut pacers = std::vec![InterfacePacer {
        id,
        #[cfg(feature = "runtime-metrics")]
        logical_interface: id,
        pacer: TokioAnnouncePacer::new(AnnounceBandwidthCap::RNS_DEFAULT, BitrateBps::guess(5_000),),
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
        let snapshot = egress.metrics_snapshot(&pacers);
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

struct LoopbackInterface {
    descriptor: InterfaceDescriptor,
    wire_in: UnboundedReceiver<std::vec::Vec<u8>>,
    wire_out: UnboundedSender<std::vec::Vec<u8>>,
}

impl Interface for LoopbackInterface {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        loop {
            tokio::select! {
                received = self.wire_in.recv() => {
                    match received {
                        Some(bytes) => seam.next_inbound(&bytes).await,
                        None => return,
                    }
                }
                outbound = seam.next_outbound() => {
                    let _ = self.wire_out.send(outbound.to_vec());
                }
            }
        }
    }
}

#[tokio::test]
async fn a_filled_grant_is_read_in_place_without_a_copy() {
    let (mut producer, mut consumer) = tokio_grant_lane(512, 2);

    let granted = producer.grant().await;
    granted.fill(b"the frame is written once");
    let written_at = granted.bytes.as_ptr() as usize;
    producer.commit();

    let received = consumer.peek().await;
    assert_eq!(received.frame(), b"the frame is written once");
    assert_eq!(
        received.bytes.as_ptr() as usize,
        written_at,
        "the consumer reads the very slot the producer filled",
    );
    received.frame_mut()[0] ^= 0x20;
    assert_eq!(&received.frame()[..3], b"The");
    consumer.release();
}

#[test]
fn a_burst_earns_one_announcement_until_the_consumer_acknowledges() {
    let (mut producer, mut consumer) = tokio_grant_lane(64, 8);

    producer.try_grant().expect("lane grants").fill(b"one");
    producer.commit();
    assert!(producer.needs_announce(), "the first commit announces");

    producer.try_grant().expect("lane grants").fill(b"two");
    producer.commit();
    assert!(
        !producer.needs_announce(),
        "a burst behind an unconsumed announcement stays silent",
    );

    consumer.acknowledge();
    while consumer.try_peek().is_some() {
        consumer.release();
    }

    producer.try_grant().expect("lane grants").fill(b"three");
    producer.commit();
    assert!(
        producer.needs_announce(),
        "a commit after the acknowledge announces again",
    );
}

#[tokio::test]
async fn a_full_lane_refuses_grants_until_the_consumer_releases() {
    let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

    producer
        .try_grant()
        .expect("an empty lane grants")
        .fill(b"one");
    producer.commit();
    assert!(producer.try_grant().is_none(), "a depth-one lane is full");

    consumer.try_peek().expect("the committed frame is there");
    consumer.release();
    assert!(
        producer.try_grant().is_some(),
        "the release frees the slot for the next grant",
    );
}

#[tokio::test]
async fn a_loopback_frame_crosses_the_seam_and_the_rebroadcast_leaves_through_the_peer() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<()>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            let _ = heard_tx.send(());
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::CommandSettled { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        heard_rx.try_recv().is_err(),
        "an idle reactor journals nothing"
    );
    assert!(
        peer_wire_out_rx.try_recv().is_err(),
        "an idle interface transmits nothing"
    );

    let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let original_hops = WirePacketHeader::parse(&raw)
        .expect("valid announce wire")
        .0
        .hops;
    source_wire_in_tx
        .send(raw)
        .expect("the source interface holds its wire");

    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("the deposited frame journals within the window")
        .expect("the reactor task is alive");

    let bytes = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the rebroadcast reaches the peer's wire within the window")
        .expect("the peer interface task is alive");
    let (header, _) = WirePacketHeader::parse(&bytes).expect("valid rebroadcast wire");
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(
        header.hops,
        original_hops + 1,
        "the rebroadcast bumps the hop count"
    );
}

#[tokio::test(start_paused = true)]
async fn a_capped_link_holds_a_rebroadcast_burst_then_drains_it_over_time() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let slow_peer = InterfaceDescriptor {
        bitrate: BitrateBps::guess(1_000),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        ..descriptor(peer)
    };
    let interfaces = std::vec![descriptor(source), slow_peer];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        |_journaled: Journaled<'_>| {},
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
        .expect("the source interface holds its wire");
    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE))
        .expect("the source interface holds its wire");

    let first = tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
        .await
        .expect("the first rebroadcast leaves the idle link within the window")
        .expect("the peer task is alive");
    assert_eq!(
        WirePacketHeader::parse(&first).unwrap().0.packet_type,
        PacketType::Announce
    );

    assert!(
        tokio::time::timeout(Duration::from_secs(5), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "the cap holds the second rebroadcast far short of its spacing window",
    );

    let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
        .await
        .expect("the held rebroadcast drains once the spacing window passes")
        .expect("the peer task is alive");
    assert_eq!(
        WirePacketHeader::parse(&second).unwrap().0.packet_type,
        PacketType::Announce
    );
    assert_ne!(first, second, "the two rebroadcasts are distinct announces");
}

#[tokio::test(start_paused = true)]
async fn the_reactor_re_emits_a_rebroadcast_once_more_then_retires_it() {
    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);

    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);

    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        |_journaled: Journaled<'_>| {},
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    source_wire_in_tx
        .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
        .expect("the source interface holds its wire");

    let first = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the first emission leaves within the jitter window")
        .expect("the peer task is alive");

    assert!(
        tokio::time::timeout(Duration::from_secs(4), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "the second emission waits the full retransmit interval",
    );

    let second = tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
        .await
        .expect("the reactor re-emits once the retransmit interval passes")
        .expect("the peer task is alive");
    assert_eq!(
        first, second,
        "the retransmit re-emits the same pinned announce, byte for byte",
    );

    assert!(
        tokio::time::timeout(Duration::from_secs(120), peer_wire_out_rx.recv())
            .await
            .is_err(),
        "after two emissions the reactor retires the entry",
    );
}

#[tokio::test]
async fn a_delivery_answers_with_a_proof_directive_on_the_arrival_lane() {
    use crate::crypto::X25519SecretKey;
    use crate::engine::RatchetPolicy;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::{IdentitySigner, RemoteIdentity, Zeroizing};
    use crate::routing::dedup::PacketHash;
    use crate::routing::proof::IMPLICIT_PROOF_WIRE_LEN;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
    use crate::wire::{
        ContextFlag, DestinationType, IfacFlag, PropagationType, WireContext, BROADCAST_MTU,
    };

    let mut secret = [0u8; 64];
    secret[..32].fill(0x22);
    secret[32..].fill(0x11);
    let secret = Zeroizing::new(secret);

    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
    let mut engine = EngineState::<TestStorageLayout>::new(secret);
    let destination = engine
        .register_single_destination(
            &identity.identity_hash(),
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the single destination");

    let remote = RemoteIdentity::from_public_keys(
        identity.encryption_public_key(),
        identity.signing_public_key(),
    );
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: destination.to_address(),
        context: WireContext::None,
    };
    let mut wire = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut wire).expect("writes the header");
    let sealed = remote
        .encrypt(
            &X25519SecretKey::new([0x77; 32]),
            &[0x88; 16],
            b"prove-through-the-stack",
            &mut wire[header_len..],
        )
        .expect("seals the payload");
    let raw = wire[..header_len + sealed].to_vec();
    let packet_hash = PacketHash::of_wire_packet(&raw).expect("hashes the wire packet");

    let mut expected_proof = std::vec::Vec::new();
    expected_proof.push(0x03);
    expected_proof.push(0x00);
    expected_proof.extend_from_slice(packet_hash.proof_destination().as_bytes());
    expected_proof.push(0x00);
    expected_proof.extend_from_slice(&identity.sign(packet_hash.as_bytes()).0);
    assert_eq!(expected_proof.len(), IMPLICIT_PROOF_WIRE_LEN);

    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = std::vec![descriptor(source)];

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (mut source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (source_out_tx, mut source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let egress = Egress::new(std::vec![(source, source_out_tx)]);

    let (delivered_tx, mut delivered_rx) = mpsc::unbounded_channel::<()>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::Delivered(_) => {
            let _ = delivered_tx.send(());
        }
        Journaled::AnnounceHeard { .. }
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::CommandSettled { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));

    source_in_tx
        .try_grant()
        .expect("an empty lane grants")
        .fill(&raw);
    source_in_tx.commit();
    notify_tx
        .send(source)
        .expect("the reactor task holds the receiver");

    tokio::time::timeout(Duration::from_secs(2), delivered_rx.recv())
        .await
        .expect("the delivery journals within the window")
        .expect("the reactor task is alive");

    let frame = tokio::time::timeout(Duration::from_secs(2), source_out_rx.peek())
        .await
        .expect("the owed proof is emitted within the window");
    assert_eq!(
        frame.frame(),
        expected_proof,
        "the proof is byte-identical to the RNS 1.3.5 implicit proof, on the arrival lane"
    );
}

#[tokio::test]
async fn ifac_members_hear_each_other_and_strangers_stay_outside() {
    use crate::interfaces::ifac::IfacContext;
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(source), descriptor(peer)];
    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let network = || {
        IfacContext::derive(
            Some("testnet"),
            Some("s3cret"),
            crate::interfaces::ifac::IfacSize::NARROW,
        )
        .unwrap()
    };
    let ifacs = std::vec![
        InterfaceIfac {
            id: source,
            context: network(),
        },
        InterfaceIfac {
            id: peer,
            context: network(),
        },
    ];

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (peer_in_tx, peer_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (source_wire_in_tx, source_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_wire_out_tx, _source_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (source_out_tx, source_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let source_iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: source_wire_in_rx,
        wire_out: source_wire_out_tx,
    };
    let source_seam =
        TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), source_out_rx);

    let (_peer_wire_in_tx, peer_wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_wire_out_tx, mut peer_wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (peer_out_tx, peer_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let peer_iface = LoopbackInterface {
        descriptor: descriptor(peer),
        wire_in: peer_wire_in_rx,
        wire_out: peer_wire_out_tx,
    };
    let peer_seam = TokioInterfaceSeam::new(peer, peer_in_tx, notify_tx.clone(), peer_out_rx);
    drop(notify_tx);

    let egress = Egress::new(std::vec![(source, source_out_tx), (peer, peer_out_tx)]);
    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| {
        if let Journaled::AnnounceHeard { observation, .. } = journaled {
            let _ = heard_tx.send(observation.destination);
        }
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs,
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx), (peer, peer_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(source_iface.run(source_seam));
    tokio::spawn(peer_iface.run(peer_seam));

    let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut member_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let masked_len = network().mask_outbound(&clean, &mut member_wire).unwrap();
    source_wire_in_tx
        .send(member_wire[..masked_len].to_vec())
        .expect("the source interface holds its wire");

    let heard = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("a member's masked announce is heard")
        .expect("the reactor task is alive");
    assert_eq!(
        heard.as_bytes(),
        bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314").as_slice(),
    );

    let rebroadcast = tokio::time::timeout(Duration::from_secs(2), peer_wire_out_rx.recv())
        .await
        .expect("the rebroadcast leaves through the peer")
        .expect("the peer task is alive");
    assert_eq!(
        rebroadcast[0] & 0x80,
        0x80,
        "the peer's wire only ever carries flagged, masked frames",
    );
    let mut recovered = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let clean_len = network()
        .unmask_inbound(&rebroadcast, &mut recovered)
        .expect("a member can open the rebroadcast");
    let (header, _) = WirePacketHeader::parse(&recovered[..clean_len]).unwrap();
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(
        header.hops, 1,
        "the relay bumped the hop count under the mask"
    );

    let stranger = IfacContext::derive(
        Some("testnet"),
        Some("wrong"),
        crate::interfaces::ifac::IfacSize::NARROW,
    )
    .unwrap();
    let mut stranger_wire = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let stranger_len = stranger
        .mask_outbound(
            &bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE),
            &mut stranger_wire,
        )
        .unwrap();
    source_wire_in_tx
        .send(stranger_wire[..stranger_len].to_vec())
        .expect("the source interface holds its wire");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), heard_rx.recv())
            .await
            .is_err(),
        "a stranger's code opens nothing",
    );
}

#[tokio::test]
async fn dynamic_ifac_state_arrives_and_leaves_with_its_interface() {
    use crate::interfaces::ifac::{IfacContext, IfacSize};
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xD4; 8]);
    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);
    let network = IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| {
        if let Journaled::AnnounceHeard { observation, .. } = journaled {
            let _ = heard_tx.send(observation.destination);
        }
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![],
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![],
            commands: command_rx,
            egress: Egress::new(std::vec![]),
        },
        app,
    ));

    let (mut protected_in, protected_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (protected_out, _protected_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    command_tx
        .send(HostCommand::AddInterface(AddInterfaceCommand {
            descriptor: descriptor(source),
            logical_interface: source,
            inbound: protected_rx,
            egress: protected_out,
            connection: None,
            ifac: Some(network.clone()),
        }))
        .unwrap();
    tokio::task::yield_now().await;

    let clean = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut masked = std::vec![0u8; MAX_WIRE_FRAME_LEN];
    let masked_len = network.mask_outbound(&clean, &mut masked).unwrap();
    protected_in
        .try_grant()
        .unwrap()
        .fill(&masked[..masked_len]);
    protected_in.commit();
    notify_tx.send(source).unwrap();
    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();

    command_tx
        .send(HostCommand::RemoveInterface {
            id: source,
            departure: Departure::MayReturn,
        })
        .unwrap();
    let (mut open_in, open_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (open_out, _open_wire) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    command_tx
        .send(HostCommand::AddInterface(AddInterfaceCommand {
            descriptor: descriptor(source),
            logical_interface: source,
            inbound: open_rx,
            egress: open_out,
            connection: None,
            ifac: None,
        }))
        .unwrap();
    tokio::task::yield_now().await;

    let open = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
    open_in.try_grant().unwrap().fill(&open);
    open_in.commit();
    notify_tx.send(source).unwrap();
    tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn routing_control_drops_a_live_route_and_journals_the_explicit_removal() {
    let source = InterfaceId::new([0xD5; 8]);
    let engine = EngineState::<TestStorageLayout>::default();
    let store = InterfaceStore::new();
    let mut store_changes = store.subscribe();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (mut inbound_tx, inbound_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let handle = PrnsNodeHandle::over(command_tx);
    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (dropped_tx, mut dropped_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            let _ = heard_tx.send(observation.destination);
        }
        Journaled::RouteRemoved {
            destination,
            cause: RouteRemovalCause::Dropped,
        } => {
            let _ = dropped_tx.send(destination);
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::CommandSettled { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. } => {}
    };

    tokio::spawn(run_with_store(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![descriptor(source)],
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, inbound_rx)],
            commands: command_rx,
            egress: Egress::new(std::vec![]),
        },
        app,
        store.clone(),
        CryptoPoolConfig::Inline,
    ));

    let announce = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    inbound_tx.try_grant().unwrap().fill(&announce);
    inbound_tx.commit();
    notify_tx.send(source).unwrap();
    let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), store_changes.changed())
        .await
        .unwrap();
    assert_eq!(store.counts(source).destinations, 1);

    assert_eq!(
        handle.drop_route(destination).await,
        Ok(DropRouteOutcome::Dropped)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), dropped_rx.recv())
            .await
            .unwrap(),
        Some(destination)
    );
    tokio::time::timeout(Duration::from_secs(2), store_changes.changed())
        .await
        .unwrap();
    assert_eq!(store.counts(source).destinations, 0);
    assert_eq!(
        handle.drop_route(destination).await,
        Ok(DropRouteOutcome::NotFound)
    );
    assert!(dropped_rx.try_recv().is_err());

    #[cfg(feature = "runtime-metrics")]
    assert_eq!(
        handle
            .metrics_snapshot()
            .await
            .unwrap()
            .reliability
            .route_removals
            .get(crate::runtime::RuntimeRouteRemoval::Dropped),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn the_reactor_culls_an_expired_route_at_its_deadline() {
    use crate::engine::{
        CommandId, EngineCommand, SendSinglePacket, SendSinglePacketFailure,
        SendSinglePacketPayload, SendSinglePacketRejection, Settlement,
    };
    use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
    use crate::wire::DestinationHash;

    let source = InterfaceId::new([0xA1; 8]);
    let interfaces = std::vec![descriptor(source)];
    let engine = EngineState::<TestStorageLayout>::default();

    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (source_in_tx, source_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (wire_in_tx, wire_in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (wire_out_tx, _wire_out_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let iface = LoopbackInterface {
        descriptor: descriptor(source),
        wire_in: wire_in_rx,
        wire_out: wire_out_tx,
    };
    let seam = TokioInterfaceSeam::new(source, source_in_tx, notify_tx.clone(), out_rx);
    drop(notify_tx);
    let egress = Egress::new(std::vec![(source, out_tx)]);
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();

    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (expired_tx, mut expired_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            let _ = heard_tx.send(observation.destination);
        }
        Journaled::RouteRemoved {
            destination,
            cause: RouteRemovalCause::Expired,
        } => {
            let _ = expired_tx.send(destination);
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = settled_tx.send((id, settlement));
        }
        Journaled::Delivered(_)
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![(source, source_in_rx)],
            commands: command_rx,
            egress,
        },
        app,
    ));
    tokio::spawn(iface.run(seam));

    wire_in_tx
        .send(bytes_from_hex(RNS_1_3_5_ANNOUNCE))
        .expect("the interface holds its wire");
    let destination = tokio::time::timeout(Duration::from_secs(2), heard_rx.recv())
        .await
        .expect("the announce is heard, so the route exists before the deadline")
        .expect("the reactor task is alive");

    tokio::time::sleep(Duration::from_millis(DEFAULT_ROUTE_EXPIRY_MILLIS + 10_000)).await;

    let expired = tokio::time::timeout(Duration::from_secs(2), expired_rx.recv())
        .await
        .expect("the cull journals the removal at the expiry deadline")
        .expect("the reactor task is alive");
    assert_eq!(
        expired, destination,
        "the expired route names its destination"
    );

    command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(3),
            command: EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload: SendSinglePacketPayload::from_slice(b"late").expect("fits the MDU"),
            }),
        }))
        .expect("the reactor task holds the receiver");

    let (settled_id, settlement) = tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
        .await
        .expect("the late send settles")
        .expect("the reactor task is alive");
    assert_eq!(settled_id, CommandId(3));
    assert_eq!(
        settlement,
        Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Rejected(
            SendSinglePacketRejection::NoRouteToDestination
        ))),
        "the reactor woke at the route's expiry and culled it",
    );
}

#[tokio::test]
async fn a_commanded_announce_fans_to_every_interface_and_settles() {
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, RatchetPolicy,
        Settlement,
    };
    use crate::identity::Zeroizing;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

    let mut secret = [0u8; 64];
    secret[..32].fill(0x22);
    secret[32..].fill(0x11);
    let mut engine = EngineState::<TestStorageLayout>::new(Zeroizing::new(secret));
    let node = engine.held_identity_hashes()[0];
    let destination = engine
        .register_single_destination(
            &node,
            "personal",
            &["node"],
            b"",
            ProofStrategy::ProveNone,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the single destination");

    let first = InterfaceId::new([0xA1; 8]);
    let second = InterfaceId::new([0xB2; 8]);
    let interfaces = std::vec![descriptor(first), descriptor(second)];

    let (_notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (first_out_tx, mut first_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (second_out_tx, mut second_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let egress = Egress::new(std::vec![(first, first_out_tx), (second, second_out_tx)]);

    let (settled_tx, mut settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::CommandSettled { id, settlement } => {
            let _ = settled_tx.send((id, settlement));
        }
        Journaled::AnnounceHeard { .. }
        | Journaled::SelfRatchetRotated { .. }
        | Journaled::Delivered(_)
        | Journaled::AnnounceHeldDropped { .. }
        | Journaled::RouteRemoved { .. }
        | Journaled::LinkEstablished(_)
        | Journaled::PeerIdentified { .. }
        | Journaled::RequestReceived { .. }
        | Journaled::ResponseReceived { .. }
        | Journaled::ResponseSegmentReceived { .. }
        | Journaled::ChannelMessageReceived { .. }
        | Journaled::LinkClosed { .. }
        | Journaled::ResourceReceived { .. }
        | Journaled::ResourceFailed { .. }
        | Journaled::ResourceNeedsDecompression { .. }
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. } => {}
    };

    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: std::vec![],
            notify: notify_rx,
            inbound_lanes: std::vec![],
            commands: command_rx,
            egress,
        },
        app,
    ));

    command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }))
        .expect("the reactor task holds the receiver");

    let (settled_id, settlement) = tokio::time::timeout(Duration::from_secs(2), settled_rx.recv())
        .await
        .expect("the command settles within the window")
        .expect("the reactor task is alive");
    assert_eq!(settled_id, CommandId(7));
    assert_eq!(settlement, Settlement::AnnounceNow(Ok(())));

    for out_rx in [&mut first_out_rx, &mut second_out_rx] {
        let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.peek())
            .await
            .expect("an announce fires on each interface");
        let (header, _) = WirePacketHeader::parse(frame.frame()).expect("valid announce wire");
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(DestinationHash::from_address(header.address), destination);
    }

    #[cfg(feature = "runtime-metrics")]
    {
        let (reply, snapshot) = oneshot::channel();
        command_tx
            .send(HostCommand::SnapshotMetrics { reply })
            .expect("the reactor task holds the receiver");
        let snapshot = snapshot.await.expect("the reactor returns its metrics");
        assert_eq!(
            snapshot
                .engine
                .announces
                .commands
                .get(crate::engine::AnnounceCommandOutcome::Succeeded),
            1
        );
        assert_eq!(
            snapshot
                .egress
                .announces
                .outcomes
                .get(AnnounceOrigin::Local, AnnounceEgressOutcome::Enqueued),
            2
        );
    }
}

#[tokio::test]
async fn a_link_establishes_and_carries_data_across_two_live_reactors() {
    use crate::engine::test_support::{personal_node_destination, second_secret_key};
    use crate::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
        LinkEstablished, RatchetPolicy, SendToLink, SendToLinkFailure, SendToLinkPayload,
        Settlement,
    };
    use crate::routing::delivery::Delivery;
    use crate::routing::links::LinkId;
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

    let initiator_iface = InterfaceId::new([0xA1; 8]);
    let responder_iface = InterfaceId::new([0xB2; 8]);

    let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
    let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();

    let initiator_engine = EngineState::<TestStorageLayout>::new(second_secret_key());
    let (a_notify_tx, a_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (a_in_tx, a_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (a_out_tx, a_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let a_iface = LoopbackInterface {
        descriptor: descriptor(initiator_iface),
        wire_in: b_to_a_rx,
        wire_out: a_to_b_tx,
    };
    let a_seam = TokioInterfaceSeam::new(initiator_iface, a_in_tx, a_notify_tx, a_out_rx);
    let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
    let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
    let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let (a_delivered_tx, mut a_delivered_rx) =
        mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
    let a_app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            let _ = a_heard_tx.send(());
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = a_settled_tx.send((id, settlement));
        }
        Journaled::Delivered(Delivery::Link(link)) => {
            let _ = a_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
        }
        _ => {}
    };

    let responder_engine = {
        use crate::engine::test_support::fixed_secret_key;
        let mut engine: EngineState<TestStorageLayout> = EngineState::new(fixed_secret_key());
        let node = engine.held_identity_hashes()[0];
        engine
            .register_single_destination(
                &node,
                "personal",
                &["node"],
                b"hello-personal",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the proving destination");
        engine
    };
    let (b_notify_tx, b_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (b_in_tx, b_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let (b_out_tx, b_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
    let b_iface = LoopbackInterface {
        descriptor: descriptor(responder_iface),
        wire_in: a_to_b_rx,
        wire_out: b_to_a_tx,
    };
    let b_seam = TokioInterfaceSeam::new(responder_iface, b_in_tx, b_notify_tx, b_out_rx);
    let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
    let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
    let (b_settled_tx, mut b_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
    let (b_delivered_tx, mut b_delivered_rx) =
        mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
    let b_app = move |journaled: Journaled<'_>| match journaled {
        Journaled::LinkEstablished(established) => {
            let _ = b_established_tx.send(established);
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = b_settled_tx.send((id, settlement));
        }
        Journaled::Delivered(Delivery::Link(link)) => {
            let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
        }
        _ => {}
    };

    tokio::spawn(run(
        initiator_engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![descriptor(initiator_iface)],
            ifacs: std::vec![],
            notify: a_notify_rx,
            inbound_lanes: std::vec![(initiator_iface, a_in_rx)],
            commands: a_command_rx,
            egress: a_egress,
        },
        a_app,
    ));
    tokio::spawn(run(
        responder_engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces: std::vec![descriptor(responder_iface)],
            ifacs: std::vec![],
            notify: b_notify_rx,
            inbound_lanes: std::vec![(responder_iface, b_in_rx)],
            commands: b_command_rx,
            egress: b_egress,
        },
        b_app,
    ));
    tokio::spawn(a_iface.run(a_seam));
    tokio::spawn(b_iface.run(b_seam));

    b_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination: personal_node_destination(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), a_heard_rx.recv())
        .await
        .expect("the announce crosses the wire")
        .expect("the initiator reactor is alive");

    a_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(7),
            command: EngineCommand::EstablishLink(EstablishLink {
                destination: personal_node_destination(),
            }),
        }))
        .unwrap();

    let (settled_id, settlement) =
        tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the link settles within the window")
            .expect("the initiator reactor is alive");
    assert_eq!(settled_id, CommandId(7));
    let Settlement::EstablishLink(Ok(established)) = settlement else {
        panic!("the command must settle established, got {settlement:?}");
    };

    let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
        .await
        .expect("the responder journals the link up")
        .expect("the responder reactor is alive");
    assert_eq!(
        responder_side.link_id, established.link_id,
        "one link, two ends",
    );
    assert!(
        responder_side.rtt_ms >= established.rtt_ms,
        "the responder takes max(measured, reported)",
    );

    a_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(8),
            command: EngineCommand::SendToLink(SendToLink {
                link_id: established.link_id,
                payload: SendToLinkPayload::from_slice(b"ping over the live link").unwrap(),
            }),
        }))
        .unwrap();
    let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
        .await
        .expect("the responder journals the delivery")
        .expect("the responder reactor is alive");
    assert_eq!(
        delivered,
        (established.link_id, b"ping over the live link".to_vec()),
    );
    let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
        .await
        .expect("the initiator's send settles")
        .expect("the initiator reactor is alive");
    assert_eq!(sent_id, CommandId(8));
    let Settlement::SendToLink(Ok(_delivered_receipt)) = sent else {
        panic!("the ProveAll responder's proof settles the send Delivered, got {sent:?}");
    };

    b_command_tx
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(2),
            command: EngineCommand::SendToLink(SendToLink {
                link_id: established.link_id,
                payload: SendToLinkPayload::from_slice(b"pong right back").unwrap(),
            }),
        }))
        .unwrap();
    let sent = loop {
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), b_settled_rx.recv())
            .await
            .expect("the responder's send settles")
            .expect("the responder reactor is alive");
        if sent_id == CommandId(2) {
            break sent;
        }
    };
    assert_eq!(
        sent,
        Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
        "the initiator's side never proves, so the responder's send times out — parity",
    );
    let delivered = tokio::time::timeout(Duration::from_secs(5), a_delivered_rx.recv())
        .await
        .expect("the initiator journals the delivery")
        .expect("the initiator reactor is alive");
    assert_eq!(
        delivered,
        (established.link_id, b"pong right back".to_vec()),
    );
}
