#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use personal_rns::prelude::{RemoteControlError, SendError};
use personal_rns::remote_control::{
    RemoteControlApplyOutcome as Apply, RemoteControlWifiTransactionStatus as Transaction,
};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

type Result<T> = std::result::Result<T, RemoteControlTargetOperationError>;
enum Step {
    Inspect(Result<Transaction>),
    Stage(Result<core::RemoteControlWifiStageOutcome>),
    Activate(Result<Apply>),
    Keep(Result<Apply>),
    Restore(Result<Apply>),
}
struct Script(RefCell<VecDeque<Step>>);
impl Script {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self(RefCell::new(steps.into_iter().collect()))
    }
    fn next(&self) -> Step {
        self.0
            .borrow_mut()
            .pop_front()
            .expect("no unexpected exchange")
    }
    fn done(&self) {
        assert!(
            self.0.borrow().is_empty(),
            "all expected exchanges completed"
        );
    }
}
impl Exchange for Script {
    async fn inspect(&self) -> Result<Transaction> {
        match self.next() {
            Step::Inspect(result) => result,
            _ => panic!("expected inspect"),
        }
    }
    async fn stage(
        &self,
        station: core::RemoteControlWifiStation,
    ) -> Result<core::RemoteControlWifiStageOutcome> {
        assert_eq!(station.ssid(), "trial-network");
        match self.next() {
            Step::Stage(result) => result,
            _ => panic!("expected stage"),
        }
    }
    async fn activate(&self, revision: core::RemoteControlWifiCredentialRevision) -> Result<Apply> {
        assert_eq!(revision, rev(2));
        match self.next() {
            Step::Activate(result) => result,
            _ => panic!("expected activate"),
        }
    }
    async fn keep(&self, revision: core::RemoteControlWifiCredentialRevision) -> Result<Apply> {
        assert_eq!(revision, rev(2));
        match self.next() {
            Step::Keep(result) => result,
            _ => panic!("expected keep"),
        }
    }
    async fn restore(&self, revision: core::RemoteControlWifiCredentialRevision) -> Result<Apply> {
        assert_eq!(revision, rev(2));
        match self.next() {
            Step::Restore(result) => result,
            _ => panic!("expected restore"),
        }
    }
}
fn rev(value: u32) -> core::RemoteControlWifiCredentialRevision {
    core::RemoteControlWifiCredentialRevision::new(value).expect("nonzero fixture")
}
fn waiting(seconds: u8) -> Transaction {
    Transaction::AwaitingConfirmation {
        revision: rev(2),
        remaining: core::RemoteControlWifiConfirmationRemaining::new(seconds)
            .expect("bounded fixture"),
    }
}
fn start() -> Work {
    Work::Start(
        core::RemoteControlWifiStation::parse("trial-network", "test-password")
            .expect("valid fixture"),
    )
}
fn finish(decision: RemoteWifiDecision) -> Work {
    Work::Finish {
        revision: rev(2),
        decision,
    }
}
fn lost() -> RemoteControlTargetOperationError {
    RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(SendError::NodeStopped))
}
fn protocol(error: core::RemoteControlProtocolError) -> RemoteControlTargetOperationError {
    RemoteControlTargetOperationError::Exchange(RemoteControlError::Remote(error))
}

#[test]
fn credential_validation_uses_utf8_bytes_and_redacts_secrets() {
    let input = |ssid: String, password: String| StartRemoteWifiTrialInput {
        target_identity_fingerprint: vec![7; 16],
        ssid,
        password,
    };
    assert!(prepare_start(input("é".repeat(16), "é".repeat(32))).is_ok());
    assert!(prepare_start(input("é".repeat(17), "private-password".to_owned())).is_err());
    assert!(prepare_start(input("network".to_owned(), "é".repeat(33))).is_err());
    assert!(prepare_start(input(String::new(), String::new())).is_err());
    let (_, Work::Start(station)) =
        prepare_start(input("network".to_owned(), "private-password".to_owned())).expect("valid")
    else {
        panic!("start")
    };
    assert!(!format!("{station:?}").contains("private-password"));
}

#[tokio::test]
async fn start_stages_once_activates_once_then_observes_not_confirms() {
    let script = Script::new([
        Step::Inspect(Ok(Transaction::Confirmed { revision: rev(1) })),
        Step::Stage(Ok(core::RemoteControlWifiStageOutcome::Staged(rev(2)))),
        Step::Activate(Ok(Apply::Scheduled)),
        Step::Inspect(Ok(waiting(119))),
    ]);
    let dispatched = AtomicBool::new(false);
    let candidate = Cell::new(None);
    assert_eq!(
        exchange(&script, start(), &dispatched, |revision| candidate
            .set(Some(revision)))
        .await,
        observed(waiting(119))
    );
    assert!(dispatched.load(Ordering::Acquire));
    assert_eq!(candidate.get(), Some(2));
    script.done();
}

#[tokio::test]
async fn existing_or_recovered_trial_is_never_overwritten_or_activated() {
    for before in [
        Transaction::Staged { revision: rev(2) },
        waiting(70),
        Transaction::RollingBack {
            rejected_revision: rev(2),
        },
    ] {
        let script = Script::new([Step::Inspect(Ok(before))]);
        let dispatched = AtomicBool::new(false);
        assert_eq!(
            exchange(&script, start(), &dispatched, |_| panic!(
                "no candidate staged"
            ))
            .await,
            observed(before)
        );
        assert!(!dispatched.load(Ordering::Acquire));
        script.done();
    }
}

#[tokio::test]
async fn lost_stage_reply_never_replays_stage_or_guesses_revision() {
    let script = Script::new([
        Step::Inspect(Ok(Transaction::FactoryProvisioning)),
        Step::Stage(Err(lost())),
    ]);
    assert!(matches!(
        exchange(&script, start(), &AtomicBool::new(false), |_| panic!(
            "unknown revision"
        ))
        .await,
        RemoteWifiStatus::OutcomeUnknown { .. }
    ));
    script.done();
    let recovered = Script::new([Step::Inspect(Ok(Transaction::Staged { revision: rev(2) }))]);
    assert_eq!(
        exchange(&recovered, Work::Inspect, &AtomicBool::new(false), |_| {}).await,
        observed(Transaction::Staged { revision: rev(2) })
    );
    recovered.done();
}

#[tokio::test]
async fn lost_activation_reply_retains_candidate_without_replay() {
    let script = Script::new([
        Step::Inspect(Ok(Transaction::FactoryProvisioning)),
        Step::Stage(Ok(core::RemoteControlWifiStageOutcome::Staged(rev(2)))),
        Step::Activate(Err(lost())),
    ]);
    let candidate = Cell::new(None);
    assert!(matches!(
        exchange(&script, start(), &AtomicBool::new(false), |revision| {
            candidate.set(Some(revision))
        })
        .await,
        RemoteWifiStatus::OutcomeUnknown { .. }
    ));
    assert_eq!(candidate.get(), Some(2));
    script.done();
}

#[tokio::test]
async fn keep_requires_matching_unexpired_trial_and_target_acceptance() {
    for before in [
        Transaction::Staged { revision: rev(2) },
        waiting(0),
        Transaction::Staged { revision: rev(3) },
    ] {
        let script = Script::new([Step::Inspect(Ok(before))]);
        let dispatched = AtomicBool::new(false);
        assert!(matches!(
            exchange(
                &script,
                finish(RemoteWifiDecision::Keep),
                &dispatched,
                |_| {}
            )
            .await,
            RemoteWifiStatus::Failed { .. }
        ));
        assert!(!dispatched.load(Ordering::Acquire));
        script.done();
    }
    let script = Script::new([
        Step::Inspect(Ok(waiting(50))),
        Step::Keep(Err(protocol(core::RemoteControlProtocolError::Busy {
            request: core::RemoteControlRequestKind::ConfirmWifiCredentials,
        }))),
    ]);
    assert!(
        matches!(exchange(&script, finish(RemoteWifiDecision::Keep), &AtomicBool::new(false), |_| {}).await, RemoteWifiStatus::Failed { stage: RemoteManagementFailureStage::Busy, detail } if detail.contains("still connecting"))
    );
    script.done();
}

#[tokio::test]
async fn keep_and_restore_report_target_state_after_reply() {
    for (decision, step, after) in [
        (
            RemoteWifiDecision::Keep,
            Step::Keep(Ok(Apply::Applied)),
            Transaction::Confirmed { revision: rev(2) },
        ),
        (
            RemoteWifiDecision::Restore,
            Step::Restore(Ok(Apply::Scheduled)),
            Transaction::Confirmed { revision: rev(1) },
        ),
    ] {
        let script = Script::new([
            Step::Inspect(Ok(waiting(60))),
            step,
            Step::Inspect(Ok(after)),
        ]);
        assert_eq!(
            exchange(&script, finish(decision), &AtomicBool::new(false), |_| {}).await,
            observed(after)
        );
        script.done();
    }
}

#[tokio::test]
async fn lost_decision_replies_require_inspection_not_reexecution() {
    for (decision, step) in [
        (RemoteWifiDecision::Keep, Step::Keep(Err(lost()))),
        (RemoteWifiDecision::Restore, Step::Restore(Err(lost()))),
    ] {
        let script = Script::new([Step::Inspect(Ok(waiting(60))), step]);
        assert!(matches!(
            exchange(&script, finish(decision), &AtomicBool::new(false), |_| {}).await,
            RemoteWifiStatus::OutcomeUnknown { .. }
        ));
        script.done();
    }
}

#[tokio::test]
async fn lost_post_write_observation_does_not_invent_success() {
    let script = Script::new([
        Step::Inspect(Ok(waiting(60))),
        Step::Keep(Ok(Apply::Applied)),
        Step::Inspect(Err(lost())),
    ]);
    assert!(matches!(
        exchange(
            &script,
            finish(RemoteWifiDecision::Keep),
            &AtomicBool::new(false),
            |_| {}
        )
        .await,
        RemoteWifiStatus::OutcomeUnknown { .. }
    ));
    script.done();
}

#[tokio::test]
async fn timeout_reboot_or_already_confirmed_are_observed_without_writes() {
    for after in [
        Transaction::FactoryProvisioning,
        Transaction::Confirmed { revision: rev(1) },
        Transaction::Confirmed { revision: rev(2) },
        Transaction::RollingBack {
            rejected_revision: rev(2),
        },
    ] {
        for decision in [RemoteWifiDecision::Keep, RemoteWifiDecision::Restore] {
            let script = Script::new([Step::Inspect(Ok(after))]);
            let dispatched = AtomicBool::new(false);
            assert_eq!(
                exchange(&script, finish(decision), &dispatched, |_| {}).await,
                observed(after)
            );
            assert!(!dispatched.load(Ordering::Acquire));
            script.done();
        }
    }
}

#[tokio::test]
async fn foreign_transaction_and_inspection_failure_prevent_any_write() {
    let script = Script::new([Step::Inspect(Err(protocol(
        core::RemoteControlProtocolError::UnsupportedRequest {
            request: core::RemoteControlRequestKind::InspectWifiTransaction,
        },
    )))]);
    let dispatched = AtomicBool::new(false);
    assert!(matches!(
        exchange(&script, start(), &dispatched, |_| {}).await,
        RemoteWifiStatus::Failed {
            stage: RemoteManagementFailureStage::Unsupported,
            ..
        }
    ));
    assert!(!dispatched.load(Ordering::Acquire));
    script.done();
}

#[tokio::test]
async fn persistence_and_rollback_failures_are_explicit() {
    for (decision, step, expected) in [
        (
            RemoteWifiDecision::Keep,
            Step::Keep(Err(protocol(
                core::RemoteControlProtocolError::PersistenceFailed {
                    request: core::RemoteControlRequestKind::ConfirmWifiCredentials,
                },
            ))),
            RemoteManagementFailureStage::Persistence,
        ),
        (
            RemoteWifiDecision::Restore,
            Step::Restore(Err(protocol(
                core::RemoteControlProtocolError::RollbackFailed {
                    request: core::RemoteControlRequestKind::CancelWifiCredentials,
                },
            ))),
            RemoteManagementFailureStage::Rollback,
        ),
    ] {
        let script = Script::new([Step::Inspect(Ok(waiting(90))), step]);
        assert!(
            matches!(exchange(&script, finish(decision), &AtomicBool::new(false), |_| {}).await, RemoteWifiStatus::Failed { stage, .. } if stage == expected)
        );
        script.done();
    }
}
