//! Actor-owned Wi-Fi steps; waiting for the user's decision never owns the actor.
use super::*;
use crate::remote_control::wifi::{self, Work};

const WIFI_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct WifiCommand {
    operation: RemoteWifiOperation,
    work: Work,
    deadline: tokio::time::Instant,
}

fn input_failure(detail: impl Into<String>) -> RemoteWifiCommandOutcome {
    RemoteWifiCommandOutcome::Failed {
        stage: RemoteManagementFailureStage::Input,
        detail: detail.into(),
    }
}

pub(super) fn start(
    supervisor: &Supervisor,
    input: StartRemoteWifiTrialInput,
) -> RemoteWifiCommandOutcome {
    match wifi::prepare_start(input) {
        Ok((target, work)) => admit(supervisor, target, work, RemoteWifiAction::Start, None),
        Err(detail) => input_failure(detail),
    }
}

pub(super) fn inspect(
    supervisor: &Supervisor,
    input: InspectRemoteWifiTrialInput,
) -> RemoteWifiCommandOutcome {
    admit(
        supervisor,
        input.target_identity_fingerprint,
        Work::Inspect,
        RemoteWifiAction::Inspect,
        None,
    )
}

pub(super) fn finish(
    supervisor: &Supervisor,
    input: FinishRemoteWifiTrialInput,
) -> RemoteWifiCommandOutcome {
    let Some(revision) =
        personal_rns::remote_control::RemoteControlWifiCredentialRevision::new(input.revision)
    else {
        return input_failure("Check the pending network change before choosing an action.");
    };
    let action = match input.decision {
        RemoteWifiDecision::Keep => RemoteWifiAction::Keep,
        RemoteWifiDecision::Restore => RemoteWifiAction::Restore,
    };
    admit(
        supervisor,
        input.target_identity_fingerprint,
        Work::Finish {
            revision,
            decision: input.decision,
        },
        action,
        Some(input.revision),
    )
}

fn admit(
    supervisor: &Supervisor,
    target: Vec<u8>,
    work: Work,
    action: RemoteWifiAction,
    revision: Option<u32>,
) -> RemoteWifiCommandOutcome {
    static NEXT_WIFI: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if crate::remote_control::identity_hash(&target).is_none() {
        return input_failure("Choose a valid paired node.");
    }
    let Ok(state) = admission::try_state(supervisor) else {
        return RemoteWifiCommandOutcome::Busy;
    };
    let worker = match admission::running_worker(supervisor, &state) {
        Ok(worker) => worker,
        Err(detail) => {
            return RemoteWifiCommandOutcome::Failed {
                stage: RemoteManagementFailureStage::Node,
                detail: detail.to_owned(),
            };
        }
    };
    if supervisor
        .operation_admitted
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return RemoteWifiCommandOutcome::Busy;
    }
    let permit = match worker.commands.try_reserve() {
        Ok(permit) => permit,
        Err(_) => {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return RemoteWifiCommandOutcome::Busy;
        }
    };
    let Ok(operation_id) =
        NEXT_WIFI.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return RemoteWifiCommandOutcome::Busy;
    };
    let operation = RemoteWifiOperation {
        operation_id,
        generation_id: supervisor.snapshots.read().generation_id,
        target_identity_fingerprint: target,
        action,
        candidate_revision: revision,
        status: RemoteWifiStatus::Pending,
    };
    let mut published = false;
    supervisor.snapshots.update(|snapshot| {
        if snapshot.runtime == DevelopmentNodeRuntime::Running
            && snapshot.generation_id == operation.generation_id
        {
            snapshot.last_remote_wifi = Some(operation.clone());
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::RemoteWifi,
                started_at_millis: wall_clock_millis(),
            });
            published = true;
        }
    });
    if !published {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return RemoteWifiCommandOutcome::Failed {
            stage: RemoteManagementFailureStage::Node,
            detail: "The node stopped before the network action was accepted.".to_owned(),
        };
    }
    permit.send(Command::RemoteWifi(WifiCommand {
        operation: operation.clone(),
        work,
        deadline: tokio::time::Instant::now() + WIFI_TIMEOUT,
    }));
    RemoteWifiCommandOutcome::Accepted { operation }
}

pub(super) async fn run(
    command: WifiCommand,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    handle: &PrnsNodeHandle,
    pairing: bool,
) -> bool {
    let target = command.operation.target_identity_fingerprint.clone();
    let operation = command.operation.clone();
    run_with(
        command,
        snapshots,
        admitted,
        shutdown,
        |work, deadline, dispatched| async move {
            if pairing {
                RemoteWifiStatus::Failed {
                    stage: RemoteManagementFailureStage::Busy,
                    detail: "Finish pairing before changing the network.".to_owned(),
                }
            } else {
                wifi::execute(handle, &target, work, deadline, &dispatched, |revision| {
                    update(snapshots, &operation, |current| {
                        current.candidate_revision = Some(revision)
                    })
                })
                .await
            }
        },
    )
    .await
}

async fn run_with<F>(
    command: WifiCommand,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    execute: impl FnOnce(Work, tokio::time::Instant, Arc<AtomicBool>) -> F,
) -> bool
where
    F: std::future::Future<Output = RemoteWifiStatus>,
{
    let WifiCommand {
        operation,
        work,
        deadline,
    } = command;
    let _admission = management::Admission {
        snapshots,
        admitted,
        generation: operation.generation_id,
        kind: DevelopmentNodeOperationKind::RemoteWifi,
    };
    let dispatched = Arc::new(AtomicBool::new(false));
    let (status, stop) = tokio::select! {
        biased;
        () = management::stopping(shutdown) => (interrupted(dispatched.load(Ordering::Acquire), true), true),
        () = tokio::time::sleep_until(deadline) => (interrupted(dispatched.load(Ordering::Acquire), false), false),
        status = execute(work, deadline, dispatched.clone()) => (status, false),
    };
    update(snapshots, &operation, |current| current.status = status);
    stop
}

fn interrupted(dispatched: bool, stopped: bool) -> RemoteWifiStatus {
    if dispatched {
        RemoteWifiStatus::OutcomeUnknown {
            reason: if stopped {
                RemoteControlAnnounceUnknownReason::NodeStopped
            } else {
                RemoteControlAnnounceUnknownReason::Timeout
            },
        }
    } else {
        RemoteWifiStatus::Failed {
            stage: if stopped {
                RemoteManagementFailureStage::Node
            } else {
                RemoteManagementFailureStage::Timeout
            },
            detail: "The network action was not sent. Check the node's status before trying again."
                .to_owned(),
        }
    }
}

fn update(
    snapshots: &SnapshotStore,
    operation: &RemoteWifiOperation,
    change: impl FnOnce(&mut RemoteWifiOperation),
) {
    snapshots.update(|snapshot| {
        if snapshot.generation_id == operation.generation_id {
            if let Some(current) = snapshot.last_remote_wifi.as_mut().filter(|current| {
                current.operation_id == operation.operation_id
                    && current.generation_id == operation.generation_id
            }) {
                change(current);
            }
        }
    });
}

#[cfg(test)]
mod tests;
