use std::cell::RefCell;
use std::rc::Rc;

use embassy_futures::block_on;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};

use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_4_2_ANNOUNCE, TEST_TRANSPORT_ID,
};
use crate::engine::{EngineState, IssuedCommand, Journaled};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{AttachedInterfaces, InterfaceDescriptor, InterfaceId};
use crate::manifold::grant::{
    GrantConsumer, GrantProducer, ManifoldLaneReader, ManifoldLaneWriter,
};
use crate::manifold::interface_seam::{Interface, InterfaceSeam, EMBEDDED_MAX_WIRE_FRAME_LEN};
use crate::wire::{PacketType, WirePacketHeader};

use super::super::test_support::{descriptor, WATCHDOG};
use super::super::{
    leaked_grant_lane, EmbassyEgress, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam,
};
use super::{run, ManifoldWiring};

struct EmbassyLoopbackInterface<'a, M: RawMutex, const FRAME: usize> {
    descriptor: InterfaceDescriptor,
    wire_in: EmbassyGrantConsumer<'a, M, FRAME>,
    wire_out: EmbassyGrantProducer<'a, M, FRAME>,
}

impl<M: RawMutex, const FRAME: usize> Interface for EmbassyLoopbackInterface<'_, M, FRAME> {
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
    use crate::interfaces::{IfacContext, IfacSize};

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

    const SOURCE_SLOT: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    const PEER_SLOT: usize = 256;
    let (source_in_tx, mut source_in_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
    let (mut source_out_tx, source_out_rx) = leaked_grant_lane::<SOURCE_SLOT>(2);
    let (peer_in_tx, mut peer_in_rx) = leaked_grant_lane::<PEER_SLOT>(2);
    let (mut peer_out_tx, peer_out_rx) = leaked_grant_lane::<PEER_SLOT>(2);

    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
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
        | Journaled::PersistenceFlushed { .. }
        | Journaled::PersistenceFlushFailed { .. }
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
        let mut egress_lanes: [(InterfaceId, &mut dyn ManifoldLaneWriter); 2] =
            [(source, &mut source_out_tx), (peer, &mut peer_out_tx)];
        let egress = EmbassyEgress::new(&mut egress_lanes);
        let mut inbound_lanes: [(InterfaceId, &mut dyn ManifoldLaneReader); 2] =
            [(source, &mut source_in_rx), (peer, &mut peer_in_rx)];

        let manifold = run(
            engine,
            EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0)),
            ManifoldWiring {
                interfaces: AttachedInterfaces::new(&interfaces),
                ifacs: &ifacs,
                notify: notify.receiver(),
                inbound_lanes: &mut inbound_lanes,
                frame_accounting_statuses: &[],
                commands: commands.receiver(),
                egress,
            },
            app,
        );

        let source_seam = EmbassyInterfaceSeam::new(
            source,
            source_in_tx,
            notify.sender(),
            source_out_rx,
            |bytes| bytes.fill(0),
        );
        let source_iface = EmbassyLoopbackInterface {
            descriptor: descriptor(source),
            wire_in: source_wire_in_rx,
            wire_out: source_wire_out_tx,
        };
        let source_run = source_iface.run(source_seam);

        let peer_seam =
            EmbassyInterfaceSeam::new(peer, peer_in_tx, notify.sender(), peer_out_rx, |bytes| {
                bytes.fill(0)
            });
        let peer_iface = EmbassyLoopbackInterface {
            descriptor: descriptor(peer),
            wire_in: peer_wire_in_rx,
            wire_out: peer_wire_out_tx,
        };
        let peer_run = peer_iface.run(peer_seam);

        let driver = async {
            Timer::after(Duration::from_millis(50)).await;
            assert_eq!(*heard.borrow(), 0, "an idle manifold journals nothing");
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
                        GrantConsumer::release(&mut peer_wire_out_rx);
                        break rebroadcast;
                    }
                }
                yield_now().await;
            }
        };

        match select4(
            manifold,
            source_run,
            peer_run,
            with_timeout(WATCHDOG, driver),
        )
        .await
        {
            Either4::Fourth(result) => result.expect("the rebroadcast fires before the watchdog"),
            _ => unreachable!("the manifold and interface loops never return"),
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
