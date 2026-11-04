use super::*;
use crate::remote_control::RemoteControlRequestKind;

fn policy() -> EmbeddedPersistencePolicy {
    EmbeddedPersistencePolicy::hopspot_default(
        EmbeddedCompactionPolicy::hopspot(0).with_route_replay_compaction_threshold(2),
    )
}

fn grants_for(request: RemoteControlRequestKind) -> RemoteControlAuthorizationSnapshot {
    grants::snapshot(&[crate::runtime::node_facade::test_remote_control_grant(
        request,
    )])
}

async fn image() -> [u8; CAPACITY] {
    let mut flash = TestFlash::new();
    flash.bytes = baseline(Campaign::Append).await;
    let (mut journal, _) = FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |_| {})
        .await
        .unwrap();
    let mut payload = [0; RECORD_SCRATCH_LEN];
    let len = encode_route_upsert(signed_route(7, b"saved"), &mut payload).unwrap();
    for _ in 0..2 {
        journal
            .append(FlashJournalRecordKind::RouteUpsert, &payload[..len])
            .await
            .unwrap();
    }
    journal
        .append(
            FlashJournalRecordKind::RemoteControlControllerGrants,
            &grants_for(RemoteControlRequestKind::Describe),
        )
        .await
        .unwrap();
    journal.release().bytes
}

async fn compact(image: [u8; CAPACITY], cut: Option<Cut>) -> ([u8; CAPACITY], Vec<Operation>) {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()), LAYOUT, policy(), FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    let report = owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert_eq!(report.route_seeded_count, 2);
    assert!(owner.replay_compaction_requested);
    assert!(owner.pending_routes.is_empty());
    assert!(!owner.snapshot_required);
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    let mut completed = false;
    for _ in 0..MAX_PROGRESS_STEPS {
        let progress = {
            let mut progress = core::pin::pin!(owner.progress(&mut engine, report.logical_start));
            core::future::Future::poll(progress.as_mut(), &mut context)
        };
        if progress.is_pending() {
            assert!(cut.is_some() && control.borrow().power_removed());
            break;
        }
        if owner.compaction.is_none() && !owner.replay_compaction_requested {
            completed = true;
            break;
        }
        assert_eq!(
            owner.route_replay_records, 2,
            "uncommitted copy cannot reset replay work"
        );
    }
    assert_eq!(completed, cut.is_none());
    if completed {
        assert_eq!(owner.route_replay_records, 1);
    }
    let image = owner.journal.take().unwrap().release().into_image();
    let trace = control.borrow().trace.clone();
    (image, trace)
}

async fn verify_reboot(image: [u8; CAPACITY]) {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let mut flash = TestFlash::new();
    flash.bytes = image;
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        flash, LAYOUT, policy(), FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    let report = owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert!(matches!(report.route_seeded_count, 1 | 2));
    assert_eq!(
        (report.route_refused_count, report.route_dropped_count),
        (0, 0)
    );
    assert_eq!(report.warning, None);
    assert_eq!(engine.persisted_route_destinations().count(), 1);
    assert_eq!(
        engine
            .persisted_route_row(&signed_route(7, b"saved").destination)
            .unwrap()
            .app_data,
        b"saved"
    );
    assert_eq!(
        controller_grants_snapshot(&remote),
        grants_for(RemoteControlRequestKind::Describe)
    );
    assert_eq!(
        groups.restored_now(),
        Some(discovery_group_snapshot("confirmed"))
    );
    assert_eq!(owner.route_replay_records, report.route_seeded_count);
    assert_eq!(
        owner.replay_compaction_requested,
        report.route_seeded_count == 2
    );
}

#[test]
fn idle_replay_compaction_preserves_routes_and_critical_state_at_every_torn_io_boundary() {
    embassy_futures::block_on(async {
        let image = image().await;
        let (completed, trace) = compact(image, None).await;
        verify_reboot(completed).await;
        let mut cases = 0;
        for (operation, event) in trace.iter().enumerate() {
            let prefixes: Vec<_> = match event {
                Operation::Read { len, .. } => std::vec![0, *len],
                Operation::Write { len, .. } | Operation::Erase { len, .. } => (0..=*len).collect(),
            };
            for completed_bytes in prefixes {
                let (interrupted, _) = compact(
                    image,
                    Some(Cut {
                        operation,
                        completed_bytes,
                    }),
                )
                .await;
                verify_reboot(interrupted).await;
                cases += 1;
            }
        }
        assert!(cases > 1_000);
        std::eprintln!("verified {cases} idle replay compaction power-loss boundaries");
    });
}

#[test]
fn authorization_arriving_during_replay_compaction_settles_only_after_the_replacement_and_append() {
    embassy_futures::block_on(async {
        let groups = DiscoveryGroupConfigurationStoreExchange::new();
        let control = Rc::new(RefCell::new(Control::new()));
        let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
            Flash::boot(image().await, control.clone()), LAYOUT, policy(), FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote = available_remote_control(&mut engine);
        let report = owner
            .restore(&mut engine, &mut remote, InstantMillis(0))
            .await;
        owner.progress(&mut engine, report.logical_start).await;
        assert!(owner.compaction.is_some());
        let previous = grants_for(RemoteControlRequestKind::Describe);
        let candidate = grants_for(RemoteControlRequestKind::AnnounceSelf);
        let mut stored = false;
        for _ in 0..MAX_PROGRESS_STEPS {
            control.borrow_mut().arm(None);
            match owner
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &candidate,
                    report.logical_start,
                )
                .await
            {
                StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {
                    assert_eq!(
                        owner.remote_control_controller_grants_snapshot,
                        Some(previous.clone())
                    );
                }
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                    assert_eq!(owner.compaction, None);
                    assert_eq!(owner.route_replay_records, 1);
                    stored = true;
                    break;
                }
                other => panic!("unexpected authorization outcome: {other:?}"),
            }
        }
        assert!(stored);
        let mut flash = TestFlash::new();
        flash.bytes = owner.journal.take().unwrap().release().into_image();
        let mut rebooted = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
            flash, LAYOUT, policy(), FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote = available_remote_control(&mut engine);
        rebooted
            .restore(&mut engine, &mut remote, InstantMillis(0))
            .await;
        assert_eq!(controller_grants_snapshot(&remote), candidate);
        assert_eq!(
            groups.restored_now(),
            Some(discovery_group_snapshot("confirmed"))
        );
        assert_eq!(engine.persisted_route_destinations().count(), 1);
    });
}
