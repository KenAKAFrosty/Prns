use super::*;
use crate::persistence::FlashJournalCommitResolution;
use crate::remote_control::RemoteControlRequestKind;

async fn exercise(cut: Option<Cut>, resolution: FlashJournalCommitResolution) -> Vec<Operation> {
    let grant = crate::runtime::node_facade::test_remote_control_grant;
    let prior = grant(RemoteControlRequestKind::Describe);
    let candidate = grant(RemoteControlRequestKind::AnnounceSelf);
    let baseline = grants::image(&grants::snapshot(&[prior]), Campaign::CompactThenAppend).await;
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let policy = EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(baseline, control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
        (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    owner.require_snapshot(EmbeddedPersistenceTarget::CriticalState, WRITE_TIME);
    owner.try_start_compaction(&engine, WRITE_TIME);
    for _ in 0..MAX_PROGRESS_STEPS {
        if owner.compaction == Some(CompactionPhase::Commit) {
            break;
        }
        owner.progress_compaction(&engine, WRITE_TIME).await;
    }
    assert_eq!(owner.compaction, Some(CompactionPhase::Commit));
    let allowed = owner.next_compaction_not_before.unwrap();
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    if cut.is_some() {
        let mut commit = core::pin::pin!(owner.progress_compaction(&engine, WRITE_TIME));
        for _ in 0..2 {
            assert!(core::future::Future::poll(commit.as_mut(), &mut context).is_pending());
        }
    } else {
        owner.progress_compaction(&engine, WRITE_TIME).await;
    }
    let trace = control.borrow().trace.clone();
    let resumed_at = if cut.is_some() {
        InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis)
    } else {
        WRITE_TIME
    };
    if cut.is_some() {
        control.borrow_mut().remove_power_at(Cut {
            operation: 0,
            completed_bytes: 0,
        });
        {
            let mut confirm = core::pin::pin!(owner.progress_compaction(&engine, WRITE_TIME));
            assert!(core::future::Future::poll(confirm.as_mut(), &mut context).is_pending());
        }
        control.borrow_mut().arm(Some(Cut {
            operation: 0,
            completed_bytes: 0,
        }));
        owner.progress_compaction(&engine, WRITE_TIME).await;
        assert!(matches!(
            owner.compaction,
            Some(CompactionPhase::ConfirmCommit { .. })
        ));
        assert_eq!(owner.retry_not_before, Some(resumed_at));
        let before = control.borrow().trace.clone();
        owner
            .progress(&mut engine, InstantMillis(resumed_at.0 - 1))
            .await;
        assert_eq!(control.borrow().trace, before);
        control.borrow_mut().arm(None);
        owner.progress_compaction(&engine, resumed_at).await;
        assert!(
            !control.borrow().trace.is_empty(),
            "cancelled commit must be read back"
        );
        assert!(control
            .borrow()
            .trace
            .iter()
            .all(|op| matches!(op, Operation::Read { .. })));
    }
    assert_eq!(owner.compaction, None);
    assert_eq!(
        owner.remote_control_controller_grants_snapshot,
        Some(grants::snapshot(&[prior]))
    );
    let projected = grants::snapshot(&[candidate]);
    match resolution {
        FlashJournalCommitResolution::Committed => {
            assert_eq!(owner.journal.as_ref().unwrap().active_epoch(), Some(1));
            assert_eq!(
                owner
                    .store_remote_control_authorization_snapshot(
                        &engine,
                        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                        &projected,
                        resumed_at
                    )
                    .await,
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored
            );
            if cut.is_some() {
                assert!(control
                    .borrow()
                    .trace
                    .iter()
                    .all(|op| !matches!(op, Operation::Erase { .. })));
            }
        }
        FlashJournalCommitResolution::NotCommitted => {
            assert_eq!(owner.journal.as_ref().unwrap().active_epoch(), Some(0));
            control.borrow_mut().arm(None);
            assert_eq!(
                owner
                    .store_remote_control_authorization_snapshot(
                        &engine,
                        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                        &projected,
                        InstantMillis(allowed.0 - 1)
                    )
                    .await,
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                    failure: EmbeddedPersistenceFailure::Capacity,
                    retry_at: Some(allowed),
                }
            );
            assert!(control.borrow().trace.is_empty());
            let mut stored = false;
            for _ in 0..MAX_PROGRESS_STEPS {
                match owner
                    .store_remote_control_authorization_snapshot(
                        &engine,
                        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                        &projected,
                        allowed,
                    )
                    .await
                {
                    StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                        stored = true;
                        break;
                    }
                    StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {}
                    other => panic!("retry must progress after cooldown: {other:?}"),
                }
            }
            assert!(stored);
        }
    }
    let image = owner.journal.take().unwrap().release().into_image();
    for _ in 0..2 {
        grants::restore(image, &[candidate]).await;
    }
    trace
}

#[test]
fn cancelled_compaction_commit_selects_the_committed_arena_before_new_appends() {
    embassy_futures::block_on(async {
        let trace = exercise(None, FlashJournalCommitResolution::Committed).await;
        let operation = trace
            .iter()
            .rposition(|op| matches!(op, Operation::Write { .. }))
            .unwrap();
        exercise(
            Some(Cut {
                operation,
                completed_bytes: 4,
            }),
            FlashJournalCommitResolution::Committed,
        )
        .await;
    });
}

#[test]
fn cancelled_compaction_commit_resolves_all_io_boundaries_and_preserves_cooldown() {
    embassy_futures::block_on(async {
        let trace = exercise(None, FlashJournalCommitResolution::Committed).await;
        let commit = trace
            .iter()
            .rposition(|op| matches!(op, Operation::Write { .. }))
            .unwrap();
        assert!(matches!(trace[commit], Operation::Write { len: 4, .. }));
        let mut cases = 0;
        for (operation, event) in trace.iter().enumerate() {
            let prefixes: Vec<_> = match event {
                Operation::Read { len, .. } => std::vec![0, *len],
                Operation::Write { len, .. } => (0..=*len).collect(),
                Operation::Erase { .. } => panic!("commit must not erase"),
            };
            for completed_bytes in prefixes {
                let resolution =
                    if operation > commit || (operation == commit && completed_bytes == 4) {
                        FlashJournalCommitResolution::Committed
                    } else {
                        FlashJournalCommitResolution::NotCommitted
                    };
                let observed = exercise(
                    Some(Cut {
                        operation,
                        completed_bytes,
                    }),
                    resolution,
                )
                .await;
                assert_eq!(observed, trace[..=operation]);
                cases += 1;
            }
        }
        std::eprintln!("verified {cases} compaction commit cancellation boundaries");
    });
}
