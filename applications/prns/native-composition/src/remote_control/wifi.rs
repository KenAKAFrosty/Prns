//! Guided Wi-Fi transactions over the authorized public target handle.
//! No automatic mutation retries; a lost response is reconciled by explicit inspection.
use std::sync::atomic::{AtomicBool, Ordering};

use personal_rns::prelude::{
    PrnsNodeHandle, RemoteControlTargetHandle, RemoteControlTargetOperationError,
};
use personal_rns::remote_control as core;
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::{management, CloseTargetLink};
use crate::contract::*;

pub(crate) enum Work {
    Start(core::RemoteControlWifiStation),
    Inspect,
    Finish {
        revision: core::RemoteControlWifiCredentialRevision,
        decision: RemoteWifiDecision,
    },
}

pub(crate) fn prepare_start(input: StartRemoteWifiTrialInput) -> Result<(Vec<u8>, Work), String> {
    let ssid = Zeroizing::new(input.ssid);
    let password = Zeroizing::new(input.password);
    if super::identity_hash(&input.target_identity_fingerprint).is_none() {
        return Err("Choose a valid paired node.".to_owned());
    }
    let station = core::RemoteControlWifiStation::parse(&ssid, &password).map_err(|error| {
        match error {
            core::RemoteControlWifiStationParseError::EmptySsid => "Enter the network name.",
            core::RemoteControlWifiStationParseError::SsidTooLong => {
                "The network name must use at most 32 UTF-8 bytes."
            }
            core::RemoteControlWifiStationParseError::PasswordTooLong => {
                "The password must use at most 64 UTF-8 bytes."
            }
        }
        .to_owned()
    })?;
    Ok((input.target_identity_fingerprint, Work::Start(station)))
}

fn failed(stage: RemoteManagementFailureStage, detail: impl Into<String>) -> RemoteWifiStatus {
    RemoteWifiStatus::Failed {
        stage,
        detail: detail.into(),
    }
}

fn read_error(error: RemoteControlTargetOperationError) -> RemoteWifiStatus {
    let (stage, detail) = management::read_failure(error);
    failed(stage, detail)
}

fn write_error(error: RemoteControlTargetOperationError) -> RemoteWifiStatus {
    match management::mutation_failure(error) {
        RemoteChangeStatus::Failed { stage, detail } => failed(stage, detail),
        RemoteChangeStatus::OutcomeUnknown { reason } => {
            RemoteWifiStatus::OutcomeUnknown { reason }
        }
        _ => failed(
            RemoteManagementFailureStage::Response,
            "The node returned an unexpected Wi-Fi result.",
        ),
    }
}

fn observed(transaction: core::RemoteControlWifiTransactionStatus) -> RemoteWifiStatus {
    use core::RemoteControlWifiTransactionStatus as Status;
    RemoteWifiStatus::Observed {
        transaction: match transaction {
            Status::FactoryProvisioning => RemoteWifiTransaction::FactoryProvisioning,
            Status::Confirmed { revision } => RemoteWifiTransaction::Confirmed {
                revision: revision.get(),
            },
            Status::Staged { revision } => RemoteWifiTransaction::Staged {
                revision: revision.get(),
            },
            Status::AwaitingConfirmation {
                revision,
                remaining,
            } => RemoteWifiTransaction::AwaitingConfirmation {
                revision: revision.get(),
                remaining_seconds: remaining.seconds(),
            },
            Status::RollingBack { rejected_revision } => RemoteWifiTransaction::RollingBack {
                rejected_revision: rejected_revision.get(),
            },
        },
    }
}

pub(crate) async fn execute(
    handle: &PrnsNodeHandle,
    target: &[u8],
    work: Work,
    deadline: Instant,
    dispatched: &AtomicBool,
    publish_candidate: impl Fn(u32),
) -> RemoteWifiStatus {
    let connected = match management::connect(handle, target, deadline).await {
        Ok(target) => target,
        Err((stage, detail)) => return failed(stage, detail),
    };
    let _link = CloseTargetLink {
        handle,
        id: connected.connection().link_id(),
    };
    let description = match connected.describe().await {
        Ok((value, _)) => value,
        Err(error) => return read_error(error),
    };
    use core::RemoteControlRequestKind as Kind;
    let kinds: &[Kind] = match &work {
        Work::Start(_) => &[
            Kind::InspectWifiTransaction,
            Kind::StageWifiCredentials,
            Kind::ActivateWifiCredentials,
            Kind::ConfirmWifiCredentials,
            Kind::CancelWifiCredentials,
        ],
        Work::Inspect => &[Kind::InspectWifiTransaction],
        Work::Finish {
            decision: RemoteWifiDecision::Keep,
            ..
        } => &[Kind::InspectWifiTransaction, Kind::ConfirmWifiCredentials],
        Work::Finish {
            decision: RemoteWifiDecision::Restore,
            ..
        } => &[Kind::InspectWifiTransaction, Kind::CancelWifiCredentials],
    };
    for kind in kinds {
        if let Err((stage, detail)) = management::require(description.available_requests(), *kind) {
            return failed(stage, detail);
        }
    }
    exchange_with_deadline(&connected, work, dispatched, publish_candidate, deadline).await
}

trait Exchange {
    async fn inspect(
        &self,
    ) -> Result<core::RemoteControlWifiTransactionStatus, RemoteControlTargetOperationError>;
    async fn stage(
        &self,
        station: core::RemoteControlWifiStation,
    ) -> Result<core::RemoteControlWifiStageOutcome, RemoteControlTargetOperationError>;
    async fn activate(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError>;
    async fn keep(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError>;
    async fn restore(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError>;
}

impl Exchange for RemoteControlTargetHandle<'_> {
    async fn inspect(
        &self,
    ) -> Result<core::RemoteControlWifiTransactionStatus, RemoteControlTargetOperationError> {
        self.inspect_wifi_transaction().await.map(|value| value.0)
    }
    async fn stage(
        &self,
        station: core::RemoteControlWifiStation,
    ) -> Result<core::RemoteControlWifiStageOutcome, RemoteControlTargetOperationError> {
        self.stage_wifi_credentials(station)
            .await
            .map(|value| value.0)
    }
    async fn activate(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError> {
        self.activate_wifi_credentials(revision)
            .await
            .map(|value| value.0)
    }
    async fn keep(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError> {
        self.confirm_wifi_credentials(revision)
            .await
            .map(|value| value.0)
    }
    async fn restore(
        &self,
        revision: core::RemoteControlWifiCredentialRevision,
    ) -> Result<core::RemoteControlApplyOutcome, RemoteControlTargetOperationError> {
        self.cancel_wifi_credentials(revision)
            .await
            .map(|value| value.0)
    }
}

async fn exchange_with_deadline(
    target: &impl Exchange,
    work: Work,
    dispatched: &AtomicBool,
    publish_candidate: impl Fn(u32),
    deadline: Instant,
) -> RemoteWifiStatus {
    use core::RemoteControlWifiTransactionStatus as Transaction;
    let before = match target.inspect().await {
        Ok(value) => value,
        Err(error) => return read_error(error),
    };
    let scheduled = match work {
        Work::Inspect => return observed(before),
        Work::Start(station) => {
            if !matches!(
                before,
                Transaction::FactoryProvisioning | Transaction::Confirmed { .. }
            ) {
                // A previous or recovered trial must be resolved explicitly. In particular,
                // Stage is not idempotent: it replaces this controller's staged candidate.
                return observed(before);
            }
            dispatched.store(true, Ordering::Release);
            let revision = match target.stage(station).await {
                Ok(core::RemoteControlWifiStageOutcome::Staged(revision)) => revision,
                Ok(core::RemoteControlWifiStageOutcome::InvalidCredentials) => {
                    return failed(
                        RemoteManagementFailureStage::Input,
                        "The node could not use those network credentials.",
                    );
                }
                Err(error) => return write_error(error),
            };
            publish_candidate(revision.get());
            match target.activate(revision).await {
                Ok(_) => return observe_activation(target, revision, deadline).await,
                Err(error) => return write_error(error),
            }
        }
        Work::Finish { revision, decision } => {
            let active_revision = match before {
                Transaction::Staged { revision }
                | Transaction::AwaitingConfirmation { revision, .. } => revision,
                // The target is already terminal. Report its actual state without claiming
                // this command changed anything or replaying a previous decision.
                Transaction::FactoryProvisioning
                | Transaction::Confirmed { .. }
                | Transaction::RollingBack { .. } => return observed(before),
            };
            if revision != active_revision {
                return failed(
                    RemoteManagementFailureStage::Input,
                    "The pending network change has changed. Check its status again.",
                );
            }
            if decision == RemoteWifiDecision::Keep
                && !matches!(before, Transaction::AwaitingConfirmation { remaining, .. } if remaining.seconds() > 0)
            {
                return failed(
                    RemoteManagementFailureStage::Request,
                    "This network change cannot be kept. Check its status or restore the previous network.",
                );
            }
            dispatched.store(true, Ordering::Release);
            let result = match decision {
                RemoteWifiDecision::Keep => target.keep(revision).await,
                RemoteWifiDecision::Restore => target.restore(revision).await,
            };
            match result {
                Ok(outcome) => outcome == core::RemoteControlApplyOutcome::Scheduled,
                Err(error) => {
                    let status = write_error(error);
                    return match status {
                        RemoteWifiStatus::Failed {
                            stage: RemoteManagementFailureStage::Busy,
                            ..
                        } if decision == RemoteWifiDecision::Keep => failed(
                            RemoteManagementFailureStage::Busy,
                            "The node is busy or still connecting. Check again before keeping this network.",
                        ),
                        _ => status,
                    };
                }
            }
        }
    };
    if scheduled {
        // Restoration has the same 250 ms response grace. Yield beyond it once;
        // subsequent observation, not the grace delay, determines the reported state.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    match target.inspect().await {
        Ok(value) => observed(value),
        // A definitive mutation reply is not a definitive observation of the ensuing
        // scheduled effect. Never pretend the Wi-Fi connection succeeded or rolled back.
        Err(_) => RemoteWifiStatus::OutcomeUnknown {
            reason: RemoteControlAnnounceUnknownReason::DeliveryUnconfirmed,
        },
    }
}

/// Activation can be persisted well before its scheduled radio effect executes.
/// AwaitingConfirmation(0) is ambiguous during that interval: it is not evidence
/// that the freshly started trial has expired. Reconcile with bounded reads only;
/// a completed confirmation/rollback or changed revision must never be hidden.
async fn observe_activation(
    target: &impl Exchange,
    revision: core::RemoteControlWifiCredentialRevision,
    operation_deadline: Instant,
) -> RemoteWifiStatus {
    use core::RemoteControlWifiTransactionStatus as Transaction;
    let deadline = operation_deadline.min(Instant::now() + std::time::Duration::from_secs(5));
    loop {
        let value = match tokio::time::timeout_at(deadline, target.inspect()).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => {
                return RemoteWifiStatus::OutcomeUnknown {
                    reason: RemoteControlAnnounceUnknownReason::DeliveryUnconfirmed,
                }
            }
            Err(_) => {
                return RemoteWifiStatus::OutcomeUnknown {
                    reason: RemoteControlAnnounceUnknownReason::Timeout,
                }
            }
        };
        if !matches!(value, Transaction::AwaitingConfirmation { revision: pending, remaining } if pending == revision && remaining.seconds() == 0)
        {
            return observed(value);
        }
        if Instant::now() >= deadline {
            return observed(value);
        }
        tokio::time::sleep_until(
            (Instant::now() + std::time::Duration::from_millis(500)).min(deadline),
        )
        .await;
        if Instant::now() >= deadline {
            // The last observation remains the only fact established. Do not
            // manufacture either a running timer or successful rollback.
            return observed(value);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "host-test"))]
mod tests_wire;
