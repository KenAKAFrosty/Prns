use std::cell::RefCell;
use std::rc::Rc;

use embassy_futures::block_on;
use embassy_futures::select::{select, Either};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};
use heapless::Vec as HeaplessVec;

use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_4_0_ANNOUNCE, TEST_TRANSPORT_ID,
};
use crate::engine::{EngineState, IssuedCommand, Journaled};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::reactor::grant::GrantProducer;
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::runtime::NoInterfaceInspectionStore;
use crate::storage::GrowableHeap;

use super::super::test_support::{descriptor, WATCHDOG};
use super::super::{
    leaked_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, PooledEgress,
};
use super::{run_pooled, InterfaceLifecycle, PooledWiring};

#[test]
fn a_pooled_ifac_slot_added_at_runtime_opens_inbound_then_frees_on_remove() {
    use crate::interfaces::{IfacContext, IfacSize};

    let source = InterfaceId::new([0xA1; 8]);
    let network = IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap();

    let mut engine = EngineState::<GrowableHeap>::default();
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

    let raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);
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
        let mut descriptors: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let _ = ifacs.push(InterfaceIfac {
            id: source,
            context: network,
        });
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
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

    let raw = bytes_from_hex(RNS_1_4_0_ANNOUNCE);

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
        let mut descriptors: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let reactor = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
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
