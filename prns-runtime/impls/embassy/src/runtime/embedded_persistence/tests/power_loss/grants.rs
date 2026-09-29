use super::*;
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey};
use crate::remote_control::{
    RemoteControlControllerAuthority as Authority, RemoteControlControllerGrant as Grant,
    RemoteControlControllerGrantTable, RemoteControlControllerIdentity,
    RemoteControlRequestKind as Request, RemoteControlRequestSet,
};

fn grant(fill: u8, authority: Authority, request: Request) -> Grant {
    Grant::new(
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        }),
        authority,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

pub(super) fn snapshot(grants: &[Grant]) -> RemoteControlAuthorizationSnapshot {
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    for grant in grants {
        remote.set_controller_grant(*grant).unwrap();
    }
    controller_grants_snapshot(&remote)
}

pub(super) async fn image(
    snapshot: &RemoteControlAuthorizationSnapshot,
    campaign: Campaign,
) -> [u8; CAPACITY] {
    let (mut journal, _) = FlashJournal::open(
        TestFlash::new(),
        LAYOUT,
        &mut [0; RECORD_SCRATCH_LEN],
        |_| {},
    )
    .await
    .unwrap();
    journal.initialize_empty().await.unwrap();
    journal
        .append(
            FlashJournalRecordKind::RemoteControlControllerGrants,
            snapshot,
        )
        .await
        .unwrap();
    if matches!(campaign, Campaign::CompactThenAppend) {
        let mut full = false;
        for _ in 0..32 {
            match journal
                .append(
                    FlashJournalRecordKind::RemoteControlControllerGrants,
                    snapshot,
                )
                .await
            {
                Ok(()) => {}
                Err(FlashJournalError::ArenaFull) => {
                    full = true;
                    break;
                }
                Err(error) => panic!("unexpected grant baseline error: {error:?}"),
            }
        }
        assert!(full);
    }
    journal.release().bytes
}

pub(super) async fn restore(image: [u8; CAPACITY], expected: &[Grant]) {
    let exchange = DiscoveryGroupConfigurationStoreExchange::new();
    let mut flash = TestFlash::new();
    flash.bytes = image;
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        flash, LAYOUT, EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &exchange,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    let report = owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert_eq!(
        (
            report.remote_control_controller_grants_restored_count,
            report.remote_control_controller_grants_refused_count,
            report.remote_control_controller_grants_dropped_count
        ),
        (expected.len() as u32, 0, 0)
    );
    let mut expected = expected.to_vec();
    expected.sort_by_key(|grant| *grant.controller().identity_hash().as_bytes());
    assert_eq!(
        remote
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        expected
    );
}

async fn store(
    image: [u8; CAPACITY],
    confirmed: &RemoteControlAuthorizationSnapshot,
    candidate: &RemoteControlAuthorizationSnapshot,
    cut: Option<Cut>,
) -> ([u8; CAPACITY], Vec<Operation>) {
    let exchange = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()), LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &exchange,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert_eq!(controller_grants_snapshot(&remote), *confirmed);
    match cut {
        Some(cut) => control.borrow_mut().remove_power_at(cut),
        None => control.borrow_mut().arm(None),
    }
    let mut reached_boundary = false;
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..MAX_PROGRESS_STEPS {
        let result = {
            let mut write = core::pin::pin!(owner.store_remote_control_authorization_snapshot(
                &engine,
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                candidate,
                WRITE_TIME
            ));
            core::future::Future::poll(write.as_mut(), &mut context)
        };
        match result {
            core::task::Poll::Pending => {
                assert!(cut.is_some() && control.borrow().power_removed());
                assert_eq!(
                    owner.remote_control_controller_grants_snapshot.as_ref(),
                    Some(confirmed)
                );
                reached_boundary = true;
                break;
            }
            core::task::Poll::Ready(StoreRemoteControlAuthorizationSnapshotOutcome::Stored) => {
                assert!(cut.is_none());
                assert_eq!(
                    owner.remote_control_controller_grants_snapshot.as_ref(),
                    Some(candidate)
                );
                reached_boundary = true;
                break;
            }
            core::task::Poll::Ready(
                StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress,
            ) => {}
            core::task::Poll::Ready(other) => panic!("unexpected grant store outcome: {other:?}"),
        }
    }
    assert!(reached_boundary, "bounded grant persistence work");
    assert_eq!(
        controller_grants_snapshot(&remote),
        *confirmed,
        "storage does not activate live authority"
    );
    let image = owner.journal.take().unwrap().release().into_image();
    let trace = control.borrow().trace.clone();
    (image, trace)
}

#[test]
fn interrupted_grant_updates_and_revocations_restore_whole_authority_tables() {
    embassy_futures::block_on(async {
        let administrator = grant(11, Authority::Administrator, Request::Describe);
        let operator = grant(22, Authority::Operator, Request::Describe);
        let confirmed = [administrator, operator];
        let confirmed_snapshot = snapshot(&confirmed);
        let candidates = [
            std::vec![
                administrator,
                grant(22, Authority::Operator, Request::AnnounceSelf)
            ],
            std::vec![administrator],
        ];
        for candidate in candidates {
            let candidate_snapshot = snapshot(&candidate);
            for campaign in [Campaign::Append, Campaign::CompactThenAppend] {
                let baseline = image(&confirmed_snapshot, campaign).await;
                let (healthy, trace) =
                    store(baseline, &confirmed_snapshot, &candidate_snapshot, None).await;
                restore(healthy, &candidate).await;
                assert_eq!(
                    trace
                        .iter()
                        .any(|event| matches!(event, Operation::Erase { .. })),
                    matches!(campaign, Campaign::CompactThenAppend)
                );
                let commit = trace
                    .iter()
                    .rposition(|event| matches!(event, Operation::Write { .. }))
                    .unwrap();
                assert!(matches!(trace[commit], Operation::Write { len: 4, .. }));
                let mut cuts = 0;
                for (operation, event) in trace.iter().enumerate() {
                    let prefixes: Vec<_> = match event {
                        Operation::Read { len, .. } => std::vec![0, *len],
                        Operation::Write { len, .. } | Operation::Erase { len, .. } => {
                            (0..=*len).collect()
                        }
                    };
                    for completed_bytes in prefixes {
                        let cut = Cut {
                            operation,
                            completed_bytes,
                        };
                        let (image, observed) = store(
                            baseline,
                            &confirmed_snapshot,
                            &candidate_snapshot,
                            Some(cut),
                        )
                        .await;
                        assert_eq!(observed, trace[..=operation], "{cut:?}");
                        let expected = if operation > commit
                            || (operation == commit && completed_bytes == 4)
                        {
                            candidate.as_slice()
                        } else {
                            &confirmed
                        };
                        for _ in 0..2 {
                            restore(image, expected).await;
                        }
                        cuts += 1;
                    }
                }
                std::eprintln!(
                    "verified {cuts} {campaign:?} grant cuts, candidate table size {}",
                    candidate.len()
                );
            }
        }
    });
}
