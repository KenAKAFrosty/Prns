use super::*;
use crate::persistence::FlashJournalCommitResolution;
use crate::remote_control::{RemoteControlControllerGrant, RemoteControlRequestKind};
use crate::runtime::remote_control_pairing_persistence::{
    RemoteControlAuthorizationStoreExchange, RemoteControlAuthorizationStoreRequirement,
    RemoteControlPairingManifoldPersistence,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

async fn attempt(
    baseline: [u8; CAPACITY],
    candidate: RemoteControlControllerGrant,
    cut: Option<Cut>,
    resolution: FlashJournalCommitResolution,
    requirement: RemoteControlAuthorizationStoreRequirement,
) -> ([u8; CAPACITY], Vec<Operation>) {
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
    let confirmed = controller_grants_snapshot(&remote);
    let projected = grants::snapshot(&[candidate]);
    let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
    stores.submit(
        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
        projected.clone(),
        requirement,
    );
    let mut manifold = RemoteControlPairingManifoldPersistence::new(&mut owner, &stores);
    ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, WRITE_TIME);
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    if cut.is_some() {
        {
            let mut progress = core::pin::pin!(manifold.progress(&mut engine, WRITE_TIME));
            for _ in 0..2 {
                assert!(core::future::Future::poll(progress.as_mut(), &mut context).is_pending());
                assert!(control.borrow().power_removed());
            }
        }
        let mut completion = core::pin::pin!(stores.next_completion());
        assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
    } else {
        manifold.progress(&mut engine, WRITE_TIME).await;
    }
    let trace = control.borrow().trace.clone();
    if cut.is_some() {
        control.borrow_mut().remove_power_at(Cut {
            operation: 0,
            completed_bytes: 0,
        });
        {
            let mut progress = core::pin::pin!(manifold.progress(&mut engine, WRITE_TIME));
            assert!(core::future::Future::poll(progress.as_mut(), &mut context).is_pending());
        }
        assert!(matches!(
            control.borrow().trace.as_slice(),
            [Operation::Read { .. }]
        ));
        control.borrow_mut().arm(None);
        manifold.progress(&mut engine, WRITE_TIME).await;
        assert!(
            control
                .borrow()
                .trace
                .iter()
                .all(|op| matches!(op, Operation::Read { .. })),
            "cancelled append must be resolved without writes or compaction: {cut:?}"
        );
    }
    let resolution = if resolution == FlashJournalCommitResolution::NotCommitted
        && requirement == RemoteControlAuthorizationStoreRequirement::Rollback
    {
        let retry = InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis);
        assert_eq!(
            ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(
                &mut manifold,
                WRITE_TIME
            ),
            Some(retry)
        );
        let before = control.borrow().trace.clone();
        manifold
            .progress(&mut engine, InstantMillis(retry.0 - 1))
            .await;
        assert_eq!(control.borrow().trace, before);
        {
            let mut completion = core::pin::pin!(stores.next_completion());
            assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
        }
        for _ in 0..MAX_PROGRESS_STEPS {
            manifold.progress(&mut engine, retry).await;
        }
        FlashJournalCommitResolution::Committed
    } else {
        resolution
    };
    let expected = match resolution {
        FlashJournalCommitResolution::Committed => Ok(()),
        FlashJournalCommitResolution::NotCommitted => Err(EmbeddedPersistenceFailure::Flash),
    };
    {
        let mut completion = core::pin::pin!(stores.next_completion());
        assert_eq!(
            core::future::Future::poll(completion.as_mut(), &mut context),
            core::task::Poll::Ready(expected),
            "{cut:?}"
        );
    }
    assert_eq!(controller_grants_snapshot(&remote), confirmed);
    drop(manifold);
    assert_eq!(
        owner.remote_control_controller_grants_snapshot.as_ref(),
        Some(match resolution {
            FlashJournalCommitResolution::Committed => &projected,
            FlashJournalCommitResolution::NotCommitted => &confirmed,
        })
    );
    (owner.journal.take().unwrap().release().into_image(), trace)
}

#[test]
fn cancelled_authorization_append_resolves_every_pending_io_before_other_writes() {
    embassy_futures::block_on(async {
        let grant = crate::runtime::node_facade::test_remote_control_grant;
        let prior = grant(RemoteControlRequestKind::Describe);
        let candidate = grant(RemoteControlRequestKind::AnnounceSelf);
        let baseline = grants::image(&grants::snapshot(&[prior]), Campaign::Append).await;
        let (_, trace) = attempt(
            baseline,
            candidate,
            None,
            FlashJournalCommitResolution::Committed,
            RemoteControlAuthorizationStoreRequirement::Initial,
        )
        .await;
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
                Operation::Erase { .. } => panic!("append must not erase"),
            };
            for completed_bytes in prefixes {
                let cut = Cut {
                    operation,
                    completed_bytes,
                };
                let resolution =
                    if operation > commit || (operation == commit && completed_bytes == 4) {
                        FlashJournalCommitResolution::Committed
                    } else {
                        FlashJournalCommitResolution::NotCommitted
                    };
                let expected = match resolution {
                    FlashJournalCommitResolution::Committed => candidate,
                    FlashJournalCommitResolution::NotCommitted => prior,
                };
                let (image, observed) = attempt(
                    baseline,
                    candidate,
                    Some(cut),
                    resolution,
                    RemoteControlAuthorizationStoreRequirement::Initial,
                )
                .await;
                assert_eq!(observed, trace[..=operation]);
                for _ in 0..2 {
                    grants::restore(image, &[expected]).await;
                }
                cases += 1;
            }
        }
        std::eprintln!("verified {cases} cancelled append/resume boundaries");
    });
}

#[test]
fn cancelled_authorization_commit_is_confirmed_without_rewriting_the_prior_table() {
    embassy_futures::block_on(async {
        let grant = crate::runtime::node_facade::test_remote_control_grant;
        let prior = grant(RemoteControlRequestKind::Describe);
        let candidate = grant(RemoteControlRequestKind::AnnounceSelf);
        let baseline = grants::image(&grants::snapshot(&[prior]), Campaign::Append).await;
        let (_, trace) = attempt(
            baseline,
            candidate,
            None,
            FlashJournalCommitResolution::Committed,
            RemoteControlAuthorizationStoreRequirement::Initial,
        )
        .await;
        let operation = trace
            .iter()
            .rposition(|op| matches!(op, Operation::Write { .. }))
            .unwrap();
        let (image, _) = attempt(
            baseline,
            candidate,
            Some(Cut {
                operation,
                completed_bytes: 4,
            }),
            FlashJournalCommitResolution::Committed,
            RemoteControlAuthorizationStoreRequirement::Initial,
        )
        .await;
        for _ in 0..2 {
            grants::restore(image, &[candidate]).await;
        }
    });
}

#[test]
fn cancelled_authorization_rollback_retries_instead_of_losing_its_deadline() {
    embassy_futures::block_on(async {
        let grant = crate::runtime::node_facade::test_remote_control_grant;
        let prior = grant(RemoteControlRequestKind::Describe);
        let candidate = grant(RemoteControlRequestKind::AnnounceSelf);
        let baseline = grants::image(&grants::snapshot(&[candidate]), Campaign::Append).await;
        let (image, _) = attempt(
            baseline,
            prior,
            Some(Cut {
                operation: 0,
                completed_bytes: 0,
            }),
            FlashJournalCommitResolution::NotCommitted,
            RemoteControlAuthorizationStoreRequirement::Rollback,
        )
        .await;
        for _ in 0..2 {
            grants::restore(image, &[prior]).await;
        }
    });
}
