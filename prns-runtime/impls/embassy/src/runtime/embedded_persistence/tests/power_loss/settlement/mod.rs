use super::*;
use crate::engine::{
    IssuedCommand, PrnsCommand, RemoteControlTargetPairingAuthorizationPersistence,
    RemoteControlTargetPairingFinalization, SettleRemoteControlTargetPairingAuthorization,
    SettleRemoteControlTargetPairingAuthorizationFailure, Settlement,
};
use crate::remote_control::{RemoteControlControllerGrantTable, RemoteControlRequestKind};
use crate::routing::links::LinkId;
use crate::runtime::remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionState;
use crate::runtime::remote_control_pairing_persistence::{
    EmbeddedRemoteControlPairingPersistenceFailure,
    EmbeddedRemoteControlPairingPersistenceOperation, RemoteControlAuthorizationStoreExchange,
    RemoteControlPairingManifoldPersistence, RemoteControlPairingPersistenceProgress,
    RemoteControlPairingPersistenceRequired,
};
use crate::runtime::{CompletionPool, PrnsNodeHandle};
use embassy_futures::join::join;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

enum Finalize {
    ActivationMismatch,
    Complete,
    LostCommitAcknowledgement,
    DeliveryFailed,
    RetentionExpired,
    SettlementBusy,
    SettlementUnavailable,
    SettlementRejected(SettleRemoteControlTargetPairingAuthorizationFailure),
    Inconsistent,
    RollBack,
}

struct Outcome {
    image: [u8; CAPACITY],
    trace: Vec<Operation>,
}

#[test]
fn committed_activation_mismatch_quarantines_without_a_flash_rollback() {
    let outcome = verify(Finalize::ActivationMismatch);
    assert_eq!(outcome.trace, Vec::<Operation>::new());
    let candidate = crate::runtime::node_facade::test_remote_control_grant(
        RemoteControlRequestKind::AnnounceSelf,
    );
    for _ in 0..2 {
        embassy_futures::block_on(super::grants::restore(outcome.image, &[candidate]));
    }
}

#[test]
fn successful_pairing_storage_activates_then_releases_authority() {
    verify(Finalize::Complete);
}

#[test]
fn lost_commit_acknowledgement_holds_ownership_until_read_only_confirmation() {
    verify(Finalize::LostCommitAcknowledgement);
}

#[test]
fn committed_authority_survives_inconsistent_finalization() {
    verify(Finalize::Inconsistent);
}

#[test]
fn committed_authority_survives_delivery_failure_and_retention_expiry() {
    verify(Finalize::DeliveryFailed);
    verify(Finalize::RetentionExpired);
}

#[test]
fn committed_authority_survives_unavailable_settlement() {
    verify(Finalize::SettlementBusy);
    verify(Finalize::SettlementUnavailable);
}

#[test]
fn committed_authority_survives_stale_attempt_settlement() {
    let settled = crate::runtime::node_facade::test_remote_control_pairing_attempt(0x92);
    verify(Finalize::SettlementRejected(
        SettleRemoteControlTargetPairingAuthorizationFailure::NoAuthorizationOwed { settled },
    ));
    verify(Finalize::SettlementRejected(
        SettleRemoteControlTargetPairingAuthorizationFailure::AttemptMismatch {
            settled,
            active: crate::runtime::node_facade::test_remote_control_pairing_attempt(0x95),
        },
    ));
}

#[test]
fn late_rollback_finalization_preserves_committed_authority() {
    verify(Finalize::RollBack);
}

#[test]
fn committed_authority_survives_completion_signing_failure() {
    let attempt_id = crate::runtime::node_facade::test_remote_control_pairing_attempt(0x92);
    let target_identity = crate::identity::IdentityHash::new([0x96; 16]);
    verify(Finalize::SettlementRejected(
        SettleRemoteControlTargetPairingAuthorizationFailure::TargetSignerUnavailable {
            attempt_id,
            target_identity,
        },
    ));
    verify(Finalize::SettlementRejected(SettleRemoteControlTargetPairingAuthorizationFailure::CompletionSigningFailed {
        attempt_id,
        error: crate::remote_control::RemoteControlPairingCompletionSigningError::TargetIdentityMismatch {
            expected: target_identity,
            found: crate::identity::IdentityHash::new([0x97; 16]),
        },
    }));
}

fn verify(finalize: Finalize) -> Outcome {
    embassy_futures::block_on(async {
        let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
        let completions = CompletionPool::<CriticalSectionRawMutex, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
        let exchange = DiscoveryGroupConfigurationStoreExchange::new();
        let control = Rc::new(RefCell::new(Control::new()));
        let flash = Flash::boot([0xff; CAPACITY], control.clone());
        let policy =
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
        let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
            flash, LAYOUT, policy, FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &exchange,
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote = available_remote_control(&mut engine);
        owner
            .restore(&mut engine, &mut remote, InstantMillis(0))
            .await;
        let prior = crate::runtime::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let candidate = crate::runtime::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::AnnounceSelf,
        );
        remote.set_controller_grant(candidate).unwrap();
        let next = controller_grants_snapshot(&remote);
        remote.set_controller_grant(prior).unwrap();
        let confirmed = controller_grants_snapshot(&remote);
        assert_ne!(confirmed, next);
        assert_eq!(
            owner
                .store_remote_control_authorization_snapshot(
                    &engine,
                    RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                    &confirmed,
                    InstantMillis(1)
                )
                .await,
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored
        );
        let mut authorization = RemoteControlPairingAuthorizationTransactionState::new();
        let mut progress = RemoteControlPairingPersistenceProgress::new();
        let attempt_id = crate::runtime::node_facade::test_remote_control_pairing_attempt(0x92);
        let delivery_failure = match finalize {
            Finalize::ActivationMismatch => None,
            Finalize::SettlementRejected(failure) => Some(failure),
            Finalize::DeliveryFailed => Some(
                SettleRemoteControlTargetPairingAuthorizationFailure::CompletionDispatchFailed {
                    attempt_id,
                    failure: crate::engine::RemoteControlPairingResponseDispatchFailure::Write(
                        crate::routing::links::request::LinkRequestWriteError::LinkVanished,
                    ),
                },
            ),
            Finalize::RetentionExpired => Some(
                SettleRemoteControlTargetPairingAuthorizationFailure::CompletionRetentionExpired {
                    attempt_id,
                    retired_link: LinkId::new([0x93; 16]),
                },
            ),
            Finalize::Complete
            | Finalize::LostCommitAcknowledgement
            | Finalize::Inconsistent
            | Finalize::SettlementBusy
            | Finalize::SettlementUnavailable
            | Finalize::RollBack => None,
        };
        progress
            .accept_required(
                RemoteControlPairingPersistenceRequired::ControllerGrant {
                    attempt_id,
                    grant: candidate,
                },
                &mut remote,
                &mut authorization,
                Some(&stores),
                handle,
            )
            .await
            .unwrap();
        assert_eq!(controller_grants_snapshot(&remote), confirmed);
        assert!(commands.receiver().try_receive().is_err());
        let mut manifold = RemoteControlPairingManifoldPersistence::new(&mut owner, &stores);
        if matches!(finalize, Finalize::LostCommitAcknowledgement) {
            control.borrow_mut().lose_commit_acknowledgement();
        }
        ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, WRITE_TIME);
        manifold.progress(&mut engine, WRITE_TIME).await;
        if matches!(finalize, Finalize::LostCommitAcknowledgement) {
            let mut completion = core::pin::pin!(stores.next_completion());
            let mut context = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
            assert!(commands.receiver().try_receive().is_err());
            assert_eq!(controller_grants_snapshot(&remote), confirmed);
            let interface =
                crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
            let groups = discovery_group_snapshot("queued");
            let mut group_change =
                core::pin::pin!(exchange.store(DiscoveryGroupConfigurationChange::upsert(
                    interface,
                    *groups.groups_for(interface).unwrap()
                )));
            assert!(core::future::Future::poll(group_change.as_mut(), &mut context).is_pending());
            assert!(!ManifoldPersistence::<crate::storage::GrowableHeap>::has_pending_configuration_change(&manifold));
            {
                let mut wake = core::pin::pin!(
                    ManifoldPersistence::<crate::storage::GrowableHeap>::wait_for_work(&manifold)
                );
                assert!(core::future::Future::poll(wake.as_mut(), &mut context).is_pending());
            }
            let retry = InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis);
            let before = control.borrow().trace.clone();
            manifold
                .progress(&mut engine, InstantMillis(retry.0 - 1))
                .await;
            assert_eq!(control.borrow().trace, before);
            manifold.progress(&mut engine, retry).await;
            assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
            assert!(commands.receiver().try_receive().is_err());
            control.borrow_mut().arm(None);
            let next_retry = InstantMillis(retry.0 + policy.retry_interval_millis);
            manifold.progress(&mut engine, next_retry).await;
            assert!(control
                .borrow()
                .trace
                .iter()
                .all(|operation| matches!(operation, Operation::Read { .. })));
        }
        let stored = stores.next_completion().await;
        control.borrow_mut().arm(None);
        assert_eq!(stored, Ok(()));
        assert_eq!(controller_grants_snapshot(&remote), confirmed);
        assert!(commands.receiver().try_receive().is_err());
        if matches!(finalize, Finalize::ActivationMismatch) {
            remote.set_controller_grant(candidate).unwrap();
            assert_eq!(
                progress.accept_store_completion(stored, &mut remote, &mut authorization, &stores, handle).await,
                Err(EmbeddedRemoteControlPairingPersistenceFailure::CommittedActivation {
                    attempt_id,
                    failure: crate::runtime::remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionFailure::RuntimeState,
                })
            );
            assert!(!remote.is_available());
            assert!(!progress.is_ready());
            assert!(!progress.is_waiting_for_store());
            assert!(!matches!(
                authorization,
                RemoteControlPairingAuthorizationTransactionState::Available
            ));
            progress
                .accept_store_completion(Ok(()), &mut remote, &mut authorization, &stores, handle)
                .await
                .unwrap();
            assert!(!progress.is_ready());
            assert!(commands.receiver().try_receive().is_err());
            {
                let mut request = core::pin::pin!(stores.wait_for_next_test_store());
                let mut context = core::task::Context::from_waker(core::task::Waker::noop());
                assert!(core::future::Future::poll(request.as_mut(), &mut context).is_pending());
            }
            drop(manifold);
            return Outcome {
                image: owner.journal.take().unwrap().release().into_image(),
                trace: control.borrow().trace.clone(),
            };
        }
        let mut competing = core::pin::pin!(handle.settle_pairing_command(
            SettleRemoteControlTargetPairingAuthorization {
                attempt_id: crate::runtime::node_facade::test_remote_control_pairing_attempt(0x94),
                persistence: RemoteControlTargetPairingAuthorizationPersistence::Persisted,
            }
        ));
        if matches!(finalize, Finalize::SettlementBusy) {
            let mut context = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(core::future::Future::poll(competing.as_mut(), &mut context).is_pending());
        }
        let accept = progress.accept_store_completion(
            stored,
            &mut remote,
            &mut authorization,
            &stores,
            handle,
        );
        let acknowledge = async {
            if matches!(finalize, Finalize::SettlementBusy) {
                return;
            }
            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::SettleRemoteControlTargetPairingAuthorization(
                    SettleRemoteControlTargetPairingAuthorization {
                        attempt_id,
                        persistence: RemoteControlTargetPairingAuthorizationPersistence::Persisted
                    }
                )
            );
            let finalization = match finalize {
                Finalize::ActivationMismatch => {
                    unreachable!("activation failure never settles persisted")
                }
                Finalize::Inconsistent => {
                    RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
                        attempt_id,
                        retired_link: LinkId::new([0x93; 16]),
                        responder: crate::remote_control::RemoteControlTargetPairingResponder::new(
                            LinkId::new([0x93; 16]),
                            crate::routing::links::request::RequestId([0x94; 16]),
                        ),
                    }
                }
                Finalize::Complete
                | Finalize::LostCommitAcknowledgement
                | Finalize::DeliveryFailed
                | Finalize::RetentionExpired
                | Finalize::SettlementBusy
                | Finalize::SettlementUnavailable
                | Finalize::SettlementRejected(_) => {
                    RemoteControlTargetPairingFinalization::CompletionDispatched { attempt_id }
                }
                Finalize::RollBack => {
                    RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired {
                        attempt_id,
                        retired_link: LinkId::new([0x93; 16]),
                        grant: candidate,
                    }
                }
            };
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: if matches!(finalize, Finalize::SettlementUnavailable) {
                        Settlement::SettleRemoteControlControllerPairingPersistence(Err(
                            crate::engine::SettleRemoteControlControllerPairingPersistenceFailure::NoPersistenceOwed { settled: attempt_id },
                        ))
                    } else { Settlement::SettleRemoteControlTargetPairingAuthorization(
                        delivery_failure.map_or(Ok(finalization), Err),
                    ) },
                },
                |_| panic!("settlement must reach its awaiter"),
            );
        };
        let (accepted, ()) = join(accept, acknowledge).await;
        assert_eq!(
            accepted,
            match finalize {
                Finalize::RollBack => Err(EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedTargetFinalization {
                    attempt_id,
                    operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    finalization: crate::runtime::remote_control_pairing_persistence::EmbeddedRemoteControlTargetPairingFinalization::AuthorizationRollbackRequired,
                }),
                Finalize::Inconsistent => Err(EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedTargetFinalization {
                    attempt_id,
                    operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    finalization: crate::runtime::remote_control_pairing_persistence::EmbeddedRemoteControlTargetPairingFinalization::AuthorizationFailureRecorded,
                }),
                Finalize::SettlementBusy => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    }
                ),
                Finalize::SettlementUnavailable => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    }
                ),
                _ => delivery_failure.map_or(Ok(()), |failure| Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::TargetSettlement {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        failure,
                    }
                )),
            }
        );
        assert!(progress.is_ready());
        {
            let mut request = core::pin::pin!(stores.wait_for_next_test_store());
            let mut context = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(core::future::Future::poll(request.as_mut(), &mut context).is_pending());
        }
        let expected = next.clone();
        let expected_grant = candidate;
        assert_eq!(controller_grants_snapshot(&remote), expected);
        assert_eq!(
            remote
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[expected_grant]
        );
        assert!(matches!(
            authorization,
            RemoteControlPairingAuthorizationTransactionState::Available
        ));
        drop(manifold);
        let bytes = owner.journal.take().unwrap().release().into_image();
        drop(owner);
        let mut records = Vec::new();
        let mut flash = TestFlash::new();
        flash.bytes = bytes;
        let _ = FlashJournal::open(flash, LAYOUT, &mut [0; RECORD_SCRATCH_LEN], |record| {
            if record.kind == FlashJournalRecordKind::RemoteControlControllerGrants {
                records.push(record.payload.to_vec());
            }
        })
        .await
        .unwrap();
        let expected_records = std::vec![confirmed.to_vec(), next.to_vec()];
        assert_eq!(records, expected_records);
        for _ in 0..2 {
            let groups = DiscoveryGroupConfigurationStoreExchange::new();
            let mut flash = TestFlash::new();
            flash.bytes = bytes;
            let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(flash, LAYOUT, policy, FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups);
            let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
            let mut remote = available_remote_control(&mut engine);
            restored
                .restore(&mut engine, &mut remote, InstantMillis(0))
                .await;
            assert_eq!(controller_grants_snapshot(&remote), expected);
            assert_eq!(
                remote
                    .controller_grants()
                    .unwrap()
                    .grants_in_identity_hash_order(),
                &[expected_grant]
            );
        }
        let trace = control.borrow().trace.clone();
        Outcome {
            image: bytes,
            trace,
        }
    })
}

mod late_rollback;
mod target_accesses;
