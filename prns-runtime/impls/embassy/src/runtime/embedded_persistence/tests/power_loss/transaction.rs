use super::*;
use crate::engine::{
    IssuedCommand, PrnsCommand, RemoteControlTargetPairingAuthorizationPersistence,
    RemoteControlTargetPairingFinalization, SettleRemoteControlTargetPairingAuthorization,
    Settlement,
};
use crate::remote_control::{RemoteControlRequestKind, RemoteControlTargetPairingResponder};
use crate::routing::links::{request::RequestId, LinkId};
use crate::runtime::remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionState;
use crate::runtime::remote_control_pairing_persistence::{
    RemoteControlAuthorizationStoreExchange, RemoteControlPairingManifoldPersistence,
    RemoteControlPairingPersistenceProgress, RemoteControlPairingPersistenceRequired,
};
use crate::runtime::{
    CompletionPool, EmbeddedRemoteControlPairingPersistenceFailure,
    EmbeddedRemoteControlPairingPersistenceOperation, PrnsNodeHandle,
};
use embassy_futures::join::join;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

#[test]
fn failed_pairing_store_keeps_authority_until_real_flash_rollback_recovers() {
    embassy_futures::block_on(async {
        let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
        let completions = CompletionPool::<CriticalSectionRawMutex, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
        let exchange = DiscoveryGroupConfigurationStoreExchange::new();
        let (flash, fail_write) = TestFlash::controlled();
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
        remote.set_controller_grant(prior).unwrap();
        let confirmed = controller_grants_snapshot(&remote);
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
        let candidate = crate::runtime::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::AnnounceSelf,
        );
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

        let mut manifold = RemoteControlPairingManifoldPersistence::new(&mut owner, &stores);
        fail_write.set(true);
        ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, WRITE_TIME);
        manifold.progress(&mut engine, WRITE_TIME).await;
        let failed = stores.next_completion().await;
        assert_eq!(failed, Err(EmbeddedPersistenceFailure::Flash));
        assert_eq!(controller_grants_snapshot(&remote), confirmed);

        let accept = progress.accept_store_completion(
            failed,
            &mut remote,
            &mut authorization,
            &stores,
            handle,
        );
        let acknowledge = async {
            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::SettleRemoteControlTargetPairingAuthorization(
                    SettleRemoteControlTargetPairingAuthorization {
                        attempt_id,
                        persistence: RemoteControlTargetPairingAuthorizationPersistence::Failed
                    }
                )
            );
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::SettleRemoteControlTargetPairingAuthorization(Ok(
                        RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
                            attempt_id,
                            retired_link: LinkId::new([0x93; 16]),
                            responder: RemoteControlTargetPairingResponder::new(
                                LinkId::new([0x93; 16]),
                                RequestId([0x94; 16]),
                            ),
                        },
                    )),
                },
                |_| panic!("settlement must reach its awaiter"),
            );
        };
        let (accepted, ()) = join(accept, acknowledge).await;
        assert_eq!(accepted, Ok(()));
        assert!(progress.is_waiting_for_store());
        assert_eq!(controller_grants_snapshot(&remote), confirmed);

        let first_retry = InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis);
        let second_retry = InstantMillis(
            first_retry.0
                + policy
                    .retry_interval_millis
                    .max(policy.compaction.minimum_interval_millis),
        );
        let mut rollback = core::pin::pin!(stores.next_completion());
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        fail_write.set(true);
        for now in [WRITE_TIME, InstantMillis(first_retry.0 - 1)] {
            ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, now);
            manifold.progress(&mut engine, now).await;
            assert!(fail_write.get(), "backoff must not attempt a flash write");
            assert!(core::future::Future::poll(rollback.as_mut(), &mut context).is_pending());
            assert_eq!(controller_grants_snapshot(&remote), confirmed);
        }
        ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, first_retry);
        for _ in 0..32 {
            ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(
                &mut manifold,
                first_retry,
            );
            manifold.progress(&mut engine, first_retry).await;
            if !fail_write.get() {
                break;
            }
        }
        assert!(
            !fail_write.get(),
            "rollback must attempt storage at the deadline"
        );
        assert!(core::future::Future::poll(rollback.as_mut(), &mut context).is_pending());
        ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(
            &mut manifold,
            InstantMillis(second_retry.0 - 1),
        );
        manifold
            .progress(&mut engine, InstantMillis(second_retry.0 - 1))
            .await;
        assert!(core::future::Future::poll(rollback.as_mut(), &mut context).is_pending());
        ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(&mut manifold, second_retry);
        for _ in 0..32 {
            ManifoldPersistence::<crate::storage::GrowableHeap>::deadline(
                &mut manifold,
                second_retry,
            );
            manifold.progress(&mut engine, second_retry).await;
        }
        assert_eq!(
            core::future::Future::poll(rollback.as_mut(), &mut context),
            core::task::Poll::Ready(Ok(()))
        );
        assert_eq!(
            progress
                .accept_store_completion(Ok(()), &mut remote, &mut authorization, &stores, handle)
                .await,
            Err(EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreAuthorization,
                failure: EmbeddedPersistenceFailure::Flash,
            })
        );
        assert!(progress.is_ready());
        assert_eq!(controller_grants_snapshot(&remote), confirmed);
        drop(manifold);
        let bytes = owner.journal.take().unwrap().release().bytes;
        drop(owner);
        let mut persisted = Vec::new();
        let mut inspected_flash = TestFlash::new();
        inspected_flash.bytes = bytes;
        let _ = FlashJournal::open(
            inspected_flash,
            LAYOUT,
            &mut [0; RECORD_SCRATCH_LEN],
            |record| {
                if record.kind == FlashJournalRecordKind::RemoteControlControllerGrants {
                    persisted.push(record.payload.to_vec());
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(
            persisted,
            std::vec![confirmed.to_vec(), confirmed.to_vec()],
            "rollback must append a second durable snapshot, not merely retain the first"
        );
        for _ in 0..2 {
            let restored_exchange = DiscoveryGroupConfigurationStoreExchange::new();
            let mut flash = TestFlash::new();
            flash.bytes = bytes;
            let mut restored = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
                flash, LAYOUT, policy, FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &restored_exchange,
            );
            let mut restored_engine = EngineState::<crate::storage::GrowableHeap>::default();
            let mut restored_remote = available_remote_control(&mut restored_engine);
            restored
                .restore(&mut restored_engine, &mut restored_remote, InstantMillis(0))
                .await;
            assert_eq!(controller_grants_snapshot(&restored_remote), confirmed);
        }
    });
}
