//! Bounded reads and actor-owned changes. A dropped foreign write waiter is not an undo.
use super::*;

const MANAGEMENT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct ReadCommand {
    input: ReadRemoteNodeInput,
    response: Reply<ReadRemoteNodeOutcome>,
    deadline: tokio::time::Instant,
    caller: oneshot::Receiver<()>,
    generation: u64,
}

fn read_failure(stage: RemoteManagementFailureStage, detail: &str) -> ReadRemoteNodeOutcome {
    ReadRemoteNodeOutcome::Failed {
        stage,
        detail: detail.to_owned(),
    }
}

pub(super) async fn admit_read(
    supervisor: &Supervisor,
    input: ReadRemoteNodeInput,
) -> ReadRemoteNodeOutcome {
    let (caller, cancelled) = oneshot::channel();
    let deadline = tokio::time::Instant::now() + MANAGEMENT_TIMEOUT;
    let admitted = (|| {
        let fail =
            |detail: &str| Box::new(read_failure(RemoteManagementFailureStage::Node, detail));
        let state = admission::try_state(supervisor).map_err(fail)?;
        let worker = admission::running_worker(supervisor, &state).map_err(fail)?;
        if supervisor
            .operation_admitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Box::new(ReadRemoteNodeOutcome::Busy));
        }
        let permit = worker.commands.try_reserve().map_err(|error| {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            match error {
                mpsc::error::TrySendError::Full(_) => Box::new(ReadRemoteNodeOutcome::Busy),
                mpsc::error::TrySendError::Closed(_) => {
                    fail("The node stopped before the read was accepted.")
                }
            }
        })?;
        let (response, receiver) = oneshot::channel();
        let generation = supervisor.snapshots.read().generation_id;
        if !publish_read(&supervisor.snapshots, generation) {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return Err(fail("The node stopped before the read was accepted."));
        }
        permit.send(Command::RemoteRead(ReadCommand {
            input,
            response,
            deadline,
            caller: cancelled,
            generation,
        }));
        Ok(receiver)
    })();
    let outcome = match admitted {
        Ok(receiver) => admission::bounded_reply(
            receiver,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
        .unwrap_or_else(|_| {
            read_failure(
                RemoteManagementFailureStage::Timeout,
                "The node did not respond in time.",
            )
        }),
        Err(outcome) => *outcome,
    };
    drop(caller);
    outcome
}

fn publish_read(snapshots: &SnapshotStore, generation: u64) -> bool {
    let mut published = false;
    snapshots.update(|snapshot| {
        // The worker can report a terminal failure without taking the lifecycle
        // state lock. Do not overwrite its terminal operation state after admission.
        if snapshot.runtime == DevelopmentNodeRuntime::Running
            && snapshot.generation_id == generation
        {
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::RemoteRead,
                started_at_millis: wall_clock_millis(),
            });
            published = true;
        }
    });
    published
}

pub(super) fn admit_change(
    supervisor: &Supervisor,
    input: ChangeRemoteNodeInput,
) -> ChangeRemoteNodeOutcome {
    static NEXT_CHANGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let failure = |stage, detail: String| ChangeRemoteNodeOutcome::Failed { stage, detail };
    if let Err(detail) = crate::remote_control::management::validate_change_input(&input) {
        return failure(RemoteManagementFailureStage::Input, detail);
    }
    let Ok(state) = admission::try_state(supervisor) else {
        return ChangeRemoteNodeOutcome::Busy;
    };
    let worker = match admission::running_worker(supervisor, &state) {
        Ok(worker) => worker,
        Err(detail) => return failure(RemoteManagementFailureStage::Node, detail.to_owned()),
    };
    if supervisor
        .operation_admitted
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ChangeRemoteNodeOutcome::Busy;
    }
    let permit = match worker.commands.try_reserve() {
        Ok(permit) => permit,
        Err(error) => {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return match error {
                mpsc::error::TrySendError::Full(_) => ChangeRemoteNodeOutcome::Busy,
                mpsc::error::TrySendError::Closed(_) => failure(
                    RemoteManagementFailureStage::Node,
                    "The node stopped before the change was accepted.".to_owned(),
                ),
            };
        }
    };
    let Ok(operation_id) =
        NEXT_CHANGE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return ChangeRemoteNodeOutcome::Busy;
    };
    let operation = RemoteChangeOperation {
        operation_id,
        generation_id: supervisor.snapshots.read().generation_id,
        target_identity_fingerprint: input.target_identity_fingerprint,
        change: input.change,
        status: RemoteChangeStatus::Pending,
    };
    let mut published = false;
    supervisor.snapshots.update(|snapshot| {
        if snapshot.runtime == DevelopmentNodeRuntime::Running
            && snapshot.generation_id == operation.generation_id
        {
            snapshot.last_remote_change = Some(operation.clone());
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::RemoteChange,
                started_at_millis: wall_clock_millis(),
            });
            published = true;
        }
    });
    if !published {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return failure(
            RemoteManagementFailureStage::Node,
            "The node stopped before the change was accepted.".to_owned(),
        );
    }
    permit.send(Command::RemoteChange(
        operation.clone(),
        tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
    ));
    ChangeRemoteNodeOutcome::Accepted { operation }
}

pub(super) struct Admission<'a> {
    pub(super) snapshots: &'a SnapshotStore,
    pub(super) admitted: &'a AtomicBool,
    pub(super) generation: u64,
    pub(super) kind: DevelopmentNodeOperationKind,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.snapshots.update(|snapshot| {
            if snapshot.generation_id == self.generation
                && snapshot
                    .active_operation
                    .as_ref()
                    .is_some_and(|operation| operation.kind == self.kind)
            {
                snapshot.active_operation = None;
            }
        });
        self.admitted.store(false, Ordering::Release);
    }
}

pub(super) async fn stopping(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() || shutdown.changed().await.is_err() {
            return;
        }
    }
}

pub(super) async fn run_read<F>(
    command: ReadCommand,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    operation: impl FnOnce(ReadRemoteNodeInput, tokio::time::Instant) -> F,
) -> bool
where
    F: std::future::Future<Output = ReadRemoteNodeOutcome>,
{
    let ReadCommand {
        input,
        response,
        deadline,
        mut caller,
        generation,
    } = command;
    let _admission = Admission {
        snapshots,
        admitted,
        generation,
        kind: DevelopmentNodeOperationKind::RemoteRead,
    };
    let (outcome, stop) = tokio::select! {
        biased;
        () = stopping(shutdown) => (read_failure(RemoteManagementFailureStage::Node, "The node stopped during the read."), true),
        _ = &mut caller => (read_failure(RemoteManagementFailureStage::Timeout, "The read was cancelled."), false),
        () = tokio::time::sleep_until(deadline) => (read_failure(RemoteManagementFailureStage::Timeout, "The node did not respond in time."), false),
        outcome = async { operation(input, deadline).await } => (outcome, false),
    };
    let _ = response.send(outcome);
    stop
}

fn interrupted(dispatched: bool, stopped: bool) -> RemoteChangeStatus {
    if dispatched {
        RemoteChangeStatus::OutcomeUnknown {
            reason: if stopped {
                RemoteControlAnnounceUnknownReason::NodeStopped
            } else {
                RemoteControlAnnounceUnknownReason::Timeout
            },
        }
    } else {
        RemoteChangeStatus::Failed {
            stage: if stopped {
                RemoteManagementFailureStage::Node
            } else {
                RemoteManagementFailureStage::Timeout
            },
            detail: "The change was not sent to the node.".to_owned(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_change(
    operation: RemoteChangeOperation,
    deadline: tokio::time::Instant,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    handle: &PrnsNodeHandle,
    pairing: bool,
) -> bool {
    run_change_with(
        operation,
        deadline,
        snapshots,
        admitted,
        shutdown,
        |input, dispatched| async move {
            if pairing {
                RemoteChangeStatus::Failed {
                    stage: RemoteManagementFailureStage::Busy,
                    detail: "Finish pairing before changing the node.".to_owned(),
                }
            } else {
                crate::remote_control::management::change(handle, input, deadline, &dispatched)
                    .await
            }
        },
    )
    .await
}

async fn run_change_with<F>(
    operation: RemoteChangeOperation,
    deadline: tokio::time::Instant,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    execute: impl FnOnce(ChangeRemoteNodeInput, Arc<AtomicBool>) -> F,
) -> bool
where
    F: std::future::Future<Output = RemoteChangeStatus>,
{
    let _admission = Admission {
        snapshots,
        admitted,
        generation: operation.generation_id,
        kind: DevelopmentNodeOperationKind::RemoteChange,
    };
    // The dispatch marker is owned by this actor operation, not its foreign waiter.
    let dispatched = Arc::new(AtomicBool::new(false));
    let input = ChangeRemoteNodeInput {
        target_identity_fingerprint: operation.target_identity_fingerprint.clone(),
        change: operation.change.clone(),
    };
    let (status, stop) = tokio::select! {
        biased;
        () = stopping(shutdown) => (interrupted(dispatched.load(Ordering::Acquire), true), true),
        () = tokio::time::sleep_until(deadline) => (interrupted(dispatched.load(Ordering::Acquire), false), false),
        status = async { execute(input, dispatched.clone()).await } => (status, false),
    };
    complete_change(snapshots, &operation, status);
    stop
}

fn complete_change(
    snapshots: &SnapshotStore,
    operation: &RemoteChangeOperation,
    status: RemoteChangeStatus,
) {
    snapshots.update(|snapshot| {
        if snapshot.generation_id != operation.generation_id {
            return;
        }
        if let Some(current) = snapshot.last_remote_change.as_mut().filter(|current| {
            current.operation_id == operation.operation_id
                && current.generation_id == operation.generation_id
        }) {
            current.status = status;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_change(store: &SnapshotStore) -> RemoteChangeOperation {
        let operation = RemoteChangeOperation {
            operation_id: 1,
            generation_id: store.read().generation_id,
            target_identity_fingerprint: vec![1; 16],
            change: RemoteNodeChange::WakeRadios,
            status: RemoteChangeStatus::Pending,
        };
        store.update(|snapshot| {
            snapshot.last_remote_change = Some(operation.clone());
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::RemoteChange,
                started_at_millis: 0,
            });
        });
        operation
    }

    #[test]
    fn read_publication_does_not_overwrite_terminal_or_new_generation_state() {
        let store = SnapshotStore::new();
        store.set_runtime(DevelopmentNodeRuntime::Failed);
        assert!(!publish_read(&store, 0));
        assert!(store.read().active_operation.is_none());
        store.set_runtime(DevelopmentNodeRuntime::Running);
        store.update(|snapshot| snapshot.generation_id = 3);
        assert!(!publish_read(&store, 2));
        assert!(store.read().active_operation.is_none());
        assert!(publish_read(&store, 3));
        assert_eq!(
            store.read().active_operation.unwrap().kind,
            DevelopmentNodeOperationKind::RemoteRead
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_after_write_dispatch_is_unknown_without_reexecution() {
        let store = SnapshotStore::new();
        let operation = pending_change(&store);
        let admitted = AtomicBool::new(true);
        let (_stop, mut shutdown) = watch::channel(false);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let calls_ref = &calls;
        run_change_with(
            operation,
            tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
            &store,
            &admitted,
            &mut shutdown,
            |_, dispatched| async move {
                calls_ref.fetch_add(1, Ordering::Relaxed);
                dispatched.store(true, Ordering::Release);
                std::future::pending().await
            },
        )
        .await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            store.read().last_remote_change.unwrap().status,
            RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::Timeout
            }
        );
        assert!(!admitted.load(Ordering::Acquire));
        assert!(store.read().active_operation.is_none());
    }

    #[tokio::test]
    async fn cancelling_a_running_read_drops_work_and_keeps_shutdown_visible() {
        struct Work<'a>(&'a AtomicBool);
        impl Drop for Work<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let store = SnapshotStore::new();
        let admitted = AtomicBool::new(true);
        let dropped = AtomicBool::new(false);
        let (caller, cancelled) = oneshot::channel();
        let (response, _receiver) = oneshot::channel();
        let (_stop, mut shutdown) = watch::channel(false);
        run_read(
            ReadCommand {
                input: ReadRemoteNodeInput {
                    target_identity_fingerprint: vec![1; 16],
                    query: RemoteNodeQuery::Overview,
                },
                response,
                deadline: tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
                caller: cancelled,
                generation: 0,
            },
            &store,
            &admitted,
            &mut shutdown,
            |_, _| async {
                let _work = Work(&dropped);
                store.begin_stop(7);
                drop(caller);
                std::future::pending().await
            },
        )
        .await;
        assert!(dropped.load(Ordering::Relaxed));
        assert!(!admitted.load(Ordering::Acquire));
        assert_eq!(
            store.read().active_operation.unwrap().kind,
            DevelopmentNodeOperationKind::Shutdown
        );
    }

    #[tokio::test]
    async fn expired_queued_change_never_invokes_network() {
        let store = SnapshotStore::new();
        let operation = pending_change(&store);
        let admitted = AtomicBool::new(true);
        let (_stop, mut shutdown) = watch::channel(false);
        let invoked = AtomicBool::new(false);
        run_change_with(
            operation,
            tokio::time::Instant::now() - Duration::from_secs(1),
            &store,
            &admitted,
            &mut shutdown,
            |_, _| async {
                invoked.store(true, Ordering::Relaxed);
                RemoteChangeStatus::Applied
            },
        )
        .await;
        assert!(!invoked.load(Ordering::Relaxed));
        assert!(!admitted.load(Ordering::Acquire));
        assert!(store.read().active_operation.is_none());
        assert!(matches!(
            store.read().last_remote_change.unwrap().status,
            RemoteChangeStatus::Failed {
                stage: RemoteManagementFailureStage::Timeout,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn change_continues_without_foreign_waiter_and_records_success() {
        let store = SnapshotStore::new();
        let operation = pending_change(&store);
        let admitted = AtomicBool::new(true);
        let (_stop, mut shutdown) = watch::channel(false);
        // Admission has already returned its owned record; no caller channel is
        // retained by the actor, so dropping a JS/Swift/Kotlin waiter cannot undo it.
        run_change_with(
            operation,
            tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
            &store,
            &admitted,
            &mut shutdown,
            |_, dispatched| async move {
                dispatched.store(true, Ordering::Release);
                tokio::task::yield_now().await;
                RemoteChangeStatus::Applied
            },
        )
        .await;
        assert_eq!(
            store.read().last_remote_change.unwrap().status,
            RemoteChangeStatus::Applied
        );
        assert!(!admitted.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn shutdown_after_dispatch_is_unknown_and_releases_admission() {
        let store = SnapshotStore::new();
        let operation = pending_change(&store);
        let admitted = AtomicBool::new(true);
        let (stop, mut shutdown) = watch::channel(false);
        assert!(
            run_change_with(
                operation,
                tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
                &store,
                &admitted,
                &mut shutdown,
                |_, dispatched| async move {
                    dispatched.store(true, Ordering::Release);
                    stop.send(true).unwrap();
                    std::future::pending().await
                }
            )
            .await
        );
        assert_eq!(
            store.read().last_remote_change.unwrap().status,
            RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped
            }
        );
        assert!(!admitted.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn dropped_read_caller_never_invokes_network_and_releases_admission() {
        let store = SnapshotStore::new();
        let admitted = AtomicBool::new(true);
        let (caller, cancelled) = oneshot::channel();
        drop(caller);
        let (response, receiver) = oneshot::channel();
        let (_shutdown, mut shutdown) = watch::channel(false);
        let invoked = AtomicBool::new(false);
        assert!(
            !run_read(
                ReadCommand {
                    input: ReadRemoteNodeInput {
                        target_identity_fingerprint: vec![1; 16],
                        query: RemoteNodeQuery::Overview
                    },
                    response,
                    deadline: tokio::time::Instant::now() + MANAGEMENT_TIMEOUT,
                    caller: cancelled,
                    generation: 0,
                },
                &store,
                &admitted,
                &mut shutdown,
                |_, _| async {
                    invoked.store(true, Ordering::Relaxed);
                    ReadRemoteNodeOutcome::Busy
                }
            )
            .await
        );
        assert!(!invoked.load(Ordering::Relaxed));
        assert!(!admitted.load(Ordering::Acquire));
        assert!(matches!(
            receiver.await.unwrap(),
            ReadRemoteNodeOutcome::Failed { .. }
        ));
    }

    #[test]
    fn only_dispatched_changes_become_unknown_after_interruption() {
        assert!(matches!(
            interrupted(false, true),
            RemoteChangeStatus::Failed {
                stage: RemoteManagementFailureStage::Node,
                ..
            }
        ));
        assert!(matches!(
            interrupted(false, false),
            RemoteChangeStatus::Failed {
                stage: RemoteManagementFailureStage::Timeout,
                ..
            }
        ));
        assert!(matches!(
            interrupted(true, false),
            RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::Timeout
            }
        ));
        assert!(matches!(
            interrupted(true, true),
            RemoteChangeStatus::OutcomeUnknown {
                reason: RemoteControlAnnounceUnknownReason::NodeStopped
            }
        ));
    }

    #[test]
    fn retired_generation_cannot_settle_current_change() {
        let store = SnapshotStore::new();
        let operation = RemoteChangeOperation {
            operation_id: 1,
            generation_id: 2,
            target_identity_fingerprint: vec![1; 16],
            change: RemoteNodeChange::WakeRadios,
            status: RemoteChangeStatus::Pending,
        };
        store.update(|snapshot| {
            snapshot.generation_id = 3;
            snapshot.last_remote_change = Some(operation.clone());
        });
        complete_change(&store, &operation, RemoteChangeStatus::Applied);
        assert_eq!(
            store.read().last_remote_change.unwrap().status,
            RemoteChangeStatus::Pending
        );
    }
}
