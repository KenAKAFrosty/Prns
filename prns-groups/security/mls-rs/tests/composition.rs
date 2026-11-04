#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod support;

use prns_groups_mls::{
    Admission, ApplicationKey, Command, CompositionError, Error, Event, MAX_APPLICATION_KEY_BYTES,
};
use prns_groups_store::{OperationId, RecoveryOutcome, StoreError};
use std::{
    cell::Cell,
    io,
    panic::{catch_unwind, AssertUnwindSafe},
};
use support::{durable::Cut, limits, Enrollment, TestResult, GROUP};

#[derive(Debug)]
enum AppError {
    Storage(StoreError<io::Error>),
    Policy,
}
impl From<StoreError<io::Error>> for AppError {
    fn from(error: StoreError<io::Error>) -> Self {
        Self::Storage(error)
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "{error}"),
            Self::Policy => f.write_str("application policy"),
        }
    }
}
impl std::error::Error for AppError {}
fn operation(n: u8) -> OperationId {
    OperationId([n; 32])
}

#[test]
fn sent_ciphertext_and_application_fact_commit_once_and_survive_reopen() -> TestResult {
    let mut peers = Enrollment::joined(limits())?;
    let staged = Cell::new(0);
    let committed = peers.owner.adapter().compose(
        operation(3),
        Command::Send {
            group: GROUP,
            body: b"shared transaction",
        },
        |_, _, _| -> Result<_, AppError> { unreachable!() },
        |application, output| {
            staged.set(staged.get() + 1);
            assert_eq!(output.event, Event::Sent);
            assert_eq!(output.context.unwrap().group, GROUP);
            application.put(b"record/3", &output.messages[0])?;
            assert_eq!(
                &**application.read(b"record/3")?.as_ref().unwrap(),
                &output.messages[0]
            );
            // Reentry cannot observe or start an operation while the attempt is active.
            assert!(matches!(
                peers.owner.adapter().application_read(b"record/3"),
                Err(Error::Store(StoreError::Busy))
            ));
            Ok(())
        },
    )?;
    assert_eq!(staged.get(), 1);
    peers.owner.reopen()?;
    let retry = peers.owner.adapter().retry(operation(3))?.unwrap();
    assert_eq!(retry, committed.output);
    assert_eq!(
        &**peers
            .owner
            .adapter()
            .application_read(b"record/3")?
            .as_ref()
            .unwrap(),
        &retry.messages[0]
    );
    let key = ApplicationKey::new(b"record/3")?;
    assert_eq!(
        peers.owner.adapter().coordinator().read(key.bytes())?,
        Some(retry.messages[0].clone()),
        "same-transaction readers use the adapter's owning key encoding"
    );
    assert_eq!(staged.get(), 1);
    assert!(matches!(
        peers.owner.adapter().compose(
            operation(3),
            Command::Send {
                group: GROUP,
                body: b"new encryption forbidden"
            },
            |_, _, _| -> Result<_, AppError> { panic!("must not call admission") },
            |_, _| { panic!("must not call stage") },
        ),
        Err(CompositionError::Mls(Error::Store(
            StoreError::AlreadyCommitted(_)
        )))
    ));
    Ok(())
}

#[test]
fn application_failure_and_unwind_roll_back_crypto_and_all_staged_facts() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let before = peers.owner.bytes()?;
    let result = peers.owner.adapter().compose(
        operation(3),
        Command::Send {
            group: GROUP,
            body: b"must roll back",
        },
        |_, _, _| -> Result<_, AppError> { unreachable!() },
        |application, _| {
            application.put(b"pending", b"not committed")?;
            Err(AppError::Policy)
        },
    );
    assert!(matches!(
        result,
        Err(CompositionError::Application(AppError::Policy))
    ));
    assert_eq!(peers.owner.bytes()?, before);
    assert!(peers
        .owner
        .adapter()
        .application_read(b"pending")?
        .is_none());
    assert!(peers.owner.adapter().retry(operation(3))?.is_none());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = peers.owner.adapter().compose(
            operation(3),
            Command::Send {
                group: GROUP,
                body: b"panic rolls back too",
            },
            |_, _, _| -> Result<_, AppError> { unreachable!() },
            |application, _| {
                application.put(b"pending", b"unwinding")?;
                panic!("injected callback panic")
            },
        );
    }));
    assert!(panic.is_err());
    assert_eq!(peers.owner.bytes()?, before);
    peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"after rolled-back attempts")?;
    Ok(())
}

#[test]
fn uncertain_publication_recovers_application_fact_and_exact_output_together() -> TestResult {
    for cut in [
        Cut::BeforeSnapshot,
        Cut::DuringSnapshot,
        Cut::AfterSnapshot,
        Cut::AfterPublish,
    ] {
        let mut peers = Enrollment::joined(limits())?;
        let before = peers.owner.bytes()?;
        let mut expected_ciphertext = Vec::new();
        peers.owner.faults.arm(cut);
        let result = peers.owner.adapter().compose(
            operation(3),
            Command::Send {
                group: GROUP,
                body: b"recover application and crypto",
            },
            |_, _, _| -> Result<_, AppError> { unreachable!() },
            |application, output| {
                expected_ciphertext = output.messages[0].clone();
                application.put(b"record/3", &expected_ciphertext)?;
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(CompositionError::Mls(Error::Uncertain(_)))
        ));
        assert!(matches!(
            peers.owner.adapter().application_read(b"record/3"),
            Err(Error::Store(StoreError::RecoveryRequired(_)))
        ));
        assert!(matches!(
            peers.owner.adapter().retry(operation(3)),
            Err(Error::Store(StoreError::RecoveryRequired(_)))
        ));
        peers.owner.reopen()?;
        let recovery = peers.owner.adapter().coordinator().recover(operation(3))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovery, RecoveryOutcome::Committed(_)));
            assert_eq!(
                peers.owner.adapter().retry(operation(3))?.unwrap().messages,
                vec![expected_ciphertext.clone()]
            );
            assert_eq!(
                &**peers
                    .owner
                    .adapter()
                    .application_read(b"record/3")?
                    .as_ref()
                    .unwrap(),
                &expected_ciphertext
            );
        } else {
            assert!(matches!(recovery, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(peers.owner.bytes()?, before);
            assert!(peers
                .owner
                .adapter()
                .application_read(b"record/3")?
                .is_none());
            assert!(peers.owner.adapter().retry(operation(3))?.is_none());
        }
    }
    Ok(())
}

#[test]
fn accepted_and_rejected_receive_share_application_facts_and_consumed_keys() -> TestResult {
    for decision in [Admission::Accept, Admission::Reject(42)] {
        let mut peers = Enrollment::joined(limits())?;
        let sent = peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"authenticated body")?;
        let committed = peers.member.adapter().compose(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &sent.output.messages[0],
            },
            |application, binding, body| -> Result<_, AppError> {
                assert_eq!(binding, &peers.owner.identity.binding());
                assert_eq!(body, b"authenticated body");
                assert!(application.read(b"dedup/3")?.is_none());
                application.put(b"dedup/3", b"authenticated")?;
                Ok(decision)
            },
            |application, output| {
                assert_eq!(
                    &**application.read(b"dedup/3")?.as_ref().unwrap(),
                    b"authenticated"
                );
                match (&decision, &output.event) {
                    (Admission::Accept, Event::Application(body)) => {
                        application.put(b"inbox/3", body)?
                    }
                    (Admission::Reject(42), Event::Rejected(42)) => {
                        application.put(b"rejected/3", b"42")?
                    }
                    _ => panic!("wrong application decision"),
                }
                application.put(b"receipt/3", b"persisted")?;
                Ok(())
            },
        )?;
        peers.member.reopen()?;
        assert_eq!(
            peers.member.adapter().retry(operation(3))?.unwrap(),
            committed.output
        );
        assert_eq!(
            &**peers
                .member
                .adapter()
                .application_read(b"receipt/3")?
                .as_ref()
                .unwrap(),
            b"persisted"
        );
        assert_eq!(
            peers
                .member
                .adapter()
                .application_read(b"inbox/3")?
                .is_some(),
            decision == Admission::Accept
        );
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &sent.output.messages[0], |_| panic!(
                "replay admission must not run"
            ))
            .is_err());
    }
    Ok(())
}

#[test]
fn failed_receive_staging_preserves_receive_key_for_retry() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let sent = peers
        .owner
        .adapter()
        .send(operation(3), GROUP, b"durable inbox required")?;
    let before = peers.member.bytes()?;
    for fail_admission in [true, false] {
        let result = peers.member.adapter().compose(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &sent.output.messages[0],
            },
            |application, _, _| -> Result<_, AppError> {
                application.put(b"dedup", b"pending")?;
                if fail_admission {
                    Err(AppError::Policy)
                } else {
                    Ok(Admission::Accept)
                }
            },
            |_, _| Err(AppError::Policy),
        );
        assert!(matches!(
            result,
            Err(CompositionError::Application(AppError::Policy))
        ));
        assert_eq!(peers.member.bytes()?, before);
    }
    let result =
        peers
            .member
            .adapter()
            .receive(operation(3), GROUP, &sent.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert_eq!(
        result.output.event,
        Event::Application(b"durable inbox required".to_vec())
    );
    Ok(())
}

#[test]
fn uncertain_receive_never_separates_application_decision_from_consumed_keys() -> TestResult {
    for decision in [Admission::Accept, Admission::Reject(42)] {
        for cut in [Cut::DuringSnapshot, Cut::AfterPublish] {
            let mut peers = Enrollment::joined(limits())?;
            let sent = peers
                .owner
                .adapter()
                .send(operation(3), GROUP, b"recover inbox")?;
            peers.member.faults.arm(cut);
            let result = peers.member.adapter().compose(
                operation(3),
                Command::Receive {
                    group: GROUP,
                    bytes: &sent.output.messages[0],
                },
                |_, _, _| -> Result<_, AppError> { Ok(decision) },
                |application, output| {
                    let fact: &[u8] = match output.event {
                        Event::Application(_) => b"accepted",
                        Event::Rejected(42) => b"rejected",
                        _ => panic!("unexpected receive outcome"),
                    };
                    application.put(b"receipt/3", fact)?;
                    Ok(())
                },
            );
            assert!(matches!(
                result,
                Err(CompositionError::Mls(Error::Uncertain(_)))
            ));
            peers.member.reopen()?;
            let recovery = peers.member.adapter().coordinator().recover(operation(3))?;
            if matches!(cut, Cut::AfterPublish) {
                assert!(matches!(recovery, RecoveryOutcome::Committed(_)));
                let expected: &[u8] = if decision == Admission::Accept {
                    b"accepted"
                } else {
                    b"rejected"
                };
                assert_eq!(
                    &**peers
                        .member
                        .adapter()
                        .application_read(b"receipt/3")?
                        .as_ref()
                        .unwrap(),
                    expected
                );
                let recovered = peers.member.adapter().retry(operation(3))?.unwrap();
                assert_eq!(
                    recovered.event,
                    if decision == Admission::Accept {
                        Event::Application(b"recover inbox".to_vec())
                    } else {
                        Event::Rejected(42)
                    }
                );
                assert!(peers
                    .member
                    .adapter()
                    .receive(operation(4), GROUP, &sent.output.messages[0], |_| panic!(
                        "consumed key must not authenticate twice"
                    ))
                    .is_err());
            } else {
                assert!(matches!(recovery, RecoveryOutcome::NotCommitted { .. }));
                assert!(peers
                    .member
                    .adapter()
                    .application_read(b"receipt/3")?
                    .is_none());
                assert!(peers.member.adapter().retry(operation(3))?.is_none());
                peers.member.adapter().receive(
                    operation(3),
                    GROUP,
                    &sent.output.messages[0],
                    |_| decision,
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn application_namespace_cannot_read_replace_or_delete_crypto_and_output_keys() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let mut group_key = vec![b'G'];
    group_key.extend_from_slice(&GROUP.0);
    let crypto_before = peers
        .owner
        .adapter()
        .coordinator()
        .read(&group_key)?
        .unwrap();
    let mut output_key = vec![b'O'];
    output_key.extend_from_slice(&operation(3).0);
    peers.owner.adapter().compose(
        operation(3),
        Command::Send {
            group: GROUP,
            body: b"namespace isolation",
        },
        |_, _, _| -> Result<_, AppError> { unreachable!() },
        |application, _| {
            assert!(application.read(b"configuration")?.is_none());
            assert!(application.read(&group_key)?.is_none());
            application.put(&group_key, b"application value")?;
            application.put(&output_key, b"not a crypto output")?;
            application.delete(b"configuration")?;
            Ok(())
        },
    )?;
    assert_ne!(
        peers
            .owner
            .adapter()
            .coordinator()
            .read(&group_key)?
            .unwrap(),
        crypto_before
    );
    assert_eq!(
        &**peers
            .owner
            .adapter()
            .application_read(&group_key)?
            .as_ref()
            .unwrap(),
        b"application value"
    );
    assert_eq!(
        &**peers
            .owner
            .adapter()
            .application_read(&output_key)?
            .as_ref()
            .unwrap(),
        b"not a crypto output"
    );
    assert_eq!(
        peers.owner.adapter().retry(operation(3))?.unwrap().event,
        Event::Sent
    );
    peers
        .owner
        .adapter()
        .send(operation(4), GROUP, b"MLS snapshot still intact")?;
    Ok(())
}

#[test]
fn ignored_application_storage_errors_still_abort_the_whole_attempt() -> TestResult {
    let peers = Enrollment::joined(limits())?;
    let before = peers.owner.bytes()?;
    for oversized_key in [false, true] {
        let result = peers.owner.adapter().compose(
            operation(3),
            Command::Send {
                group: GROUP,
                body: b"no partial staging",
            },
            |_, _, _| -> Result<_, AppError> { unreachable!() },
            |application, _| {
                application.put(b"valid", b"staged")?;
                if oversized_key {
                    let _ = application.read(&[1; MAX_APPLICATION_KEY_BYTES + 1]);
                } else {
                    let _ = application.put(b"oversized value", &vec![0; 1_048_577]);
                }
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(CompositionError::ApplicationStorageFault)
        ));
        assert_eq!(peers.owner.bytes()?, before);
    }
    Ok(())
}

#[test]
fn maintenance_retires_output_and_application_fact_atomically_at_every_cut() -> TestResult {
    for cut in [
        Cut::BeforeSnapshot,
        Cut::DuringSnapshot,
        Cut::AfterSnapshot,
        Cut::AfterPublish,
    ] {
        let mut peers = Enrollment::joined(limits())?;
        peers.owner.adapter().compose(
            operation(3),
            Command::Send {
                group: GROUP,
                body: b"retire safely",
            },
            |_, _, _| -> Result<_, AppError> { unreachable!() },
            |app, _| {
                app.put(b"record/3", b"retained")?;
                Ok(())
            },
        )?;
        let before = peers.owner.bytes()?;
        peers.owner.faults.arm(cut);
        let result = peers
            .owner
            .adapter()
            .maintain(operation(4), &[operation(3)], |app| {
                app.delete(b"record/3")?;
                app.put(b"retired-floor", b"3")?;
                Ok::<_, AppError>(())
            });
        assert!(matches!(
            result,
            Err(CompositionError::Mls(Error::Uncertain(_)))
        ));
        assert!(matches!(
            peers.owner.adapter().retry(operation(3)),
            Err(Error::Store(StoreError::RecoveryRequired(_)))
        ));
        peers.owner.reopen()?;
        let committed = matches!(cut, Cut::AfterPublish);
        assert_eq!(
            matches!(
                peers.owner.adapter().coordinator().recover(operation(4))?,
                RecoveryOutcome::Committed(_)
            ),
            committed
        );
        assert_eq!(
            peers
                .owner
                .adapter()
                .application_read(b"retired-floor")?
                .is_some(),
            committed
        );
        assert_eq!(
            peers
                .owner
                .adapter()
                .application_read(b"record/3")?
                .is_none(),
            committed
        );
        assert_eq!(
            peers.owner.adapter().retry(operation(3))?.is_none(),
            committed
        );
        if !committed {
            assert_eq!(peers.owner.bytes()?, before);
        }
        assert!(matches!(
            peers
                .owner
                .adapter()
                .send(operation(3), GROUP, b"cannot re-encrypt"),
            Err(Error::Store(StoreError::AlreadyCommitted(_)))
        ));
        assert!(peers.owner.adapter().retry(operation(4))?.is_none());
    }
    Ok(())
}

#[test]
fn maintenance_can_free_a_full_output_index_but_aborts_on_ignored_storage_error() -> TestResult {
    let mut cap = limits();
    cap.max_pending_outputs = 3;
    let peers = Enrollment::new(cap)?;
    let owner = support::Endpoint::new(
        peers.owner.identity.clone(),
        peers.owner.policy.clone(),
        cap,
    )?;
    owner.adapter().create(operation(1), GROUP)?;
    owner
        .adapter()
        .send(operation(2), GROUP, b"second output")?;
    owner.adapter().send(operation(3), GROUP, b"fill index")?;
    assert_eq!(owner.adapter().pending_operations()?.len(), 3);
    assert!(owner.adapter().send(operation(4), GROUP, b"full").is_err());
    let before = owner.bytes()?;
    let failed = owner
        .adapter()
        .maintain(operation(4), &[operation(3)], |app| {
            let _ = app.put(&[0; MAX_APPLICATION_KEY_BYTES + 1], b"too large");
            Ok::<_, AppError>(())
        });
    assert!(matches!(
        failed,
        Err(CompositionError::ApplicationStorageFault)
    ));
    assert_eq!(owner.bytes()?, before);
    owner
        .adapter()
        .maintain(operation(4), &[operation(3)], |app| {
            app.put(b"retired-floor", b"3")?;
            Ok::<_, AppError>(())
        })?;
    assert_eq!(owner.adapter().pending_operations()?.len(), 2);
    owner
        .adapter()
        .send(operation(5), GROUP, b"space available")?;
    assert_eq!(owner.adapter().pending_operations()?.len(), 3);
    Ok(())
}
