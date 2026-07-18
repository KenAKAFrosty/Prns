use super::*;
use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_3_5_ANNOUNCE, TEST_TRANSPORT_ID,
};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceMode, TransportCapability,
};
use crate::reactor::grant::{GrantConsumer, GrantProducer};
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::wire::{PacketType, WirePacketHeader};

use embassy_futures::block_on;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};

use std::cell::RefCell;
use std::rc::Rc;

const WATCHDOG: Duration = Duration::from_secs(5);

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
fn packet_phy_retention_reuses_the_classified_packet_hash() {
    const PACKET_PHY_CAPACITY: usize = 8;
    const PACKET_PHY_INDEX_BUCKETS: usize =
        crate::routing::dedup::dedup_index_buckets(PACKET_PHY_CAPACITY);

    let store = EmbassyInterfaceStore::<
        CriticalSectionRawMutex,
        8,
        PACKET_PHY_CAPACITY,
        PACKET_PHY_INDEX_BUCKETS,
    >::new();
    let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let expected = crate::routing::dedup::PacketHash::of_wire_packet(&raw)
        .expect("the fixture is a wire packet");
    let packet = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: InterfaceId::new([0xC7; 8]),
        bytes: &mut raw,
    });
    let packet_phy = PacketPhyStats {
        rssi: Some(crate::interfaces::RssiDbm::new(-103)),
        snr: Some(crate::interfaces::SnrQuarterDb::new(-11)),
        quality: crate::interfaces::SignalQualityTenthsPercent::new(731),
    };

    retain_packet_phy(&store, &packet, packet_phy);

    assert_eq!(packet.packet_hash(), Some(expected));
    assert_eq!(store.packet_phy(expected), Some(packet_phy));
}

#[test]
fn packet_phy_crosses_the_embassy_ingress_seam_with_its_frame() {
    const SLOT: usize = 64;

    let interface = InterfaceId::new([0xA1; 8]);
    let (inbound, mut reactor_inbound) = leaked_grant_lane::<SLOT>(1);
    let (_reactor_outbound, outbound) = leaked_grant_lane::<SLOT>(1);
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let packet_phy = PacketPhyStats {
        rssi: Some(crate::interfaces::RssiDbm::new(-87)),
        snr: Some(crate::interfaces::SnrQuarterDb::new(-9)),
        quality: crate::interfaces::SignalQualityTenthsPercent::new(875),
    };
    let mut seam = EmbassyInterfaceSeam::new(interface, inbound, notify.sender(), outbound);

    block_on(seam.next_inbound_with_phy(b"observed", packet_phy));

    let retained = reactor_inbound
        .try_peek()
        .expect("the committed frame reaches the reactor lane");
    assert_eq!(
        (retained.frame(), retained.packet_phy),
        (b"observed".as_slice(), packet_phy)
    );
    assert_eq!(notify.receiver().try_receive(), Ok(interface));

    reactor_inbound.release();
    block_on(seam.next_inbound(b"plain"));

    let retained = reactor_inbound
        .try_peek()
        .expect("the next committed frame reaches the reactor lane");
    assert_eq!(
        (retained.frame(), retained.packet_phy),
        (b"plain".as_slice(), PacketPhyStats::default())
    );
}

struct EmbassyLoopbackInterface<'a, M: RawMutex, const SLOT: usize> {
    descriptor: InterfaceDescriptor,
    wire_in: EmbassyGrantConsumer<'a, M, SLOT>,
    wire_out: EmbassyGrantProducer<'a, M, SLOT>,
}

impl<M: RawMutex, const SLOT: usize> Interface for EmbassyLoopbackInterface<'_, M, SLOT> {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn channel_tag(&self) -> &[u8] {
        self.descriptor.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let id = self.descriptor.id;
        let mut wire_in = self.wire_in;
        let mut wire_out = self.wire_out;
        loop {
            match select(wire_in.peek(), seam.next_outbound()).await {
                Either::First(slot) => {
                    seam.next_inbound(slot.frame()).await;
                    wire_in.release();
                }
                Either::Second(out) => {
                    wire_out.grant().await.fill_for(id, out);
                    wire_out.commit();
                }
            }
        }
    }
}

#[test]
fn an_ifac_frame_crosses_the_seam_and_leaves_masked_through_the_peer() {
    use crate::interfaces::ifac::{IfacContext, IfacSize};

    let source = InterfaceId::new([0xA1; 8]);
    let peer = InterfaceId::new([0xB2; 8]);
    let interfaces = [descriptor(source), descriptor(peer)];
    let network =
        || IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();
    let ifacs = [
        InterfaceIfac {
            id: source,
            context: network(),
        },
        InterfaceIfac {
            id: peer,
            context: network(),
        },
    ];

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
    let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();

    let (mut source_wire_in_tx, source_wire_in_rx) =
        leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let (source_wire_out_tx, _source_wire_out_rx) =
        leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let (_peer_wire_in_tx, peer_wire_in_rx) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let (peer_wire_out_tx, mut peer_wire_out_rx) =
        leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);

    // The grant lanes are deliberately sized apart: erasure carries both through one reactor, each paying only its own slot size.
    const SOURCE_SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    const PEER_SLOT: usize = 256;
    let (source_in_tx, mut source_in_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
    let (mut source_out_tx, source_out_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
    let (peer_in_tx, mut peer_in_rx) = leaked_grant_lane::<PEER_SLOT>(2);
    let (mut peer_out_tx, peer_out_rx) = leaked_grant_lane::<PEER_SLOT>(2);

    let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut masked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
    let masked_len = network().mask_outbound(&raw, &mut masked).unwrap();
    let original_hops = WirePacketHeader::parse(&raw)
        .expect("valid announce wire")
        .0
        .hops;

    let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let heard_sink = heard.clone();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            *heard_sink.borrow_mut() += 1;
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

    let outcome = block_on(async {
        let mut egress_lanes: [(InterfaceId, &mut dyn AnyGrantProducer); 2] =
            [(source, &mut source_out_tx), (peer, &mut peer_out_tx)];
        let egress = EmbassyEgress::new(&mut egress_lanes);
        let mut inbound_lanes: [(InterfaceId, &mut dyn AnyGrantConsumer); 2] =
            [(source, &mut source_in_rx), (peer, &mut peer_in_rx)];

        let reactor = run(
            engine,
            EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
            ReactorWiring {
                interfaces: AttachedInterfaces::new(&interfaces),
                ifacs: &ifacs,
                notify: notify.receiver(),
                inbound_lanes: &mut inbound_lanes,
                commands: commands.receiver(),
                egress,
            },
            app,
        );

        let source_seam =
            EmbassyInterfaceSeam::new(source, source_in_tx, notify.sender(), source_out_rx);
        let source_iface = EmbassyLoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_run = source_iface.run(source_seam);

        let peer_seam = EmbassyInterfaceSeam::new(peer, peer_in_tx, notify.sender(), peer_out_rx);
        let peer_iface = EmbassyLoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_run = peer_iface.run(peer_seam);

        let driver = async {
            Timer::after(Duration::from_millis(50)).await;
            assert_eq!(*heard.borrow(), 0, "an idle reactor journals nothing");
            assert!(
                peer_wire_out_rx.try_peek().is_none(),
                "an idle interface transmits nothing"
            );

            source_wire_in_tx
                .grant()
                .await
                .fill_for(source, &masked[..masked_len]);
            source_wire_in_tx.commit();

            loop {
                if *heard.borrow() >= 1 {
                    if let Some(slot) = peer_wire_out_rx.try_peek() {
                        let rebroadcast = slot.frame().to_vec();
                        peer_wire_out_rx.release();
                        break rebroadcast;
                    }
                }
                yield_now().await;
            }
        };

        match select4(
            reactor,
            source_run,
            peer_run,
            with_timeout(WATCHDOG, driver),
        )
        .await
        {
            Either4::Fourth(result) => result.expect("the rebroadcast fires before the watchdog"),
            _ => unreachable!("the reactor and interface loops never return"),
        }
    });

    assert_eq!(outcome[0] & 0x80, 0x80);
    let mut opened = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
    let opened_len = network().unmask_inbound(&outcome, &mut opened).unwrap();
    let (header, _) =
        WirePacketHeader::parse(&opened[..opened_len]).expect("valid rebroadcast wire");
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(
        header.hops,
        original_hops + 1,
        "the rebroadcast bumps the hop count"
    );
}

#[test]
fn a_pooled_ifac_slot_added_at_runtime_opens_inbound_then_frees_on_remove() {
    use crate::interfaces::ifac::{IfacContext, IfacSize};

    let source = InterfaceId::new([0xA1; 8]);
    let network = IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
    let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();
    let lifecycle: Channel<CriticalSectionRawMutex, InterfaceLifecycle, 2> = Channel::new();

    const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<SLOT>(2);
    let (source_out_tx, _source_out_rx) = leaked_grant_lane::<SLOT>(2);

    let mut inbound: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantConsumer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = inbound.push((source, source_in_rx));
    let mut egress_lanes: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = egress_lanes.push((source, source_out_tx));

    let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
    let mut masked = [0u8; SLOT];
    let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();

    let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let heard_sink = heard.clone();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            *heard_sink.borrow_mut() += 1;
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

    let mut egress = PooledEgress::new(egress_lanes);
    let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
    let count = block_on(async {
        let initial: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let _ = ifacs.push(InterfaceIfac {
            id: source,
            context: network,
        });
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                initial: &initial,
                inbound: &mut inbound,
                egress: &mut egress,
                notify: notify.receiver(),
                commands: commands.receiver(),
                lifecycle: lifecycle.receiver(),
                ifacs: &mut ifacs,
            },
            app,
            crate::reactor::decline_all(),
            &NoInterfaceInspectionStore,
        );

        let driver = async {
            lifecycle
                .sender()
                .send(InterfaceLifecycle::Add {
                    descriptor: descriptor(source),
                })
                .await;
            Timer::after(Duration::from_millis(30)).await;
            source_in_tx
                .grant()
                .await
                .fill_for(source, &masked[..masked_len]);
            source_in_tx.commit();
            notify.sender().send(source).await;
            loop {
                if *heard.borrow() >= 1 {
                    break;
                }
                yield_now().await;
            }

            lifecycle
                .sender()
                .send(InterfaceLifecycle::Remove { id: source })
                .await;
            Timer::after(Duration::from_millis(30)).await;
            *heard.borrow()
        };

        match select(reactor, with_timeout(WATCHDOG, driver)).await {
            Either::Second(result) => result.expect("the slot is heard before the watchdog"),
            Either::First(()) => unreachable!("the reactor loop never returns"),
        }
    });

    assert_eq!(
        count, 1,
        "the runtime-added slot carried exactly the one announce"
    );
}

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

#[test]
fn a_pooled_slot_retagged_at_runtime_carries_traffic_under_the_new_id() {
    let old_id = InterfaceId::new([0xA1; 8]);
    let new_id = InterfaceId::new([0xB2; 8]);

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
    let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();
    let lifecycle: Channel<CriticalSectionRawMutex, InterfaceLifecycle, 2> = Channel::new();

    const SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<SLOT>(2);
    let (source_out_tx, _source_out_rx) = leaked_grant_lane::<SLOT>(2);

    let mut inbound: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantConsumer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = inbound.push((old_id, source_in_rx));
    let mut egress_lanes: HeaplessVec<
        (
            InterfaceId,
            EmbassyGrantProducer<'static, CriticalSectionRawMutex, SLOT>,
        ),
        1,
    > = HeaplessVec::new();
    let _ = egress_lanes.push((old_id, source_out_tx));

    let raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);

    let heard: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let heard_sink = heard.clone();
    let app = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { .. } => {
            *heard_sink.borrow_mut() += 1;
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

    let mut egress = PooledEgress::new(egress_lanes);
    let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
    let count = block_on(async {
        let initial: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                initial: &initial,
                inbound: &mut inbound,
                egress: &mut egress,
                notify: notify.receiver(),
                commands: commands.receiver(),
                lifecycle: lifecycle.receiver(),
                ifacs: &mut ifacs,
            },
            app,
            crate::reactor::decline_all(),
            &NoInterfaceInspectionStore,
        );

        let driver = async {
            lifecycle
                .sender()
                .send(InterfaceLifecycle::Add {
                    descriptor: descriptor(old_id),
                })
                .await;
            Timer::after(Duration::from_millis(30)).await;
            lifecycle
                .sender()
                .send(InterfaceLifecycle::Retag {
                    old_id,
                    new_id,
                    descriptor: descriptor(new_id),
                })
                .await;
            Timer::after(Duration::from_millis(30)).await;
            source_in_tx.grant().await.fill_for(new_id, &raw);
            source_in_tx.commit();
            notify.sender().send(new_id).await;
            loop {
                if *heard.borrow() >= 1 {
                    break;
                }
                yield_now().await;
            }
            *heard.borrow()
        };

        match select(reactor, with_timeout(WATCHDOG, driver)).await {
            Either::Second(result) => {
                result.expect("the retagged slot is heard before the watchdog")
            }
            Either::First(()) => unreachable!("the reactor loop never returns"),
        }
    });

    assert_eq!(
        count, 1,
        "the retagged slot carried the announce under its new channel id"
    );
}
