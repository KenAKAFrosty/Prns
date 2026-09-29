use super::*;
use crate::engine::{
    RemoteControlControllerPairingFinalization, RemoteControlControllerPairingPersistence,
    SettleRemoteControlControllerPairingPersistence,
    SettleRemoteControlControllerPairingPersistenceFailure as Failure,
};
use crate::identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey};
use crate::remote_control::{
    RemoteControlControllerAuthority, RemoteControlRequestSet, RemoteControlTargetAccess,
    RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
};
use crate::runtime::remote_control_pairing_persistence::EmbeddedRemoteControlControllerPairingFinalization;

enum Acknowledgement {
    Completed,
    CommitReplyLost,
    Busy,
    Mismatch,
    NotOwed,
    Rejected,
}

fn keys(fill: u8) -> IdentityPublicKeys {
    IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([fill; 32])),
        signing: IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([fill; 32])),
    }
}

fn access(fill: u8, request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(keys(fill)),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

#[test]
fn committed_target_access_survives_rejected_settlement_and_fresh_restore() {
    for acknowledgement in [
        Acknowledgement::Completed,
        Acknowledgement::CommitReplyLost,
        Acknowledgement::Busy,
        Acknowledgement::Mismatch,
        Acknowledgement::NotOwed,
        Acknowledgement::Rejected,
    ] {
        for prior in [None, Some(RemoteControlRequestKind::Describe)] {
            embassy_futures::block_on(async {
                let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
                let completions = CompletionPool::<CriticalSectionRawMutex, 0>::new();
                let handle = PrnsNodeHandle::new(commands.sender(), &completions);
                let stores =
                    RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
                let groups = DiscoveryGroupConfigurationStoreExchange::new();
                let policy = EmbeddedPersistencePolicy::hopspot_default(
                    EmbeddedCompactionPolicy::hopspot(0),
                );
                let control = Rc::new(RefCell::new(Control::new()));
                let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
                    Flash::boot([0xff; CAPACITY], control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups,
                );
                let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
                let mut remote = available_remote_control(&mut engine);
                owner
                    .restore(&mut engine, &mut remote, InstantMillis(0))
                    .await;
                remote
                    .set_target_access(access(0x51, RemoteControlRequestKind::Describe))
                    .unwrap();
                if let Some(request) = prior {
                    remote.set_target_access(access(0x52, request)).unwrap();
                }
                let confirmed = target_accesses_snapshot(&remote);
                assert_eq!(
                    owner
                        .store_remote_control_authorization_snapshot(
                            &engine,
                            RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                            &confirmed,
                            InstantMillis(1),
                        )
                        .await,
                    StoreRemoteControlAuthorizationSnapshotOutcome::Stored
                );
                let candidate = || access(0x52, RemoteControlRequestKind::AnnounceSelf);
                let mut expected = std::vec![
                    access(0x51, RemoteControlRequestKind::Describe),
                    candidate()
                ];
                expected.sort_by_key(|access| *access.target().identity_hash().as_bytes());
                let attempt_id =
                    crate::runtime::node_facade::test_remote_control_pairing_attempt(0x53);
                let other = crate::runtime::node_facade::test_remote_control_pairing_attempt(0x54);
                let mut authorization = RemoteControlPairingAuthorizationTransactionState::new();
                let mut progress = RemoteControlPairingPersistenceProgress::new();
                progress
                    .accept_required(
                        RemoteControlPairingPersistenceRequired::TargetAccess {
                            attempt_id,
                            target_public_keys: keys(0x52),
                            authority: RemoteControlControllerAuthority::Operator,
                            permitted_requests: RemoteControlRequestSet::only(
                                RemoteControlRequestKind::AnnounceSelf,
                            ),
                        },
                        &mut remote,
                        &mut authorization,
                        Some(&stores),
                        handle,
                    )
                    .await
                    .unwrap();
                let mut manifold =
                    RemoteControlPairingManifoldPersistence::new(&mut owner, &stores);
                if matches!(acknowledgement, Acknowledgement::CommitReplyLost) {
                    control.borrow_mut().lose_commit_acknowledgement();
                }
                ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(
                    &mut manifold,
                    WRITE_TIME,
                );
                manifold.progress(&mut engine, WRITE_TIME).await;
                if matches!(acknowledgement, Acknowledgement::CommitReplyLost) {
                    let mut completion = core::pin::pin!(stores.next_completion());
                    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
                    assert!(
                        core::future::Future::poll(completion.as_mut(), &mut context).is_pending()
                    );
                    assert!(commands.receiver().try_receive().is_err());
                    assert_eq!(target_accesses_snapshot(&remote), confirmed);
                    control.borrow_mut().arm(None);
                    manifold
                        .progress(
                            &mut engine,
                            InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis),
                        )
                        .await;
                    assert!(control
                        .borrow()
                        .trace
                        .iter()
                        .all(|operation| matches!(operation, Operation::Read { .. })));
                }
                assert_eq!(stores.next_completion().await, Ok(()));
                assert_eq!(target_accesses_snapshot(&remote), confirmed);
                let mut competing = core::pin::pin!(handle.settle_pairing_command(
                    SettleRemoteControlControllerPairingPersistence {
                        attempt_id: other,
                        persistence: RemoteControlControllerPairingPersistence::Persisted,
                    }
                ));
                if matches!(acknowledgement, Acknowledgement::Busy) {
                    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
                    assert!(
                        core::future::Future::poll(competing.as_mut(), &mut context).is_pending()
                    );
                }
                let accept = progress.accept_store_completion(
                    Ok(()),
                    &mut remote,
                    &mut authorization,
                    &stores,
                    handle,
                );
                let acknowledge = async {
                    if matches!(acknowledgement, Acknowledgement::Busy) {
                        return Err(EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                            attempt_id,
                            operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        });
                    }
                    let issued = commands.receiver().receive().await;
                    assert_eq!(
                        issued.command,
                        PrnsCommand::SettleRemoteControlControllerPairingPersistence(
                            SettleRemoteControlControllerPairingPersistence {
                                attempt_id,
                                persistence: RemoteControlControllerPairingPersistence::Persisted,
                            }
                        )
                    );
                    let result = match acknowledgement {
                        Acknowledgement::Busy => unreachable!("occupied settlement slot has no acknowledgement"),
                        Acknowledgement::Completed | Acknowledgement::CommitReplyLost => Ok(RemoteControlControllerPairingFinalization::Completed {
                            attempt_id, retired_link: LinkId::new([0x55; 16]), access: candidate(),
                        }),
                        Acknowledgement::Mismatch => Err(Failure::AttemptMismatch { settled: attempt_id, active: other }),
                        Acknowledgement::NotOwed => Err(Failure::NoPersistenceOwed { settled: attempt_id }),
                        Acknowledgement::Rejected => Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                            attempt_id, retired_link: LinkId::new([0x55; 16]), access: candidate(),
                        }),
                    };
                    let operation =
                        EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted;
                    let expected_result = match &result {
                        Ok(RemoteControlControllerPairingFinalization::Completed { .. }) => Ok(()),
                        Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded { .. }) => Err(
                            EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedControllerFinalization {
                                attempt_id, operation, finalization: EmbeddedRemoteControlControllerPairingFinalization::PersistenceFailureRecorded,
                            }
                        ),
                        Err(failure) => Err(EmbeddedRemoteControlPairingPersistenceFailure::ControllerSettlement {
                            attempt_id, operation, failure: *failure,
                        }),
                    };
                    handle.route_journaled(
                        Journaled::CommandSettled {
                            id: issued.id,
                            settlement: Settlement::SettleRemoteControlControllerPairingPersistence(
                                result,
                            ),
                        },
                        |_| panic!("settlement must reach its awaiter"),
                    );
                    expected_result
                };
                let (accepted, expected_result) = join(accept, acknowledge).await;
                assert_eq!(accepted, expected_result);
                assert!(progress.is_ready());
                let mut pending_store = core::pin::pin!(stores.wait_for_next_test_store());
                let mut context = core::task::Context::from_waker(core::task::Waker::noop());
                assert!(
                    core::future::Future::poll(pending_store.as_mut(), &mut context).is_pending()
                );
                assert!(matches!(
                    authorization,
                    RemoteControlPairingAuthorizationTransactionState::Available
                ));
                assert_eq!(
                    remote
                        .target_accesses()
                        .unwrap()
                        .accesses_in_identity_hash_order(),
                    expected.as_slice()
                );
                let next = target_accesses_snapshot(&remote);
                drop(manifold);
                let bytes = owner.journal.take().unwrap().release().into_image();
                drop(owner);
                let mut records = Vec::new();
                let mut flash = TestFlash::new();
                flash.bytes = bytes;
                let _ = FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |record| {
                    if record.kind == FlashJournalRecordKind::RemoteControlTargetAccesses {
                        records.push(record.payload.to_vec());
                    }
                })
                .await
                .unwrap();
                assert_eq!(records, std::vec![confirmed.to_vec(), next.to_vec()]);
                for _ in 0..2 {
                    let groups = DiscoveryGroupConfigurationStoreExchange::new();
                    let mut flash = TestFlash::new();
                    flash.bytes = bytes;
                    let mut restored = EmbeddedFlashPersistence::<
                        _,
                        FixedRouteSnapshotKeys<8>,
                        _,
                        4,
                        _,
                    >::with_discovery_group_store(
                        flash,
                        LAYOUT,
                        policy,
                        FixedRouteSnapshotKeys::new(),
                        (|_| {}) as fn(EmbeddedPersistenceDiagnostic),
                        &groups,
                    );
                    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
                    let mut remote = available_remote_control(&mut engine);
                    restored
                        .restore(&mut engine, &mut remote, InstantMillis(0))
                        .await;
                    assert_eq!(target_accesses_snapshot(&remote), next);
                    assert_eq!(
                        remote
                            .target_accesses()
                            .unwrap()
                            .accesses_in_identity_hash_order(),
                        expected.as_slice()
                    );
                }
            });
        }
    }
}
