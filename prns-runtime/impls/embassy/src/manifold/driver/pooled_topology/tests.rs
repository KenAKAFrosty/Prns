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
use crate::interfaces::rns_management::RnsRemotePathTableRequest;
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::manifold::grant::{GrantProducer, ManifoldLaneReader};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::routing::links::request::{response_envelope_prefix, RequestId, RESPONSE_WIRE_OVERHEAD};
use crate::runtime::{ManifoldPersistence, NoInterfaceInspectionStore, NoManifoldPersistence};
use crate::runtime::{ResourceResponse, ResourceResponsePayload};
use crate::storage::{GrowableHeap, StorageLayout};

use super::super::test_support::{descriptor, WATCHDOG};
use super::super::{leaked_grant_lane, EmbassyHost, PooledEgress};
use super::{inbound_source, resource_response_data, run_pooled, InterfaceLifecycle, PooledWiring};

struct AlwaysDuePersistence {
    progress: Rc<Cell<usize>>,
}

#[test]
fn path_table_response_materializes_inside_the_manifold() {
    const RESPONSE_BYTES: usize = RESPONSE_WIRE_OVERHEAD + 1;
    let engine = EngineState::<GrowableHeap>::default();
    let request_id = RequestId([0x42; 16]);
    let selection = RnsRemotePathTableRequest::new(None, None);
    let mut ready = HeaplessVec::<u8, RESPONSE_BYTES>::new();
    ready.extend_from_slice(b"ready").unwrap();

    let materialized = resource_response_data(
        &engine,
        &[],
        request_id,
        ResourceResponsePayload::Ready(ready.clone()),
    )
    .unwrap();
    assert_eq!(materialized.as_slice(), ready.as_slice());

    let data = resource_response_data::<_, RESPONSE_BYTES>(
        &engine,
        &[],
        request_id,
        ResourceResponsePayload::RnsPathTable(selection),
    )
    .unwrap();

    assert_eq!(
        &data.as_slice()[..RESPONSE_WIRE_OVERHEAD],
        &response_envelope_prefix(&request_id)
    );
    assert_eq!(data.as_slice()[RESPONSE_WIRE_OVERHEAD], 0x90);
    assert!(resource_response_data::<_, 0>(
        &engine,
        &[],
        request_id,
        ResourceResponsePayload::RnsPathTable(selection),
    )
    .is_some());
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
    fn has_pending_configuration_change(&self) -> bool {
        false
    }

    fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

    fn deadline(&mut self, now: InstantMillis) -> Option<InstantMillis> {
        Some(now)
    }

    fn observe_remote_control_pairing_failure(
        &mut self,
        _failure: crate::runtime::EmbeddedRemoteControlPairingPersistenceFailure,
    ) {
    }

    async fn progress(&mut self, _engine: &mut EngineState<S>, _now: InstantMillis) {
        let progress = self.progress.get() + 1;
        self.progress.set(progress);
        assert!(progress <= 4, "persistence monopolized the executor");
    }

    async fn store_remote_control_authorization_snapshot(
        &mut self,
        _engine: &EngineState<S>,
        _kind: crate::runtime::RemoteControlAuthorizationSnapshotKind,
        _snapshot: &crate::runtime::RemoteControlAuthorizationSnapshot,
        _now: InstantMillis,
    ) -> crate::runtime::StoreRemoteControlAuthorizationSnapshotOutcome {
        crate::runtime::StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
            failure: crate::runtime::EmbeddedPersistenceFailure::Flash,
            retry_at: None,
        }
    }
}

#[test]
fn continuously_due_persistence_yields_to_sibling_tasks() {
    let progress = Rc::new(Cell::new(0));
    let observed = progress.clone();

    block_on(async {
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut host = EmbassyHost::new(crate::manifold::driver::test_support::entropy_handle());
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 1> = Channel::new();
        let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 1> = Channel::new();
        let responses: Channel<CriticalSectionRawMutex, ResourceResponse<0>, 1> = Channel::new();
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
                resource_responses: responses.receiver(),
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
    let responses: Channel<CriticalSectionRawMutex, ResourceResponse<0>, 1> = Channel::new();
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
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. }
        | Journaled::RemoteControlPairingAvailabilityObserved(_)
        | Journaled::RemoteControlPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
        | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
        | Journaled::RemoteControlTargetPairingAuthorizationRequired { .. }
        | Journaled::RemoteControlTargetPairingAuthorizationPersisted { .. }
        | Journaled::RemoteControlTargetPairingExpiredDuringAuthorization { .. }
        | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
        | Journaled::RemoteControlControllerPairingPersistenceRequired(_)
        | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
        | Journaled::RemoteControlControllerPairingAuthorizationPersistenceFailed { .. }
        | Journaled::RemoteControlControllerPairingExpired { .. }
        | Journaled::RemoteControlControllerPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
        | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
        | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
    };

    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        source,
        std::boxed::Box::leak(std::boxed::Box::new(source_out_tx)),
    );
    let mut host = EmbassyHost::new(crate::manifold::driver::test_support::entropy_handle());
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
                resource_responses: responses.receiver(),
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
    let published = embassy_sync::signal::Signal::new();
    let old_id = InterfaceId::new([0xA1; 8]);
    let new_id = InterfaceId::new([0xB2; 8]);

    let mut engine = EngineState::<TestStorageLayout>::default();
    pin_transport_id(&mut engine, TEST_TRANSPORT_ID);

    let notify: Channel<CriticalSectionRawMutex, InterfaceId, 4> = Channel::new();
    let commands: Channel<CriticalSectionRawMutex, IssuedCommand, 2> = Channel::new();
    let responses: Channel<CriticalSectionRawMutex, ResourceResponse<0>, 1> = Channel::new();
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
        | Journaled::ResourceSegmentReceived { .. }
        | Journaled::ResourceAssembled { .. }
        | Journaled::LinkInterfaceMismatch { .. }
        | Journaled::RemoteControlPairingAvailabilityObserved(_)
        | Journaled::RemoteControlPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
        | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
        | Journaled::RemoteControlTargetPairingAuthorizationRequired { .. }
        | Journaled::RemoteControlTargetPairingAuthorizationPersisted { .. }
        | Journaled::RemoteControlTargetPairingExpiredDuringAuthorization { .. }
        | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
        | Journaled::RemoteControlControllerPairingPersistenceRequired(_)
        | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
        | Journaled::RemoteControlControllerPairingAuthorizationPersistenceFailed { .. }
        | Journaled::RemoteControlControllerPairingExpired { .. }
        | Journaled::RemoteControlControllerPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingExpired { .. }
        | Journaled::RemoteControlTargetPairingLinkClosed { .. }
        | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
        | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
        | Journaled::RemoteControlPairingExpiryFailed { .. } => {}
    };

    let mut egress: PooledEgress<1> = PooledEgress::new();
    let _ = egress.push(
        old_id,
        std::boxed::Box::leak(std::boxed::Box::new(source_out_tx)),
    );
    let mut host = EmbassyHost::new(crate::manifold::driver::test_support::entropy_handle());
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
                resource_responses: responses.receiver(),
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
                .send(InterfaceLifecycle::Publish {
                    old_id,
                    descriptor: descriptor(new_id),
                    completion: &published,
                })
                .await;
            assert_eq!(
                published.wait().await,
                super::InterfacePublicationOutcome::Published
            );
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

#[test]
fn publication_rejection_preserves_topology_and_acceptance_updates_the_whole_lane() {
    use super::super::egress::{InterfacePacer, ManifoldEgress};
    use super::{publish_interface, InterfacePublicationOutcome};
    let old = InterfaceId::new([0x11; 8]);
    let new = InterfaceId::new([0x22; 8]);
    let conflict = InterfaceId::new([0x33; 8]);
    let unknown = InterfaceId::new([0x44; 8]);
    let mut descriptors = HeaplessVec::<InterfaceDescriptor, 3>::new();
    descriptors.push(descriptor(old)).unwrap();
    descriptors.push(descriptor(conflict)).unwrap();
    let mut egress = PooledEgress::<1>::new();
    let mut inbound = HeaplessVec::new();
    let mut ifacs = HeaplessVec::new();
    let mut pacers = HeaplessVec::new();
    assert_eq!(
        publish_interface(
            unknown,
            descriptor(new),
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::UnknownInterface
    );
    assert_eq!(
        publish_interface(
            old,
            descriptor(new),
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::UnknownInterface
    );
    let (outbound, inbound_lane) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
    assert!(egress
        .push(old, std::boxed::Box::leak(std::boxed::Box::new(outbound)))
        .is_ok());
    assert!(inbound
        .push((
            old,
            std::boxed::Box::leak(std::boxed::Box::new(inbound_lane))
                as &'static mut dyn ManifoldLaneReader
        ))
        .is_ok());
    assert!(pacers
        .push(InterfacePacer::from_descriptor(old, &descriptor(old)))
        .is_ok());
    ifacs
        .push(InterfaceIfac {
            id: old,
            context: crate::interfaces::IfacContext::derive(
                Some("radio"),
                Some("secret"),
                crate::interfaces::IfacSize::NARROW,
            )
            .unwrap(),
        })
        .ok()
        .unwrap();
    let before = descriptors.clone();
    assert_eq!(
        publish_interface(
            old,
            descriptor(conflict),
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::IdentityConflict
    );
    assert_eq!(descriptors, before);
    assert_eq!(inbound[0].0, old);
    assert_eq!(ifacs[0].id, old);
    assert_eq!(egress.lane_for(old), Some(old));
    assert_eq!(
        publish_interface(
            old,
            descriptor(new),
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::Published
    );
    assert_eq!(descriptors[0], descriptor(new));
    assert_eq!(inbound[0].0, new);
    assert_eq!(ifacs[0].id, new);
    assert_eq!(pacers[0].id, new);
    assert_eq!(egress.lane_for(old), None);
    assert_eq!(egress.lane_for(new), Some(new));
    assert_eq!(
        publish_interface(
            new,
            descriptor(new),
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::Published
    );
}

#[test]
fn publication_preserves_a_shared_fleet_lane_and_clamps_mtu() {
    use super::super::egress::{InterfacePacer, ManifoldEgress};
    use super::{publish_interface, InterfacePublicationOutcome};
    use crate::interfaces::InterfaceKind;
    let fleet = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"fleet");
    let old = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"old");
    let new = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"new");
    let mut descriptors = HeaplessVec::<InterfaceDescriptor, 1>::new();
    descriptors.push(descriptor(old)).unwrap();
    let mut egress = PooledEgress::<1>::new();
    let (producer, _) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
    assert!(egress
        .push(fleet, std::boxed::Box::leak(std::boxed::Box::new(producer)))
        .is_ok());
    let mut inbound = HeaplessVec::new();
    let mut ifacs = HeaplessVec::new();
    let mut pacers = HeaplessVec::new();
    pacers
        .push(InterfacePacer::from_descriptor(fleet, &descriptor(old)))
        .ok()
        .unwrap();
    let mut requested = descriptor(new);
    requested.hardware_mtu = Some(usize::MAX);
    assert_eq!(
        publish_interface(
            old,
            requested,
            &mut descriptors,
            &mut egress,
            &mut inbound,
            &mut ifacs,
            &mut pacers
        ),
        InterfacePublicationOutcome::Published
    );
    assert_eq!(egress.lane_for(new), Some(fleet));
    assert_eq!(pacers[0].id, fleet);
    assert_eq!(
        descriptors[0].hardware_mtu,
        Some(crate::manifold::interface_seam::EMBEDDED_MAX_LINK_MTU)
    );
}

#[test]
fn lifecycle_commands_preserve_unrelated_descriptors_and_handle_missing_or_duplicate_ids() {
    use super::InterfacePublicationOutcome;
    let a = InterfaceId::new([0x11; 8]);
    let b = InterfaceId::new([0x22; 8]);
    let c = InterfaceId::new([0x33; 8]);
    let marker = InterfaceId::new([0xee; 8]);
    let mut changed = descriptor(a);
    changed.hardware_mtu = Some(128);
    let cases = [
        (
            vec![],
            vec![a],
            InterfaceLifecycle::Add {
                descriptor: descriptor(a),
            },
            vec![descriptor(a)],
        ),
        (
            vec![descriptor(a)],
            vec![a],
            InterfaceLifecycle::Add {
                descriptor: changed,
            },
            vec![descriptor(a)],
        ),
        (
            vec![descriptor(b), descriptor(a)],
            vec![a, b],
            InterfaceLifecycle::Remove { id: a },
            vec![descriptor(b)],
        ),
        (
            vec![descriptor(a)],
            vec![a],
            InterfaceLifecycle::Remove { id: b },
            vec![descriptor(a)],
        ),
        (
            vec![descriptor(b), descriptor(a)],
            vec![a, b],
            InterfaceLifecycle::Update {
                descriptor: changed,
            },
            vec![descriptor(b), changed],
        ),
        (
            vec![descriptor(b)],
            vec![b],
            InterfaceLifecycle::Update {
                descriptor: changed,
            },
            vec![descriptor(b)],
        ),
        (
            vec![descriptor(a)],
            vec![a],
            InterfaceLifecycle::Retag {
                old_id: a,
                new_id: b,
                descriptor: descriptor(b),
            },
            vec![descriptor(b)],
        ),
        (
            vec![descriptor(a)],
            vec![a],
            InterfaceLifecycle::Retag {
                old_id: a,
                new_id: b,
                descriptor: descriptor(c),
            },
            vec![descriptor(a)],
        ),
        (
            vec![descriptor(a)],
            vec![a],
            InterfaceLifecycle::Retag {
                old_id: b,
                new_id: c,
                descriptor: descriptor(c),
            },
            vec![descriptor(a)],
        ),
        (
            vec![descriptor(a), descriptor(b)],
            vec![a, b],
            InterfaceLifecycle::Retag {
                old_id: a,
                new_id: b,
                descriptor: descriptor(b),
            },
            vec![descriptor(a), descriptor(b)],
        ),
    ];
    for (index, (initial, lanes, message, expected)) in cases.into_iter().enumerate() {
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut host = EmbassyHost::new(crate::manifold::driver::test_support::entropy_handle());
        let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
        let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
        let responses = Channel::<CriticalSectionRawMutex, ResourceResponse<0>, 1>::new();
        let completion = embassy_sync::signal::Signal::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 2>::new();
        let mut descriptors = HeaplessVec::<InterfaceDescriptor, 4>::new();
        descriptors.extend_from_slice(&initial).unwrap();
        descriptors.push(descriptor(marker)).unwrap();
        let mut egress = PooledEgress::<4>::new();
        for id in lanes.into_iter().chain([marker]) {
            let (producer, _) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
            assert!(egress
                .push(id, std::boxed::Box::leak(std::boxed::Box::new(producer)))
                .is_ok());
        }
        let mut inbound = HeaplessVec::new();
        let mut ifacs = HeaplessVec::new();
        let mut persistence = NoManifoldPersistence;
        block_on(async {
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
                    resource_responses: responses.receiver(),
                    lifecycle: lifecycle.receiver(),
                },
                |_| {},
                crate::manifold::decline_all(),
                &NoInterfaceInspectionStore,
                &mut persistence,
            );
            let driver = async {
                lifecycle.send(message).await;
                lifecycle
                    .send(InterfaceLifecycle::Publish {
                        old_id: marker,
                        descriptor: descriptor(marker),
                        completion: &completion,
                    })
                    .await;
                assert_eq!(
                    completion.wait().await,
                    InterfacePublicationOutcome::Published
                );
            };
            match select(manifold, with_timeout(WATCHDOG, driver)).await {
                Either::First(()) => panic!("manifold stopped"),
                Either::Second(result) => result.unwrap(),
            }
        });
        let observed = descriptors
            .iter()
            .copied()
            .filter(|value| value.id != marker)
            .collect::<Vec<_>>();
        assert_eq!(observed, expected, "lifecycle case {index}");
    }
}

struct ManualHost(Rc<Cell<u64>>);

impl crate::manifold::Host for ManualHost {
    fn now(&self) -> InstantMillis {
        InstantMillis(self.0.get())
    }
    async fn sleep_until(&self, deadline: InstantMillis) {
        core::future::poll_fn(|_| {
            if deadline.0 <= self.0.get() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
        .await;
    }
    fn fill_random(&mut self, bytes: &mut [u8]) {
        bytes.fill(0x41);
    }
}

fn poll_pending(future: core::pin::Pin<&mut impl core::future::Future<Output = ()>>) {
    use core::task::{Context, Poll, Waker};
    assert_eq!(
        future.poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    );
}

#[test]
fn lifecycle_changes_preserve_or_replace_each_lanes_announce_pacer() {
    use crate::engine::test_support::{personal_node_announcer, personal_node_destination};
    use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, PrnsCommand};
    use crate::interfaces::{AnnounceBandwidthCap, BitrateBps};
    use crate::manifold::grant::GrantConsumer;
    let a = InterfaceId::new([0xA1; 8]);
    let b = InterfaceId::new([0xB2; 8]);
    let c = InterfaceId::new([0xC3; 8]);
    let paced = |id| InterfaceDescriptor {
        bitrate: BitrateBps::guess(1_000),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        ..descriptor(id)
    };
    for operation in 0..8 {
        let mut engine = personal_node_announcer();
        let clock = Rc::new(Cell::new(1_000));
        let mut host = ManualHost(clock.clone());
        let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
        let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
        let responses = Channel::<CriticalSectionRawMutex, ResourceResponse<0>, 1>::new();
        let completed = embassy_sync::signal::Signal::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let mut descriptors = HeaplessVec::<InterfaceDescriptor, 2>::new();
        descriptors.push(paced(a)).unwrap();
        if operation != 1 {
            descriptors.push(paced(b)).unwrap();
        }
        let mut egress = PooledEgress::<2>::new();
        let (a_tx, mut a_rx) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
        let (b_tx, mut b_rx) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
        assert!(egress.push(a, Box::leak(Box::new(a_tx))).is_ok());
        assert!(egress.push(b, Box::leak(Box::new(b_tx))).is_ok());
        let mut inbound = HeaplessVec::new();
        let mut ifacs = HeaplessVec::new();
        let mut persistence = NoManifoldPersistence;
        let mut manifold = Box::pin(run_pooled(
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
                resource_responses: responses.receiver(),
                lifecycle: lifecycle.receiver(),
            },
            |_| {},
            crate::manifold::decline_all(),
            &NoInterfaceInspectionStore,
            &mut persistence,
        ));
        poll_pending(manifold.as_mut());
        if operation == 1 {
            assert!(lifecycle
                .try_send(InterfaceLifecycle::Add {
                    descriptor: paced(b)
                })
                .is_ok());
            poll_pending(manifold.as_mut());
        }
        for (sequence, id) in [a, b, a, b].into_iter().enumerate() {
            commands
                .try_send(IssuedCommand {
                    id: CommandId(sequence as u64),
                    command: PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: personal_node_destination(),
                        target: AnnounceTarget::Interface(id),
                        app_data: AnnounceAppData::Registered,
                    }),
                })
                .unwrap();
            poll_pending(manifold.as_mut());
        }
        assert!(a_rx.try_peek().is_some(), "first A, operation {operation}");
        GrantConsumer::release(&mut a_rx);
        assert!(b_rx.try_peek().is_some(), "first B, operation {operation}");
        GrantConsumer::release(&mut b_rx);
        assert!(a_rx.try_peek().is_none(), "A pacing, operation {operation}");
        assert!(b_rx.try_peek().is_none(), "B pacing, operation {operation}");
        let message = match operation {
            0 | 1 => InterfaceLifecycle::Add {
                descriptor: paced(b),
            },
            2 => InterfaceLifecycle::Update {
                descriptor: descriptor(b),
            },
            3 => InterfaceLifecycle::Remove { id: b },
            4 => InterfaceLifecycle::Publish {
                old_id: b,
                descriptor: paced(c),
                completion: &completed,
            },
            5 => InterfaceLifecycle::Retag {
                old_id: b,
                new_id: c,
                descriptor: paced(c),
            },
            6 => InterfaceLifecycle::Publish {
                old_id: b,
                descriptor: paced(a),
                completion: &completed,
            },
            7 => InterfaceLifecycle::Update {
                descriptor: descriptor(c),
            },
            _ => unreachable!(),
        };
        assert!(lifecycle.try_send(message).is_ok());
        poll_pending(manifold.as_mut());
        if operation == 4 || operation == 6 {
            assert_eq!(
                completed.try_take(),
                Some(if operation == 4 {
                    super::InterfacePublicationOutcome::Published
                } else {
                    super::InterfacePublicationOutcome::IdentityConflict
                })
            );
        }
        clock.set(1_000_000);
        poll_pending(manifold.as_mut());
        assert!(
            a_rx.try_peek().is_some(),
            "unrelated A retained, operation {operation}"
        );
        assert_eq!(
            b_rx.try_peek().is_some(),
            matches!(operation, 0 | 1 | 6 | 7),
            "B queue lifecycle, operation {operation}"
        );
    }
}

#[test]
fn path_table_buffer_covers_eight_worst_case_entries_with_its_fixed_memory_budget() {
    use crate::engine::RouteSnapshot;
    use crate::routing::routes::{NextHop, RouteRetention};
    use crate::wire::DestinationHash;
    let entry = RouteSnapshot {
        destination: DestinationHash::new([0xFF; 16]),
        hops: 255,
        via: NextHop::Via(TEST_TRANSPORT_ID),
        learned_at: InstantMillis(u64::MAX),
        last_route_activity_at: InstantMillis(u64::MAX),
        expires_at: InstantMillis(u64::MAX),
        interface: InterfaceId::new([0xFF; 8]),
        retention: RouteRetention::Network,
    };
    let entries = [
        entry.clone(),
        entry.clone(),
        entry.clone(),
        entry.clone(),
        entry.clone(),
        entry.clone(),
        entry.clone(),
        entry,
    ];
    let mut buffer = [0; super::RNS_PATH_TABLE_RESPONSE_BYTES];
    let written =
        crate::interfaces::rns_management::write_route_snapshots(&entries, &mut buffer[19..])
            .unwrap();
    assert_eq!(buffer[19], 0x98);
    assert!(written > 8 * 100);
    // 19-byte response envelope, array tag, eight 133-byte maximum maps.
    assert_eq!(buffer.len(), 1084);
}

#[test]
fn pooled_ingress_retains_live_counts_and_phy_and_rejects_bad_ifac_envelopes() {
    use crate::engine::{CommandId, FanTarget};
    use crate::interfaces::{
        ConnectionState, IfacContext, IfacSize, InterfaceStatus, PacketPhyStats, RssiDbm,
    };
    use crate::manifold::driver::EmbassyInterfaceStatus;
    use crate::routing::links::LinkId;
    use crate::runtime::EmbassyInterfaceStore;
    let id = InterfaceId::new([0x51; 8]);
    let network = IfacContext::derive(Some("lora-test"), Some("shared"), IfacSize::NARROW).unwrap();
    let raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    let packet_hash = crate::routing::dedup::PacketHash::of_wire_packet(&raw).unwrap();
    let phy = PacketPhyStats {
        rssi: Some(RssiDbm::new(-91)),
        ..PacketPhyStats::default()
    };
    let mut masked = [0; EMBEDDED_MAX_WIRE_FRAME_LEN];
    let len = network.mask_outbound(&raw, &mut masked).unwrap();
    let store = EmbassyInterfaceStore::<CriticalSectionRawMutex, 2, 8, 16>::new();
    let status = Box::leak(Box::new(EmbassyInterfaceStatus::new_accounted(
        id,
        ConnectionState::Connected,
    )));
    let statuses = [&*status];
    let mut engine = EngineState::<GrowableHeap>::default();
    let mut host = ManualHost(Rc::new(Cell::new(1_000)));
    let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let responses = Channel::<CriticalSectionRawMutex, ResourceResponse<0>, 1>::new();
    let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
    let mut descriptors = HeaplessVec::<InterfaceDescriptor, 1>::new();
    descriptors.push(descriptor(id)).unwrap();
    let mut ifacs = HeaplessVec::new();
    assert!(ifacs
        .push(InterfaceIfac {
            id,
            context: network
        })
        .is_ok());
    let (mut tx, rx) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
    let mut inbound = HeaplessVec::new();
    assert!(inbound
        .push((
            id,
            Box::leak(Box::new(rx)) as &'static mut dyn ManifoldLaneReader
        ))
        .is_ok());
    let mut egress = PooledEgress::<1>::new();
    let mut persistence = NoManifoldPersistence;
    let mut manifold = Box::pin(run_pooled(
        &mut engine,
        &mut host,
        PooledWiring {
            descriptors: &mut descriptors,
            ifacs: &mut ifacs,
            inbound: &mut inbound,
            frame_accounting_statuses: &statuses,
            egress: &mut egress,
            notify: notify.receiver(),
            commands: commands.receiver(),
            resource_responses: responses.receiver(),
            lifecycle: lifecycle.receiver(),
        },
        |_| {},
        crate::manifold::decline_all(),
        &store,
        &mut persistence,
    ));
    poll_pending(manifold.as_mut());
    for case in 0..5 {
        let slot = tx.try_grant().unwrap();
        match case {
            0 => slot.fill_for_fan(FanTarget::All, &raw),
            1 => slot.fill_for(id, &[0]),
            2 => slot.fill_for(id, &raw),
            3 => {
                let mut corrupt = masked;
                corrupt[len - 1] ^= 1;
                slot.fill_for(id, &corrupt[..len]);
            }
            4 => {
                slot.fill_for(id, &masked[..len]);
                slot.packet_phy = phy;
            }
            _ => unreachable!(),
        }
        tx.commit();
        notify.try_send(id).unwrap();
        poll_pending(manifold.as_mut());
        assert_eq!(store.counts(id).destinations, u32::from(case == 4));
    }
    assert_eq!(store.packet_phy(packet_hash), Some(phy));
    assert!(responses
        .try_send(ResourceResponse {
            id: CommandId(9),
            link_id: LinkId::new([0x11; 16]),
            request_id: RequestId([0x22; 16]),
            payload: ResourceResponsePayload::RnsPathTable(RnsRemotePathTableRequest::new(
                None, None
            )),
        })
        .is_ok());
    poll_pending(manifold.as_mut());

    assert_eq!(status.frame_accounting().unwrap().protocol_violations, 1);
    assert!(responses
        .try_send(ResourceResponse {
            id: CommandId(10),
            link_id: LinkId::new([0x11; 16]),
            request_id: RequestId([0x22; 16]),
            payload: ResourceResponsePayload::Ready(HeaplessVec::new())
        })
        .is_ok());
    poll_pending(manifold.as_mut());
    assert!(lifecycle
        .try_send(InterfaceLifecycle::Remove { id })
        .is_ok());
    poll_pending(manifold.as_mut());
    assert_eq!(store.counts(id).destinations, 0);
    drop(manifold);
    assert!(descriptors.is_empty());
}

#[test]
fn publication_refreshes_route_expiry_before_acknowledging_the_new_descriptor() {
    use crate::interfaces::InterfaceMode;
    use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
    use crate::runtime::EmbassyInterfaceStore;
    for operation in 0..3 {
        let id = InterfaceId::new([0x61; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let clock = Rc::new(Cell::new(1_000));
        let mut host = ManualHost(clock.clone());
        let notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
        let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
        let responses = Channel::<CriticalSectionRawMutex, ResourceResponse<0>, 1>::new();
        let completed = embassy_sync::signal::Signal::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let mut descriptors = HeaplessVec::<InterfaceDescriptor, 1>::new();
        descriptors.push(descriptor(id)).unwrap();
        let (mut tx, rx) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
        let (out_tx, _) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(1);
        let mut inbound = HeaplessVec::new();
        assert!(inbound
            .push((
                id,
                Box::leak(Box::new(rx)) as &'static mut dyn ManifoldLaneReader
            ))
            .is_ok());
        let mut egress = PooledEgress::<1>::new();
        assert!(egress.push(id, Box::leak(Box::new(out_tx))).is_ok());
        let mut ifacs = HeaplessVec::new();
        let store = EmbassyInterfaceStore::<CriticalSectionRawMutex, 2, 8, 16>::new();
        let removed = Cell::new(0);
        let mut persistence = NoManifoldPersistence;
        let mut manifold = Box::pin(run_pooled(
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
                resource_responses: responses.receiver(),
                lifecycle: lifecycle.receiver(),
            },
            |journaled| {
                if matches!(journaled, Journaled::RouteRemoved { .. }) {
                    removed.set(removed.get() + 1);
                }
            },
            crate::manifold::decline_all(),
            &store,
            &mut persistence,
        ));
        poll_pending(manifold.as_mut());
        tx.try_grant()
            .unwrap()
            .fill_for(id, &bytes_from_hex(RNS_1_4_2_ANNOUNCE));
        tx.commit();
        notify.try_send(id).unwrap();
        poll_pending(manifold.as_mut());
        assert_eq!(store.counts(id).destinations, 1);
        let descriptor = InterfaceDescriptor {
            mode: InterfaceMode::Roaming,
            ..descriptor(id)
        };
        let message = match operation {
            0 => InterfaceLifecycle::Publish {
                old_id: id,
                descriptor,
                completion: &completed,
            },
            1 => InterfaceLifecycle::Retag {
                old_id: id,
                new_id: id,
                descriptor,
            },
            2 => InterfaceLifecycle::Update { descriptor },
            _ => unreachable!(),
        };
        assert!(lifecycle.try_send(message).is_ok());
        poll_pending(manifold.as_mut());
        if operation == 0 {
            assert_eq!(
                completed.try_take(),
                Some(super::InterfacePublicationOutcome::Published)
            );
        }
        clock.set(999 + ROAMING_ROUTE_EXPIRY_MILLIS);
        poll_pending(manifold.as_mut());
        assert_eq!(store.counts(id).destinations, 1);
        clock.set(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS);
        poll_pending(manifold.as_mut());
        assert_eq!(store.counts(id).destinations, 0, "operation {operation}");
        assert_eq!(removed.get(), 1);
    }
}

#[test]
fn pooled_peers_establish_a_link_and_dispatch_resource_responses() {
    use crate::engine::test_support::{personal_node_announcer, personal_node_destination};
    use crate::engine::{
        CommandId, EstablishLink, PrnsCommand, SetRegisteredAnnounceAppData, Settlement,
    };
    use crate::routing::links::LinkId;
    let clock = Rc::new(Cell::new(1_000));
    let a_link = Cell::<Option<LinkId>>::new(None);
    let b_link = Cell::<Option<LinkId>>::new(None);
    let settlements = RefCell::new(Vec::new());
    let closed = Cell::new(0);
    let mut a_engine = EngineState::<TestStorageLayout>::default();
    let mut b_engine = personal_node_announcer();
    let a_id = InterfaceId::new([0xA1; 8]);
    let mut a_host = ManualHost(clock.clone());
    let a_notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let a_commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let a_responses = Channel::<CriticalSectionRawMutex, ResourceResponse<64>, 1>::new();
    let a_lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
    let mut a_descriptors = HeaplessVec::<InterfaceDescriptor, 1>::new();
    a_descriptors.push(descriptor(a_id)).unwrap();
    let (mut a_in, a_reader) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let (a_writer, mut a_out) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let mut a_inbound = HeaplessVec::new();
    assert!(a_inbound
        .push((
            a_id,
            Box::leak(Box::new(a_reader)) as &'static mut dyn ManifoldLaneReader
        ))
        .is_ok());
    let mut a_egress = PooledEgress::<1>::new();
    assert!(a_egress.push(a_id, Box::leak(Box::new(a_writer))).is_ok());
    let mut a_ifacs = HeaplessVec::new();
    let mut a_persistence = NoManifoldPersistence;
    let mut a_run = Box::pin(run_pooled(
        &mut a_engine,
        &mut a_host,
        PooledWiring {
            descriptors: &mut a_descriptors,
            ifacs: &mut a_ifacs,
            inbound: &mut a_inbound,
            frame_accounting_statuses: &[],
            egress: &mut a_egress,
            notify: a_notify.receiver(),
            commands: a_commands.receiver(),
            resource_responses: a_responses.receiver(),
            lifecycle: a_lifecycle.receiver(),
        },
        |event| match event {
            Journaled::LinkEstablished(link) => a_link.set(Some(link.link_id)),
            Journaled::CommandSettled { id, settlement } => {
                if let Settlement::EstablishLink(Ok(link)) = &settlement {
                    a_link.set(Some(link.link_id));
                }
                settlements.borrow_mut().push((id, settlement));
            }
            Journaled::LinkClosed { .. } => closed.set(closed.get() + 1),
            _ => {}
        },
        crate::manifold::decline_all(),
        &NoInterfaceInspectionStore,
        &mut a_persistence,
    ));
    let b_id = InterfaceId::new([0xB2; 8]);
    let mut b_host = ManualHost(clock.clone());
    let b_notify = Channel::<CriticalSectionRawMutex, InterfaceId, 1>::new();
    let b_commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let b_responses = Channel::<CriticalSectionRawMutex, ResourceResponse<64>, 1>::new();
    let b_lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
    let mut b_descriptors = HeaplessVec::<InterfaceDescriptor, 1>::new();
    b_descriptors.push(descriptor(b_id)).unwrap();
    let (mut b_in, b_reader) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let (b_writer, mut b_out) = leaked_grant_lane::<EMBEDDED_MAX_WIRE_FRAME_LEN>(2);
    let mut b_inbound = HeaplessVec::new();
    assert!(b_inbound
        .push((
            b_id,
            Box::leak(Box::new(b_reader)) as &'static mut dyn ManifoldLaneReader
        ))
        .is_ok());
    let mut b_egress = PooledEgress::<1>::new();
    assert!(b_egress.push(b_id, Box::leak(Box::new(b_writer))).is_ok());
    let mut b_ifacs = HeaplessVec::new();
    let mut b_persistence = NoManifoldPersistence;
    let mut b_run = Box::pin(run_pooled(
        &mut b_engine,
        &mut b_host,
        PooledWiring {
            descriptors: &mut b_descriptors,
            ifacs: &mut b_ifacs,
            inbound: &mut b_inbound,
            frame_accounting_statuses: &[],
            egress: &mut b_egress,
            notify: b_notify.receiver(),
            commands: b_commands.receiver(),
            resource_responses: b_responses.receiver(),
            lifecycle: b_lifecycle.receiver(),
        },
        |event| match event {
            Journaled::LinkEstablished(link) => b_link.set(Some(link.link_id)),
            Journaled::CommandSettled { id, settlement } => {
                if let Settlement::EstablishLink(Ok(link)) = &settlement {
                    b_link.set(Some(link.link_id));
                }
                settlements.borrow_mut().push((id, settlement));
            }
            Journaled::LinkClosed { .. } => closed.set(closed.get() + 1),
            _ => {}
        },
        crate::manifold::decline_all(),
        &NoInterfaceInspectionStore,
        &mut b_persistence,
    ));
    b_commands
        .try_send(IssuedCommand {
            id: CommandId(0),
            command: PrnsCommand::AnnounceNow(crate::engine::AnnounceNow {
                destination: personal_node_destination(),
                target: crate::engine::AnnounceTarget::AllInterfaces,
                app_data: crate::engine::AnnounceAppData::Registered,
            }),
        })
        .unwrap();
    {
        let mut pump = || {
            for _ in 0..32 {
                clock.set(clock.get() + 10);
                poll_pending(a_run.as_mut());
                poll_pending(b_run.as_mut());
                let mut moved = false;
                while let Some((_, _, frame)) = a_out.try_read() {
                    b_in.try_grant().unwrap().fill_for(b_id, frame);
                    b_in.commit();
                    a_out.release();
                    let _ = b_notify.try_send(b_id);
                    moved = true;
                }
                while let Some((_, _, frame)) = b_out.try_read() {
                    a_in.try_grant().unwrap().fill_for(a_id, frame);
                    a_in.commit();
                    b_out.release();
                    let _ = a_notify.try_send(a_id);
                    moved = true;
                }
                if !moved {
                    return;
                }
            }
            panic!("linked peers did not settle within 32 exchanges");
        };
        pump();
        a_commands
            .try_send(IssuedCommand {
                id: CommandId(1),
                command: PrnsCommand::EstablishLink(EstablishLink {
                    destination: personal_node_destination(),
                }),
            })
            .unwrap();
        pump();
        assert!(
            a_link.get().is_some(),
            "settlements: {:?}",
            settlements.borrow()
        );
        assert_eq!(a_link.get(), b_link.get());
        let mut data = HeaplessVec::new();
        data.extend_from_slice(b"radio response").unwrap();
        assert!(b_responses
            .try_send(ResourceResponse {
                id: CommandId(2),
                link_id: b_link.get().unwrap(),
                request_id: RequestId([0x31; 16]),
                payload: ResourceResponsePayload::Ready(data)
            })
            .is_ok());
        pump();
        a_commands
            .try_send(IssuedCommand {
                id: CommandId(3),
                command: PrnsCommand::SetRegisteredAnnounceAppData(SetRegisteredAnnounceAppData {
                    destination: personal_node_destination(),
                    app_data: HeaplessVec::new(),
                }),
            })
            .unwrap();
        pump();
        assert!(settlements
            .borrow()
            .iter()
            .any(|(id, result)| *id == CommandId(3)
                && matches!(result, Settlement::SetRegisteredAnnounceAppData(Err(_)))));
        clock.set(1_000_000);
        pump();
    }
    for now in [2_000_000, 3_000_000, 4_000_000] {
        clock.set(now);
        poll_pending(a_run.as_mut());
        while a_out.try_read().is_some() {
            a_out.release();
        }
    }
    assert!(closed.get() > 0, "a peer that stops responding must expire");
}
