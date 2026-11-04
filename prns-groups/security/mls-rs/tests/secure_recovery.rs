#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/secure.rs"]
mod secure;

use prns_groups_mls::{Admission, Event};
use prns_groups_store::secure::SecureRequest;
use prns_groups_store::{
    CommitOutcome, MaintenanceOutcome, OperationRejection, RecoveryOutcome, Revision, StoreError,
};
use secure::nor::{Cut, Timing};
use secure::{
    operation, publication_and_retirement, ticket, Enrollment, TestResult, CAPSULES, GROUP,
    PARTITION,
};
use std::cell::Cell;

const BODY: &[u8] = b"encrypted flash and real MLS, without replaying consumed ratchets";

#[test]
fn secure_interrupted_compaction_fences_real_mls_store_until_maintenance_recovery() -> TestResult {
    let mut peers = Enrollment::joined()?;
    let sent = peers.owner.adapter().send(operation(4), GROUP, BODY)?;
    let before = peers.owner.controller.bytes();
    let revision = peers.owner.adapter().coordinator().head()?;
    peers.owner.controller.clear_trace();
    assert!(matches!(
        peers
            .owner
            .adapter()
            .coordinator()
            .maintain(SecureRequest::Compact)?,
        MaintenanceOutcome::Complete(_)
    ));
    let (_, retirement) = publication_and_retirement(&peers.owner.controller.actions());
    peers.owner.restart_from(before)?;
    peers.owner.controller.arm(Cut {
        action: retirement,
        timing: Timing::During,
    });
    let coordinator = peers.owner.adapter().coordinator();
    assert!(matches!(
        coordinator.maintain(SecureRequest::Compact),
        Err(_) | Ok(MaintenanceOutcome::Uncertain)
    ));
    assert!(peers.owner.controller.fired());
    assert!(coordinator.head().is_err());
    assert!(coordinator.read(b"configuration").is_err());
    assert!(coordinator.begin(operation(5), revision).is_err());
    assert!(coordinator.recover(operation(4)).is_err());
    assert!(peers.owner.adapter().retry(operation(4)).is_err());
    assert!(coordinator.recover_maintenance().is_err());
    assert!(coordinator.head().is_err());
    // I/O returns, but the same coordinator must remain fenced until the caller
    // uses the dedicated recovery path. A new operation cannot resolve this.
    peers.owner.controller.restore_power();
    assert!(coordinator.head().is_err());
    assert!(coordinator.begin(operation(5), revision).is_err());
    assert!(coordinator.recover(operation(4)).is_err());
    assert!(matches!(
        coordinator.recover_maintenance()?,
        MaintenanceOutcome::Complete(_)
    ));
    assert_eq!(coordinator.head()?, revision);
    assert_eq!(
        peers.owner.adapter().retry(operation(4))?.unwrap().messages,
        sent.output.messages
    );
    peers.owner.reopen()?;
    assert_eq!(peers.owner.adapter().coordinator().head()?, revision);
    assert_eq!(
        peers.owner.adapter().retry(operation(4))?.unwrap().messages,
        sent.output.messages
    );
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    Ok(())
}

#[test]
fn secure_enrollment_reloads_identity_and_group_from_flash_bytes() -> TestResult {
    let mut peers = Enrollment::joined()?;
    peers.owner.reopen()?;
    peers.member.reopen()?;
    let sent = peers.owner.adapter().send(operation(4), GROUP, BODY)?;
    peers.owner.reopen()?;
    let retried = peers.owner.adapter().retry(operation(4))?.unwrap();
    assert_eq!(retried.messages, sent.output.messages);
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &retried.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    peers.member.reopen()?;
    assert!(
        matches!(peers.member.adapter().retry(operation(4))?.unwrap().event,
        Event::Application(ref body) if body == BODY)
    );
    Ok(())
}

#[test]
fn secure_published_send_survives_every_retirement_cut_with_exact_outbox() -> TestResult {
    for timing in [Timing::Before, Timing::During, Timing::After] {
        let mut peers = Enrollment::joined()?;
        let before = peers.owner.controller.bytes();
        peers.owner.controller.clear_trace();
        peers.owner.adapter().send(operation(4), GROUP, BODY)?;
        let (_, retirement) = publication_and_retirement(&peers.owner.controller.actions());
        peers.owner.restart_from(before)?;
        peers.owner.controller.arm(Cut {
            action: retirement,
            timing,
        });
        assert!(peers
            .owner
            .adapter()
            .send(operation(4), GROUP, BODY)
            .is_err());
        assert!(peers.owner.controller.fired());
        assert!(peers.owner.adapter().coordinator().head().is_err());
        assert!(
            peers.owner.adapter().retry(operation(4)).is_err(),
            "strict outputs remain fenced before recovery"
        );
        let published_image = peers.owner.controller.bytes();
        peers.owner.reopen()?;
        assert!(matches!(
            peers.owner.adapter().coordinator().recover(operation(4))?,
            RecoveryOutcome::Committed(_)
        ));
        let output = peers.owner.adapter().retry(operation(4))?.unwrap();
        assert!(matches!(output.event, Event::Sent));
        let data_start = PARTITION.offset as usize + CAPSULES * secure::nor::ERASE;
        assert_eq!(
            &peers.owner.controller.bytes()[data_start..],
            &published_image[data_start..],
            "retirement recovery must not regenerate persisted ciphertext objects"
        );
        peers.owner.reopen()?;
        assert_eq!(
            peers.owner.adapter().retry(operation(4))?.unwrap().messages,
            output.messages
        );
        let revision = peers.owner.adapter().coordinator().head()?;
        assert!(peers
            .owner
            .adapter()
            .send(operation(4), GROUP, b"different reused work")
            .is_err());
        assert_eq!(peers.owner.adapter().coordinator().head()?, revision);
        let received =
            peers
                .member
                .adapter()
                .receive(operation(4), GROUP, &output.messages[0], |_| {
                    Admission::Accept
                })?;
        assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    }
    Ok(())
}

#[test]
fn secure_marker_result_loss_recovers_send_without_reencryption() -> TestResult {
    let mut peers = Enrollment::joined()?;
    let before = peers.owner.controller.bytes();
    peers.owner.controller.clear_trace();
    peers.owner.adapter().send(operation(4), GROUP, BODY)?;
    let (publication, _) = publication_and_retirement(&peers.owner.controller.actions());
    peers.owner.restart_from(before)?;
    peers.owner.controller.arm(Cut {
        action: publication,
        timing: Timing::After,
    });
    assert!(peers
        .owner
        .adapter()
        .send(operation(4), GROUP, BODY)
        .is_err());
    assert!(peers.owner.controller.fired());
    peers.owner.reopen()?;
    assert!(matches!(
        peers.owner.adapter().coordinator().recover(operation(4))?,
        RecoveryOutcome::Committed(_)
    ));
    let output = peers.owner.adapter().retry(operation(4))?.unwrap();
    peers.owner.reopen()?;
    assert_eq!(
        peers.owner.adapter().retry(operation(4))?.unwrap().messages,
        output.messages
    );
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    Ok(())
}

#[test]
fn secure_received_and_rejected_generations_stay_consumed_after_retirement_recovery() -> TestResult
{
    for accept in [false, true] {
        let mut peers = Enrollment::joined()?;
        let sent = peers.owner.adapter().send(operation(4), GROUP, BODY)?;
        let wire = &sent.output.messages[0];
        let before = peers.member.controller.bytes();
        peers.member.controller.clear_trace();
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, wire, |_| {
                if accept {
                    Admission::Accept
                } else {
                    Admission::Reject(31)
                }
            })?;
        let (_, retirement) = publication_and_retirement(&peers.member.controller.actions());
        peers.member.restart_from(before)?;
        peers.member.controller.arm(Cut {
            action: retirement,
            timing: Timing::During,
        });
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, wire, |_| {
                if accept {
                    Admission::Accept
                } else {
                    Admission::Reject(31)
                }
            })
            .is_err());
        assert!(peers.member.controller.fired());
        peers.member.reopen()?;
        assert!(matches!(
            peers.member.adapter().coordinator().recover(operation(4))?,
            RecoveryOutcome::Committed(_)
        ));
        let output = peers.member.adapter().retry(operation(4))?.unwrap();
        if accept {
            assert!(matches!(&output.event, Event::Application(body) if body == BODY));
        } else {
            assert!(matches!(output.event, Event::Rejected(31)));
        }
        let calls = Cell::new(0);
        let revision = peers.member.adapter().coordinator().head()?;
        assert!(peers
            .member
            .adapter()
            .receive(operation(5), GROUP, wire, |_| {
                calls.set(calls.get() + 1);
                Admission::Accept
            })
            .is_err());
        assert_eq!(calls.get(), 0);
        assert_eq!(peers.member.adapter().coordinator().head()?, revision);
        peers
            .member
            .adapter()
            .acknowledge(operation(5), operation(4))?;
        peers.member.reopen()?;
        assert!(peers.member.adapter().retry(operation(4))?.is_none());
        assert!(peers
            .member
            .adapter()
            .receive(operation(6), GROUP, wire, |_| {
                calls.set(calls.get() + 1);
                Admission::Accept
            })
            .is_err());
        assert_eq!(calls.get(), 0);
    }
    Ok(())
}

#[test]
fn secure_join_recovery_consumes_key_package_with_group_creation() -> TestResult {
    let mut peers = Enrollment::new()?;
    let packages = peers
        .member
        .adapter()
        .coordinator()
        .read(b"key-packages")?
        .unwrap();
    assert_eq!(packages.len(), 32);
    let mut package_key = vec![b'K'];
    package_key.extend_from_slice(&packages);
    assert!(peers
        .member
        .adapter()
        .coordinator()
        .read(&package_key)?
        .is_some());
    let before = peers.member.controller.bytes();
    peers.member.controller.clear_trace();
    peers
        .member
        .adapter()
        .join(operation(3), GROUP, &peers.welcome, secure::NOW)?;
    let (_, retirement) = publication_and_retirement(&peers.member.controller.actions());
    peers.member.restart_from(before)?;
    peers.member.controller.arm(Cut {
        action: retirement,
        timing: Timing::Before,
    });
    assert!(peers
        .member
        .adapter()
        .join(operation(3), GROUP, &peers.welcome, secure::NOW)
        .is_err());
    assert!(peers.member.controller.fired());
    peers.member.reopen()?;
    assert!(matches!(
        peers.member.adapter().coordinator().recover(operation(3))?,
        RecoveryOutcome::Committed(_)
    ));
    assert!(peers
        .member
        .adapter()
        .coordinator()
        .read(&package_key)?
        .is_none());
    assert!(peers
        .member
        .adapter()
        .coordinator()
        .read(b"key-packages")?
        .unwrap()
        .is_empty());
    assert!(matches!(
        peers.member.adapter().retry(operation(3))?.unwrap().event,
        Event::Joined
    ));
    let sent = peers.owner.adapter().send(operation(4), GROUP, BODY)?;
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    Ok(())
}

#[test]
fn secure_rekey_keeps_delayed_receive_but_removal_purges_it_across_reopen() -> TestResult {
    let mut peers = Enrollment::joined()?;
    let delayed = peers.owner.adapter().send(operation(4), GROUP, BODY)?;
    let rekey = peers
        .owner
        .adapter()
        .rekey(operation(5), GROUP)
        .map_err(|error| format!("owner routine rekey: {error}"))?;
    peers
        .member
        .adapter()
        .receive(operation(4), GROUP, &rekey.output.messages[0], |_| {
            Admission::Accept
        })
        .map_err(|error| format!("member routine rekey: {error}"))?;
    peers.member.reopen()?;
    let received = peers
        .member
        .adapter()
        .receive(operation(5), GROUP, &delayed.output.messages[0], |_| {
            Admission::Accept
        })
        .map_err(|error| format!("member delayed receive: {error}"))?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    peers.member.reopen()?;
    assert!(peers
        .member
        .adapter()
        .receive(operation(6), GROUP, &delayed.output.messages[0], |_| {
            panic!("consumed prior-epoch key was restored")
        })
        .is_err());
    let pending = peers
        .owner
        .adapter()
        .send(operation(6), GROUP, b"unread before removal")
        .map_err(|error| format!("owner pre-removal send: {error}"))?;
    let removed = peers
        .owner
        .adapter()
        .remove(operation(7), GROUP, 1)
        .map_err(|error| format!("owner removal: {error}"))?;
    let result = peers
        .member
        .adapter()
        .receive(operation(6), GROUP, &removed.output.messages[0], |_| {
            Admission::Accept
        })
        .map_err(|error| format!("member removal: {error}"))?;
    assert!(matches!(result.output.event, Event::Removed));
    peers.member.reopen()?;
    assert!(peers
        .member
        .adapter()
        .receive(operation(7), GROUP, &pending.output.messages[0], |_| {
            panic!("removal barrier disappeared during reopen")
        })
        .is_err());
    let mut group_key = vec![b'G'];
    group_key.extend_from_slice(&GROUP.0);
    assert!(peers
        .member
        .adapter()
        .coordinator()
        .read(&group_key)?
        .is_none());
    let mut history_key = vec![b'H'];
    history_key.extend_from_slice(&GROUP.0);
    assert!(peers
        .member
        .adapter()
        .coordinator()
        .read(&history_key)?
        .unwrap()
        .is_empty());
    peers.owner.reopen()?;
    assert!(peers.owner.adapter().retry(operation(6)).is_err());
    assert!(peers.owner.adapter().coordinator().head()? > Revision(0));
    Ok(())
}

#[test]
fn secure_compaction_preserves_outstanding_mls_ciphertext_and_live_receive_state() -> TestResult {
    let mut peers = Enrollment::joined()?;
    let mut outgoing = Vec::new();
    for sequence in 4..=6 {
        let sent = peers
            .owner
            .adapter()
            .send(operation(sequence), GROUP, BODY)?;
        outgoing.push(sent.output.messages[0].clone());
        let before_revision = peers.owner.adapter().coordinator().head()?;
        let before_arena = peers.owner.last_mount.active_arena;
        let usage = peers.owner.maintenance(SecureRequest::Compact)?;
        assert_eq!(peers.owner.adapter().coordinator().head()?, before_revision);
        assert_ne!(usage.active_arena, before_arena);
        assert!(usage.used_bytes <= usage.arena_bytes);
        for (index, wire) in outgoing.iter().enumerate() {
            assert_eq!(
                peers
                    .owner
                    .adapter()
                    .retry(operation(4 + index as u64))?
                    .unwrap()
                    .messages,
                vec![wire.clone()]
            );
        }
    }
    for (index, wire) in outgoing.iter().enumerate() {
        let received =
            peers
                .member
                .adapter()
                .receive(operation(4 + index as u64), GROUP, wire, |_| {
                    Admission::Accept
                })?;
        assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
        peers.member.maintenance(SecureRequest::Compact)?;
    }
    assert!(peers
        .member
        .adapter()
        .receive(operation(7), GROUP, &outgoing[0], |_| {
            panic!("compaction restored a consumed receive key")
        })
        .is_err());
    Ok(())
}

#[test]
fn secure_receipt_floor_after_output_acknowledgement_never_reauthorizes_old_encryption(
) -> TestResult {
    let mut peers = Enrollment::joined()?;
    // This fixture has discharged enrollment; acknowledge both bootstrap outputs
    // before attesting that *all* tickets through the finalization floor are done.
    peers
        .owner
        .adapter()
        .acknowledge(operation(4), operation(2))?;
    peers
        .owner
        .adapter()
        .acknowledge(operation(5), operation(3))?;
    let sent = peers.owner.adapter().send(operation(6), GROUP, BODY)?;
    let received =
        peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    peers
        .owner
        .adapter()
        .acknowledge(operation(7), operation(6))?;
    assert!(peers.owner.adapter().retry(operation(6))?.is_none());
    let usage = peers
        .owner
        .maintenance(SecureRequest::FinalizeThrough(ticket(7)))?;
    assert_eq!(usage.retired_through, 7);
    peers.owner.maintenance(SecureRequest::Compact)?;
    assert!(
        matches!(peers.owner.adapter().coordinator().recover(operation(6))?,
        RecoveryOutcome::Retired { retired_through, .. } if retired_through == ticket(7))
    );
    let before = peers.owner.controller.bytes();
    let revision = peers.owner.adapter().coordinator().head()?;
    assert!(peers
        .owner
        .adapter()
        .send(
            operation(6),
            GROUP,
            b"must not consume a new ratchet generation"
        )
        .is_err());
    assert_eq!(peers.owner.adapter().coordinator().head()?, revision);
    assert_eq!(peers.owner.controller.bytes(), before);
    let next =
        peers
            .owner
            .adapter()
            .send(operation(8), GROUP, b"still usable after finalization")?;
    let received =
        peers
            .member
            .adapter()
            .receive(operation(5), GROUP, &next.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(
        matches!(&received.output.event, Event::Application(body) if body == b"still usable after finalization")
    );
    Ok(())
}

#[test]
fn secure_bounded_receipt_capacity_preserves_pending_outbox_and_recovers_after_safe_floor(
) -> TestResult {
    let mut peers = Enrollment::joined()?;
    peers
        .owner
        .adapter()
        .acknowledge(operation(4), operation(2))?;
    peers
        .owner
        .adapter()
        .acknowledge(operation(5), operation(3))?;
    let pending = peers.owner.adapter().send(operation(6), GROUP, BODY)?;
    let mut sequence = 7;
    loop {
        let coordinator = peers.owner.adapter().coordinator();
        let before = coordinator.head()?;
        let tx = match coordinator.begin(operation(sequence), before) {
            Ok(tx) => tx,
            Err(StoreError::OperationRejected(OperationRejection::RecoveryWindowFull)) => {
                assert_eq!(coordinator.head()?, before);
                break;
            }
            Err(error) => return Err(error.into()),
        };
        // Fill receipt capacity using harmless application facts. Keep the one
        // outstanding MLS ciphertext live throughout storage reclamation.
        tx.put(b"fixture/counter", &sequence.to_be_bytes())?;
        match tx.commit()? {
            CommitOutcome::Committed(_) => sequence += 1,
            CommitOutcome::Rejected(OperationRejection::RecoveryWindowFull) => {
                assert_eq!(coordinator.head()?, before);
                break;
            }
            other => return Err(format!("unexpected capacity outcome: {other:?}").into()),
        }
        assert!(
            sequence <= 40,
            "configured receipt bound must reject new work"
        );
    }
    peers.owner.reopen()?;
    assert_eq!(
        peers.owner.adapter().retry(operation(6))?.unwrap().messages,
        pending.output.messages
    );
    let before = peers.owner.controller.bytes();
    assert!(peers
        .owner
        .adapter()
        .send(operation(sequence), GROUP, b"no room yet")
        .is_err());
    assert_eq!(peers.owner.controller.bytes(), before);
    // Finalize only the acknowledged prefix. Ticket 6 still owns an outstanding
    // ciphertext and must remain recoverable after the capacity repair.
    peers
        .owner
        .maintenance(SecureRequest::FinalizeThrough(ticket(5)))?;
    peers.owner.maintenance(SecureRequest::Compact)?;
    assert!(matches!(
        peers.owner.adapter().coordinator().recover(operation(6))?,
        RecoveryOutcome::Committed(_)
    ));
    assert_eq!(
        peers.owner.adapter().retry(operation(6))?.unwrap().messages,
        pending.output.messages
    );
    let sent = peers
        .owner
        .adapter()
        .send(operation(sequence), GROUP, b"capacity restored")?;
    for (id, wire, body) in [
        (4, &pending.output.messages[0], BODY),
        (5, &sent.output.messages[0], b"capacity restored".as_slice()),
    ] {
        let received = peers
            .member
            .adapter()
            .receive(operation(id), GROUP, wire, |_| Admission::Accept)?;
        assert!(matches!(&received.output.event, Event::Application(value) if value == body));
    }
    Ok(())
}
