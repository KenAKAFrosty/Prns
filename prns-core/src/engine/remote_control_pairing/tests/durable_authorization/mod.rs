use super::*;
use crate::persistence::{
    read_remote_control_controller_grants_snapshot,
    remote_control_controller_grants_snapshot_capacity,
    write_remote_control_controller_grants_snapshot, FlashJournal, FlashJournalRecordKind,
};
use crate::remote_control::{
    FixedRemoteControlControllerGrantTable, RemoteControlControllerAuthority,
    RemoteControlControllerGrantTable,
};
use std::vec::Vec;

mod flash;
use flash::{Flash, LAYOUT};

enum CompletionDelivery {
    Discarded,
    Unavailable,
}

fn snapshot(grants: &[RemoteControlControllerGrant]) -> Vec<u8> {
    let mut table = FixedRemoteControlControllerGrantTable::<2>::default();
    for grant in grants {
        table.set_controller_grant(*grant).unwrap();
    }
    let mut bytes = std::vec![0; remote_control_controller_grants_snapshot_capacity(table.len())];
    let len = write_remote_control_controller_grants_snapshot(&table, &mut bytes).unwrap();
    bytes.truncate(len);
    bytes
}

async fn restore(flash: Flash, expected: &[RemoteControlControllerGrant]) -> Flash {
    let mut latest = Vec::new();
    let (journal, _) = FlashJournal::open(flash, LAYOUT, &mut [0; 512], |record| {
        assert_eq!(
            record.kind,
            FlashJournalRecordKind::RemoteControlControllerGrants
        );
        latest = read_remote_control_controller_grants_snapshot(record.payload)
            .unwrap()
            .collect();
    })
    .await
    .unwrap();
    let mut expected = expected.to_vec();
    expected.sort_by_key(|grant| *grant.controller().identity_hash().as_bytes());
    assert_eq!(latest, expected);
    journal.release()
}

#[test]
fn prepared_engine_grant_recovers_by_commit_not_by_completion_delivery() {
    verify(CompletionDelivery::Discarded);
}

#[test]
fn prepared_engine_grant_recovers_after_completion_egress_failure() {
    verify(CompletionDelivery::Unavailable);
}

fn verify(delivery: CompletionDelivery) {
    embassy_futures::block_on(async {
        for previous in [None, Some(RemoteControlRequestKind::AnnounceSelf)] {
            let controller = controller_identity();
            let (mut engine, interfaces, endpoint, link_id, _, _) =
                open_pairing_link(controller.identity_hash());
            let (attempt_id, transcript) = authorize_target_pairing(
                &mut engine,
                &interfaces,
                endpoint,
                link_id,
                controller,
                RequestId([0xB1; 16]),
                RequestId([0xB2; 16]),
            );
            let candidate = RemoteControlControllerGrant::new(
                controller,
                transcript.permissions().authority(),
                transcript.permissions().clone().into_permitted_requests(),
            )
            .unwrap();
            let administrator = RemoteControlControllerGrant::new(
                controller_identity_from_secret_fill(0xE1),
                RemoteControlControllerAuthority::Administrator,
                RemoteControlRequestSet::all(),
            )
            .unwrap();
            let mut prior = std::vec![administrator];
            if let Some(request) = previous {
                prior.push(
                    RemoteControlControllerGrant::new(
                        controller,
                        RemoteControlControllerAuthority::Operator,
                        RemoteControlRequestSet::only(request),
                    )
                    .unwrap(),
                );
            }
            let committed = [administrator, candidate];
            let prior_snapshot = snapshot(&prior);
            let candidate_snapshot = snapshot(&committed);
            assert_ne!(prior_snapshot, candidate_snapshot);
            let (mut journal, _) = FlashJournal::open(Flash::new(), LAYOUT, &mut [0; 512], |_| {})
                .await
                .unwrap();
            journal.initialize_empty().await.unwrap();
            journal
                .append(
                    FlashJournalRecordKind::RemoteControlControllerGrants,
                    &prior_snapshot,
                )
                .await
                .unwrap();
            let baseline = journal.release();
            let (mut healthy, _) =
                FlashJournal::open(baseline.reboot(), LAYOUT, &mut [0; 512], |_| {})
                    .await
                    .unwrap();
            healthy
                .append(
                    FlashJournalRecordKind::RemoteControlControllerGrants,
                    &candidate_snapshot,
                )
                .await
                .unwrap();
            let committed_flash = healthy.release();
            let programmed_bytes = committed_flash.programmed_bytes();
            assert!(programmed_bytes > candidate_snapshot.len());

            for cut in 0..=programmed_bytes {
                let (mut interrupted, _) =
                    FlashJournal::open(baseline.cut_after(cut), LAYOUT, &mut [0; 512], |_| {})
                        .await
                        .unwrap();
                assert!(interrupted
                    .append(
                        FlashJournalRecordKind::RemoteControlControllerGrants,
                        &candidate_snapshot,
                    )
                    .await
                    .is_err());
                let expected = if cut == programmed_bytes {
                    &committed[..]
                } else {
                    &prior
                };
                let flash = interrupted.release().reboot();
                let flash = restore(flash, expected).await;
                restore(flash.reboot(), expected).await;
            }

            // Storage may have committed even when its final acknowledgement was lost.
            // No persistence settlement was sent during any interrupted execution.
            assert!(matches!(engine.remote_control_target_pairing.view(),
                RemoteControlTargetPairingView::Authorizing(attempt)
                    if attempt.attempt_id() == attempt_id));
            assert_eq!(
                engine
                    .held_identities
                    .release(&transcript.target().identity_hash()),
                ReleaseHeldIdentityOutcome::Released
            );
            let mut settlement = None;
            let mut responses = 0;
            engine.ingest_command_into(
                IssuedCommand {
                    id: CommandId(0xB3),
                    command: PrnsCommand::SettleRemoteControlTargetPairingAuthorization(
                        SettleRemoteControlTargetPairingAuthorization {
                            attempt_id,
                            persistence:
                                RemoteControlTargetPairingAuthorizationPersistence::Persisted,
                        },
                    ),
                },
                AttachedInterfaces::new(match delivery {
                    CompletionDelivery::Discarded => &interfaces,
                    CompletionDelivery::Unavailable => &[],
                }),
                InstantMillis(32_000),
                &mut |bytes| bytes.fill(0xB4),
                &mut |reaction| match reaction {
                    EngineReaction::Directive(_) => responses += 1,
                    EngineReaction::Journaled(Journaled::CommandSettled {
                        settlement: observed,
                        ..
                    }) => settlement = Some(observed),
                    EngineReaction::Journaled(_) => {}
                },
            );
            let (expected_responses, expected_settlement) = match delivery {
                CompletionDelivery::Discarded => (1, Ok(
                    RemoteControlTargetPairingFinalization::CompletionDispatched { attempt_id },
                )),
                CompletionDelivery::Unavailable => (0, Err(
                    SettleRemoteControlTargetPairingAuthorizationFailure::CompletionDispatchFailed {
                        attempt_id,
                        failure: RemoteControlPairingResponseDispatchFailure::EgressUnavailable {
                            interface: interfaces[0].id,
                        },
                    },
                )),
            };
            assert_eq!(responses, expected_responses);
            assert_eq!(
                settlement,
                Some(Settlement::SettleRemoteControlTargetPairingAuthorization(
                    expected_settlement,
                ))
            );
            // Discard the outbound response and all volatile engine state.
            drop(engine);
            let flash = restore(committed_flash.reboot(), &committed).await;
            restore(flash.reboot(), &committed).await;
        }
    });
}
