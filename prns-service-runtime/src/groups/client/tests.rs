#![allow(clippy::unwrap_used)]
use super::*;
use prns_storage::{OwnerEvent, OwnerProgress, Worker};
use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[test]
fn repeated_submission_clients_share_input_until_the_accepted_job_finishes() {
    let released = Arc::new(AtomicBool::new(false));
    let gate = released.clone();
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let (held_tx, held_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let mut worker = Worker::<owned::Job<Command>, ()>::start(
        "group-input-fixture",
        NonZeroUsize::new(8).unwrap(),
        move |inbox| {
            wake_tx.send(inbox.progress_waker()).unwrap();
            let mut held = None;
            inbox.run_owned(|event, _| {
                if let OwnerEvent::Job(job) = event {
                    assert!(held.is_none());
                    held = Some(job);
                    held_tx.send(()).unwrap();
                }
                if gate.load(Ordering::Acquire) && held.is_some() {
                    drop(held.take());
                    done_tx.send(()).unwrap();
                }
                if held.is_some() {
                    OwnerProgress::Pending
                } else {
                    OwnerProgress::Drained
                }
            });
            Ok(())
        },
    )
    .unwrap();
    let input_bytes = (std::mem::size_of::<groups::Job>() + 4) as u64;
    let submission = Submission::new(
        owned::Commands::from_lane(worker.client()),
        SubmissionState::new::<1>(
            groups::InputBudget::new(groups::ClientLimits {
                max_job_bytes: input_bytes,
                max_pending_job_bytes: input_bytes,
            })
            .unwrap(),
        )
        .unwrap(),
    );
    let identity = core::IdentityReference {
        id: core::LocalIdentityId {
            incarnation: [1; 16],
            sequence: 1,
        },
        public_hash: core::PublicIdentityHash([2; 16]),
    };
    let first = client(submission.clone(), identity, |command| command);
    let request = || groups::Request {
        id: groups::create_request_id().unwrap(),
        original_scope: None,
        intent: groups::Intent::Create {
            title: "held".into(),
            profile: groups::GroupProfile::default(),
        },
    };
    let context = groups::ExecutionContext {
        clock: prns_groups_core::ClockReading::Unknown,
    };
    drop(first.admit_execute(request(), context).unwrap());
    held_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let second = client(submission.clone(), identity, |command| command);
    assert!(matches!(
        second.admit_execute(request(), context),
        Err(groups::ApiError::Busy)
    ));
    released.store(true, Ordering::Release);
    wake_rx.recv_timeout(Duration::from_secs(3)).unwrap().wake();
    done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    // The ordinary lane had space throughout. Only completion of the retained
    // job, never dropping its waiter or making another client, frees the budget.
    drop(second.admit_execute(request(), context).unwrap());
    done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.close().unwrap();
    let after_close = client(submission, identity, |command| command);
    assert!(matches!(
        after_close.admit_execute(request(), context),
        Err(groups::ApiError::Unavailable)
    ));
    assert!(
        matches!(
            after_close.admit_execute(request(), context),
            Err(groups::ApiError::Unavailable)
        ),
        "rejected queue admission leaked input allowance"
    );
}
