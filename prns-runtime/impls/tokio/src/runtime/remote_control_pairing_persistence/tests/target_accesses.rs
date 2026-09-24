use super::*;
use crate::engine::{
    PrnsCommand, SettleRemoteControlControllerPairingPersistenceFailure as Failure, Settlement,
};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::manifold::driver::HostCommand;
use crate::persistence::{read_remote_control_target_accesses_snapshot, FileStore, PersistedStore};
use crate::remote_control::RemoteControlTargetAccessTable;
use crate::routing::links::LinkId;
use crate::runtime::node_facade::{NodePersistence, TestDirectory};

enum Acknowledgement {
    Completed,
    Lost,
    Mismatch,
    NotOwed,
    Rejected,
}

fn access(fill: u8, request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey(
                [fill; 32],
            )),
            signing: IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([fill; 32])),
        }),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

#[tokio::test]
async fn committed_target_access_activation_mismatch_preserves_the_durable_candidate() {
    for prior in [None, Some(RemoteControlRequestKind::Describe)] {
        let directory = TestDirectory::new();
        let (commands, _receiver) = mpsc::unbounded_channel();
        let worker = NodePersistence::custom_dir(directory.path())
            .unwrap()
            .worker(PrnsNodeHandle::over(commands));
        let persistence = worker.remote_control_authorization_persistence();
        let mut remote = remote_control();
        if let Some(request) = prior {
            remote.set_target_access(access(0x52, request)).unwrap();
        }
        let candidate = || access(0x52, RemoteControlRequestKind::AnnounceSelf);
        let desired = TargetAccessSpec::from_access(&candidate());
        let mutation = match remote.set_target_access(candidate()).unwrap() {
            SetRemoteControlTargetAccessOutcome::Added => TargetAccessMutation::Added { desired },
            SetRemoteControlTargetAccessOutcome::Updated { previous } => {
                TargetAccessMutation::Updated {
                    desired,
                    previous: TargetAccessSpec::from_access(&previous),
                }
            }
            SetRemoteControlTargetAccessOutcome::Unchanged => panic!("candidate must differ"),
        };
        let projected = target_accesses_snapshot(&remote).unwrap();
        rollback_target_access(&mut remote, mutation).unwrap();
        let transaction = persistence.begin().await.unwrap();
        transaction
            .store(
                SnapshotRegion::RemoteControlTargetAccesses,
                projected.clone(),
            )
            .await
            .unwrap()
            .unwrap();
        remote.set_target_access(candidate()).unwrap();
        assert_eq!(
            activate_target_access(&mut remote, &mutation),
            Err(RemoteControlAuthorizationPersistenceFailure::CommittedTargetAccessActivation)
        );
        assert!(!remote.is_available());
        assert_eq!(remote.write_target_accesses_snapshot(&mut []), Ok(None));
        for _ in 0..2 {
            let store = FileStore::new(directory.path());
            let mut bytes = vec![0; remote_control_target_accesses_snapshot_capacity(1)];
            let loaded = store
                .load(SnapshotRegion::RemoteControlTargetAccesses, &mut bytes)
                .unwrap()
                .unwrap();
            assert_eq!(loaded, projected.as_slice());
            assert_eq!(
                read_remote_control_target_accesses_snapshot(loaded)
                    .unwrap()
                    .collect::<Vec<_>>(),
                vec![candidate()]
            );
        }
        assert!(persistence.begin().await.is_err());
        drop(transaction);
        assert!(persistence.begin().await.is_err());
    }
}

#[tokio::test]
async fn committed_target_access_survives_missing_and_rejected_settlement() {
    for acknowledgement in [
        Acknowledgement::Completed,
        Acknowledgement::Lost,
        Acknowledgement::Mismatch,
        Acknowledgement::NotOwed,
        Acknowledgement::Rejected,
    ] {
        for prior in [None, Some(RemoteControlRequestKind::Describe)] {
            let directory = TestDirectory::new();
            let (commands, mut command_rx) = mpsc::unbounded_channel();
            let node = PrnsNodeHandle::over(commands);
            let worker = NodePersistence::custom_dir(directory.path())
                .unwrap()
                .worker(node.clone());
            let persistence = worker.remote_control_authorization_persistence();
            let mut remote = remote_control();
            remote
                .set_target_access(access(0x51, RemoteControlRequestKind::Describe))
                .unwrap();
            if let Some(request) = prior {
                remote.set_target_access(access(0x52, request)).unwrap();
            }
            let transaction = persistence.begin().await.unwrap();
            transaction
                .store(
                    SnapshotRegion::RemoteControlTargetAccesses,
                    target_accesses_snapshot(&remote).unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            let attempt_id =
                RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([0x53; 32]);
            let other =
                RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([0x54; 32]);
            let candidate = || access(0x52, RemoteControlRequestKind::AnnounceSelf);
            let mut expected = vec![
                access(0x51, RemoteControlRequestKind::Describe),
                candidate(),
            ];
            expected.sort_by_key(|access| *access.target().identity_hash().as_bytes());
            let applying = persist_target_access(
                &mut remote,
                Some(&transaction),
                &node,
                attempt_id,
                candidate(),
            );
            let acknowledge = async {
                let Some(HostCommand::AwaitedEngine { issued, completion }) =
                    command_rx.recv().await
                else {
                    panic!("awaited pairing settlement");
                };
                assert_eq!(
                    issued.command,
                    PrnsCommand::SettleRemoteControlControllerPairingPersistence(
                        SettleRemoteControlControllerPairingPersistence {
                            attempt_id,
                            persistence: RemoteControlControllerPairingPersistence::Persisted
                        }
                    )
                );
                let store = FileStore::new(directory.path());
                let mut bytes = vec![0; remote_control_target_accesses_snapshot_capacity(2)];
                let loaded = store
                    .load(SnapshotRegion::RemoteControlTargetAccesses, &mut bytes)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    read_remote_control_target_accesses_snapshot(loaded)
                        .unwrap()
                        .collect::<Vec<_>>(),
                    expected
                );
                let result = match acknowledgement {
                    Acknowledgement::Lost => {
                        drop(completion);
                        return Err(RemoteControlAuthorizationPersistenceFailure::CommittedTargetAccessSettlementUnavailable);
                    }
                    Acknowledgement::Completed => {
                        Ok(RemoteControlControllerPairingFinalization::Completed {
                            attempt_id,
                            retired_link: LinkId::new([0x55; 16]),
                            access: candidate(),
                        })
                    }
                    Acknowledgement::Mismatch => Err(Failure::AttemptMismatch {
                        settled: attempt_id,
                        active: other,
                    }),
                    Acknowledgement::NotOwed => Err(Failure::NoPersistenceOwed {
                        settled: attempt_id,
                    }),
                    Acknowledgement::Rejected => Ok(
                        RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                            attempt_id,
                            retired_link: LinkId::new([0x55; 16]),
                            access: candidate(),
                        },
                    ),
                };
                let expected_result = match &result {
                    Ok(RemoteControlControllerPairingFinalization::Completed { .. }) => Ok(()),
                    Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded { .. }) => Err(RemoteControlAuthorizationPersistenceFailure::CommittedTargetAccessFinalizationMismatch),
                    Err(failure) => Err(RemoteControlAuthorizationPersistenceFailure::CommittedTargetAccessSettlement { failure: *failure }),
                };
                completion
                    .send(Settlement::SettleRemoteControlControllerPairingPersistence(
                        result,
                    ))
                    .unwrap();
                expected_result
            };
            let (result, expected_result) = tokio::join!(applying, acknowledge);
            assert_eq!(result, expected_result);
            assert_eq!(
                remote
                    .target_accesses()
                    .unwrap()
                    .accesses_in_identity_hash_order(),
                expected.as_slice()
            );
            for _ in 0..2 {
                let store = FileStore::new(directory.path());
                let mut bytes = vec![0; remote_control_target_accesses_snapshot_capacity(2)];
                let loaded = store
                    .load(SnapshotRegion::RemoteControlTargetAccesses, &mut bytes)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    read_remote_control_target_accesses_snapshot(loaded)
                        .unwrap()
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }
}
