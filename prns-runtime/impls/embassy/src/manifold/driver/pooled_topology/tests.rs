use std::cell::{Cell, RefCell};
use std::rc::Rc;

use embassy_futures::block_on;
use embassy_futures::select::{select, Either};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Timer};
use heapless::Vec as HeaplessVec;

use crate::engine::test_support::{
    bytes_from_hex, pin_transport_id, TestStorageLayout, RNS_1_4_2_ANNOUNCE, TEST_TRANSPORT_ID,
};
use crate::engine::{EngineState, InstantMillis, IssuedCommand, Journaled};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::manifold::grant::{GrantProducer, ManifoldLaneReader};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::runtime::{ManifoldPersistence, NoInterfaceInspectionStore, NoManifoldPersistence};
use crate::storage::{GrowableHeap, StorageLayout};

use super::super::test_support::{descriptor, WATCHDOG};
use super::super::{leaked_grant_lane, EmbassyHost, PooledEgress};
use super::{inbound_source, run_pooled, InterfaceLifecycle, PooledWiring};

struct AlwaysDuePersistence {
    progress: Rc<Cell<usize>>,
}

#[test]
fn dedicated_lanes_own_the_source_id_but_fleet_lanes_preserve_the_member_stamp() {
    let prior_dedicated_id = InterfaceId::new([0xA1; 8]);
    let live_dedicated_id = InterfaceId::new([0xB2; 8]);
    let fleet_lane_id = InterfaceId::new([0xC3; 8]);
    let member_id = InterfaceId::new([0xD4; 8]);
    let descriptors = [descriptor(live_dedicated_id), descriptor(member_id)];

    assert_eq!(
        inbound_source(live_dedicated_id, prior_dedicated_id, &descriptors),
        live_dedicated_id
    );
    assert_eq!(
        inbound_source(fleet_lane_id, member_id, &descriptors),
        member_id
    );
}

impl<S: StorageLayout> ManifoldPersistence<S> for AlwaysDuePersistence {
    fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

    fn deadline(&self, now: InstantMillis) -> Option<InstantMillis> {
        Some(now)
    }

    async fn progress(&mut self, _engine: &mut EngineState<S>, _now: InstantMillis) {
        let progress = self.progress.get() + 1;
        self.progress.set(progress);
        assert!(progress <= 4, "persistence monopolized the executor");
    }
}

#[test]
fn continuously_due_persistence_yields_to_sibling_tasks() {
    let progress = Rc::new(Cell::new(0));
    let observed = progress.clone();

    block_on(async {
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 1> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 1> = Channel::new();
        let lifecycle: Channel<CriticalSectionRawMutex, InterfaceLifecycle, 1> = Channel::new();
        let mut descriptors: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let mut inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), 1> =
            HeaplessVec::new();
        let mut egress: PooledEgress<1> = PooledEgress::new();
        let mut persistence = AlwaysDuePersistence { progress };
        let manifold = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
                ifacs: &mut ifacs,
                inbound: &mut inbound,
                frame_accounting_statuses: &[],
                egress: &mut egress,
                notify: notify.receiver(),
                commands: commands.receiver(),
                lifecycle: lifecycle.receiver(),
            },
            |_| {},
            crate::manifold::decline_all(),
            &NoInterfaceInspectionStore,
            &mut persistence,
        );
        let sibling = async {
            yield_now().await;
        };

        match select(manifold, sibling).await {
            Either::Second(()) => {}
            Either::First(()) => unreachable!("the manifold loop never returns"),
        }
    });

    assert!((1..=4).contains(&observed.get()));
}

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

    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<FRAME>(2);
    let (source_out_tx, _source_out_rx) = leaked_grant_lane::<FRAME>(2);

    let mut inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), 1> =
        HeaplessVec::new();
    let _ = inbound.push((
        source,
        std::boxed::Box::leak(std::boxed::Box::new(source_in_rx)),
    ));

    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    let mut masked = [0u8; FRAME];
    let masked_len = network.mask_outbound(&raw, &mut masked).unwrap();

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

    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        source,
        std::boxed::Box::leak(std::boxed::Box::new(source_out_tx)),
    );
    let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
    let count = block_on(async {
        let mut descriptors: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let mut persistence = NoManifoldPersistence;
        let _ = ifacs.push(InterfaceIfac {
            id: source,
            context: network,
        });
        let manifold = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
                inbound: &mut inbound,
                frame_accounting_statuses: &[],
                egress: &mut egress,
                notify: notify.receiver(),
                commands: commands.receiver(),
                lifecycle: lifecycle.receiver(),
                ifacs: &mut ifacs,
            },
            app,
            crate::manifold::decline_all(),
            &NoInterfaceInspectionStore,
            &mut persistence,
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

        match select(manifold, with_timeout(WATCHDOG, driver)).await {
            Either::Second(result) => result.expect("the slot is heard before the watchdog"),
            Either::First(()) => unreachable!("the manifold loop never returns"),
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

    const FRAME: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
    let (mut source_in_tx, source_in_rx) = leaked_grant_lane::<FRAME>(2);
    let (source_out_tx, _source_out_rx) = leaked_grant_lane::<FRAME>(2);

    let mut inbound: HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), 1> =
        HeaplessVec::new();
    let _ = inbound.push((
        old_id,
        std::boxed::Box::leak(std::boxed::Box::new(source_in_rx)),
    ));

    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);

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

    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        old_id,
        std::boxed::Box::leak(std::boxed::Box::new(source_out_tx)),
    );
    let mut host = EmbassyHost::new(|bytes: &mut [u8]| bytes.fill(0));
    let count = block_on(async {
        let mut descriptors: HeaplessVec<InterfaceDescriptor, 1> = HeaplessVec::new();
        let mut ifacs: HeaplessVec<InterfaceIfac, 1> = HeaplessVec::new();
        let mut persistence = NoManifoldPersistence;
        let manifold = run_pooled(
            &mut engine,
            &mut host,
            PooledWiring {
                descriptors: &mut descriptors,
                inbound: &mut inbound,
                frame_accounting_statuses: &[],
                egress: &mut egress,
                notify: notify.receiver(),
                commands: commands.receiver(),
                lifecycle: lifecycle.receiver(),
                ifacs: &mut ifacs,
            },
            app,
            crate::manifold::decline_all(),
            &NoInterfaceInspectionStore,
            &mut persistence,
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
            // The real interface seam was constructed under `old_id`; retagging the pooled lane
            // cannot rewrite that value inside an already-running interface task.
            source_in_tx.grant().await.fill_for(old_id, &raw);
            source_in_tx.commit();
            notify.sender().send(old_id).await;
            loop {
                if *heard.borrow() >= 1 {
                    break;
                }
                yield_now().await;
            }
            *heard.borrow()
        };

        match select(manifold, with_timeout(WATCHDOG, driver)).await {
            Either::Second(result) => {
                result.expect("the retagged slot is heard before the watchdog")
            }
            Either::First(()) => unreachable!("the manifold loop never returns"),
        }
    });

    assert_eq!(
        count, 1,
        "the retagged slot carried the announce under its new channel id"
    );
    assert_eq!(engine.interface_counts(new_id).destinations, 1);
    assert_eq!(engine.interface_counts(old_id).destinations, 0);
}
