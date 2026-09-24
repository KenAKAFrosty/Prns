use super::*;
use crate::remote_control::RemoteControlRequestKind;

#[derive(Clone, Copy, Debug)]
enum Step {
    Erase(usize),
    Grants,
    Groups,
}

impl Step {
    fn phase(self) -> CompactionPhase {
        match self {
            Self::Erase(sector) => CompactionPhase::Erase { sector },
            Self::Grants => CompactionPhase::AuthorizationSnapshot(
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
            ),
            Self::Groups => CompactionPhase::DiscoveryGroupConfigurations,
        }
    }
}

fn grant_snapshot(request: RemoteControlRequestKind) -> RemoteControlAuthorizationSnapshot {
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
    let snapshot = grant_snapshot(RemoteControlRequestKind::Describe);
    let mut full = false;
    for _ in 0..MAX_PROGRESS_STEPS {
        match journal
            .append(
                FlashJournalRecordKind::RemoteControlControllerGrants,
                &snapshot,
            )
            .await
        {
            Ok(()) => {}
            Err(FlashJournalError::ArenaFull) => {
                full = true;
                break;
            }
            Err(error) => panic!("fixture append: {error:?}"),
        }
    }
    assert!(full);
    let mut image = journal.release().bytes;
    image[4 * ERASE..].fill(0);
    image
}

async fn exercise(image: [u8; CAPACITY], step: Step, cut: Option<Cut>) -> Vec<Operation> {
    let groups = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let policy = EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
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
        if owner.compaction == Some(step.phase()) {
            break;
        }
        owner.progress_compaction(&engine, WRITE_TIME).await;
    }
    assert_eq!(owner.compaction, Some(step.phase()));
    let allowed = owner.next_compaction_not_before.unwrap();
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    if cut.is_some() {
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        let mut pending = core::pin::pin!(owner.progress_compaction(&engine, WRITE_TIME));
        for _ in 0..2 {
            assert!(core::future::Future::poll(pending.as_mut(), &mut context).is_pending());
        }
    } else {
        owner.progress_compaction(&engine, WRITE_TIME).await;
    }
    let trace = control.borrow().trace.clone();
    if cut.is_some() {
        control.borrow_mut().arm(None);
        owner.progress_compaction(&engine, WRITE_TIME).await;
        assert!(
            control
                .borrow()
                .trace
                .iter()
                .all(|op| matches!(op, Operation::Read { .. })),
            "cancellation cannot repeat writes or erases inside the same wear budget"
        );
        assert_eq!(owner.compaction, None);
        assert_eq!(owner.next_compaction_not_before, Some(allowed));
        owner.try_start_compaction(&engine, InstantMillis(allowed.0 - 1));
        assert_eq!(owner.compaction, None);
        assert!(control.borrow().trace.is_empty());
    }
    assert_eq!(
        owner.remote_control_controller_grants_snapshot,
        Some(grant_snapshot(RemoteControlRequestKind::Describe))
    );
    assert_eq!(
        groups.restored_now(),
        Some(discovery_group_snapshot("confirmed"))
    );
    let candidate = grant_snapshot(RemoteControlRequestKind::AnnounceSelf);
    let at = if cut.is_some() { allowed } else { WRITE_TIME };
    let mut stored = false;
    for _ in 0..MAX_PROGRESS_STEPS {
        control.borrow_mut().arm(None);
        match owner
            .store_remote_control_authorization_snapshot(
                &engine,
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                &candidate,
                at,
            )
            .await
        {
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                stored = true;
                break;
            }
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {}
            other => panic!("compaction must recover: {other:?}"),
        }
    }
    assert!(stored);
    let persisted = owner.journal.take().unwrap().release().into_image();
    for _ in 0..2 {
        let restored = DiscoveryGroupConfigurationStoreExchange::new();
        control.borrow_mut().arm(None);
        let mut rebooted = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
            Flash::boot(persisted, control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &restored,
        );
        let mut fresh_engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut fresh_remote = available_remote_control(&mut fresh_engine);
        rebooted
            .restore(&mut fresh_engine, &mut fresh_remote, InstantMillis(0))
            .await;
        assert_eq!(controller_grants_snapshot(&fresh_remote), candidate);
        assert_eq!(
            restored.restored_now(),
            Some(discovery_group_snapshot("confirmed"))
        );
    }
    trace
}

async fn campaign(steps: &[Step], partition: FaultPartition) {
    let image = image().await;
    let mut cases = 0;
    let mut boundary = 0;
    for &step in steps {
        let trace = exercise(image, step, None).await;
        assert!(!trace.is_empty());
        for (operation, event) in trace.iter().enumerate() {
            let prefixes: Vec<_> = match event {
                Operation::Read { len, .. } => std::vec![0, *len],
                Operation::Write { len, .. } | Operation::Erase { len, .. } => (0..=*len).collect(),
            };
            for completed_bytes in prefixes {
                let selected = partition.includes(boundary);
                boundary += 1;
                if !selected {
                    continue;
                }
                let observed = exercise(
                    image,
                    step,
                    Some(Cut {
                        operation,
                        completed_bytes,
                    }),
                )
                .await;
                assert_eq!(observed, trace[..=operation]);
                cases += 1;
            }
        }
    }
    assert!(cases > 0);
    std::eprintln!("verified {cases} cancellation boundaries for {steps:?}");
}

#[test]
fn cancelled_completed_erase_cannot_consume_a_second_erase() {
    embassy_futures::block_on(async {
        exercise(
            image().await,
            Step::Erase(0),
            Some(Cut {
                operation: 0,
                completed_bytes: ERASE,
            }),
        )
        .await;
    });
}

partitioned_campaign!(
    cancelled_arena_erases_wait_for_a_new_wear_budget,
    arena_erase_campaign,
);

fn arena_erase_campaign(partition: FaultPartition) {
    embassy_futures::block_on(campaign(&[Step::Erase(0), Step::Erase(1)], partition));
}

#[test]
fn cancelled_grant_copies_recover_without_reprogramming_the_tail() {
    embassy_futures::block_on(campaign(&[Step::Grants], FaultPartition::All));
}

#[test]
fn cancelled_group_copies_recover_without_reprogramming_the_tail() {
    embassy_futures::block_on(campaign(&[Step::Groups], FaultPartition::All));
}
