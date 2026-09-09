//! Handwritten runtime-neutral admission used by generated foreign entry points. Only native bootstrap may do path I/O
//! or wait for lifecycle transitions. Async admission never waits for that lock.
use super::*;
use crate::contract::NativeStoragePreparationOutcome;
use prns_lxmf::mailbox::{MailboxFailure, MailboxReply, MailboxRequest};

const TRANSITION: &str = "The native lifecycle owner is transitioning. Try again when it is ready.";
const STORAGE_NOT_PREPARED: &str =
    "Native storage must be prepared on the platform queue before this operation.";

pub(crate) fn prepare_native_storage(storage_root: &Path) -> NativeStoragePreparationOutcome {
    prepare_native_storage_with_supervisor(supervisor(), storage_root)
}

pub(super) fn prepare_native_storage_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
) -> NativeStoragePreparationOutcome {
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    let prepared = prepare_storage(storage_root)
        .map_err(DevelopmentStoreFailure::unavailable)
        .and_then(|paths| ensure_application_owner_locked(&mut state, &paths).map(|_| ()));
    match prepared {
        Ok(()) => NativeStoragePreparationOutcome::Prepared,
        Err(DevelopmentStoreFailure::Unavailable(detail)) => {
            NativeStoragePreparationOutcome::Unavailable { detail }
        }
        Err(DevelopmentStoreFailure::ResetRequired(reason)) => {
            NativeStoragePreparationOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn try_state(supervisor: &Supervisor) -> Result<MutexGuard<'_, SupervisorState>, &'static str> {
    match supervisor.state.try_lock() {
        Ok(state) => Ok(state),
        Err(std::sync::TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => Err(TRANSITION),
    }
}

fn running_worker<'a>(
    supervisor: &Supervisor,
    state: &'a SupervisorState,
) -> Result<&'a Worker, &'static str> {
    if supervisor.snapshots.read().runtime != DevelopmentNodeRuntime::Running
        || supervisor.snapshots.is_explicit_stop_in_progress()
    {
        return Err("The native Prns node is not running.");
    }
    state
        .worker
        .as_ref()
        .ok_or("The native generation has no command authority.")
}

fn admit_running<T>(
    supervisor: &Supervisor,
    command: impl FnOnce(Reply<T>) -> Command,
) -> Result<oneshot::Receiver<T>, LxmfAdmissionFailure> {
    let state = try_state(supervisor).map_err(|_| LxmfAdmissionFailure::Busy)?;
    let worker =
        running_worker(supervisor, &state).map_err(|_| LxmfAdmissionFailure::LocalNodeStopped)?;
    let (response, receiver) = oneshot::channel();
    worker
        .commands
        .try_send(command(response))
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => LxmfAdmissionFailure::Busy,
            mpsc::error::TrySendError::Closed(_) => LxmfAdmissionFailure::LocalNodeStopped,
        })?;
    Ok(receiver)
}

/// This timer has no executor or node authority. In particular it remains usable
/// for offline database reads when the native Tokio node runtime does not exist.
async fn bounded_reply<T>(
    receiver: oneshot::Receiver<T>,
    timeout: Duration,
) -> Result<T, &'static str> {
    tokio::select! {
        biased;
        result = receiver => result.map_err(|_| "The native response owner stopped before replying."),
        () = futures_timer::Delay::new(timeout) => Err("The native operation exceeded its bounded wait."),
    }
}

async fn pairing(
    timeout: Duration,
    command: impl FnOnce(Reply<RemoteControlPairingCommandOutcome>) -> Command,
) -> RemoteControlPairingCommandOutcome {
    let supervisor = supervisor();
    let admitted = (|| {
        let state = try_state(supervisor)
            .map_err(|detail| pairing_failed(RemoteControlPairingFailureStage::Node, detail))?;
        let worker = running_worker(supervisor, &state)
            .map_err(|detail| pairing_failed(RemoteControlPairingFailureStage::Node, detail))?;
        if supervisor
            .operation_admitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RemoteControlPairingCommandOutcome::Busy);
        }
        let (response, receiver) = oneshot::channel();
        if let Err(error) = worker.commands.try_send(command(response)) {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return Err(match error {
                mpsc::error::TrySendError::Full(_) => RemoteControlPairingCommandOutcome::Busy,
                mpsc::error::TrySendError::Closed(_) => pairing_failed(
                    RemoteControlPairingFailureStage::Node,
                    "The native pairing command lane has closed.",
                ),
            });
        }
        Ok(receiver)
    })();
    match admitted {
        Ok(receiver) => bounded_reply(receiver, timeout)
            .await
            .unwrap_or_else(|detail| {
                pairing_failed(RemoteControlPairingFailureStage::Node, detail)
            }),
        Err(outcome) => outcome,
    }
}

pub async fn initiate_pairing(
    input: InitiateRemoteControlPairingInput,
) -> RemoteControlPairingCommandOutcome {
    pairing(COMMAND_TIMEOUT, |response| {
        Command::Initiate(input, response)
    })
    .await
}
pub async fn approve_pairing(
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    pairing(PAIRING_APPROVAL_TIMEOUT, |response| {
        Command::Approve(input, response)
    })
    .await
}
pub async fn reject_pairing(
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    pairing(COMMAND_TIMEOUT, |response| Command::Reject(input, response)).await
}

pub async fn describe_target(
    input: DescribeRemoteControlTargetInput,
) -> RemoteControlDescribeOutcome {
    describe_with_supervisor(supervisor(), input).await
}

pub(super) async fn describe_with_supervisor(
    supervisor: &Supervisor,
    input: DescribeRemoteControlTargetInput,
) -> RemoteControlDescribeOutcome {
    let (caller, cancelled) = oneshot::channel();
    let deadline = tokio::time::Instant::now() + COMMAND_TIMEOUT;
    let admitted = (|| {
        let fail = |detail: &str| {
            Box::new(RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Node,
                detail: detail.to_owned(),
            })
        };
        let state = try_state(supervisor).map_err(fail)?;
        let worker = running_worker(supervisor, &state).map_err(fail)?;
        if supervisor
            .operation_admitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Box::new(RemoteControlDescribeOutcome::Busy));
        }
        let (response, receiver) = oneshot::channel();
        if let Err(error) = worker.commands.try_send(Command::Describe(DescribeCommand {
            input,
            response,
            deadline,
            caller: cancelled,
        })) {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return Err(match error {
                mpsc::error::TrySendError::Full(_) => Box::new(RemoteControlDescribeOutcome::Busy),
                mpsc::error::TrySendError::Closed(_) => {
                    fail("The native Describe lane has closed.")
                }
            });
        }
        Ok(receiver)
    })();
    let outcome = match admitted {
        Ok(receiver) => bounded_reply(
            receiver,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
        .unwrap_or_else(|_| describe_timed_out()),
        Err(outcome) => *outcome,
    };
    // The actor owns releasing operation admission, including after caller drop.
    drop(caller);
    outcome
}

pub async fn announce_target(
    input: AnnounceRemoteControlTargetInput,
) -> RemoteControlAnnounceOutcome {
    announce_target_with_supervisor(supervisor(), input).await
}

pub(super) async fn announce_target_with_supervisor(
    supervisor: &Supervisor,
    input: AnnounceRemoteControlTargetInput,
) -> RemoteControlAnnounceOutcome {
    let Ok(state) = try_state(supervisor) else {
        return RemoteControlAnnounceOutcome::Busy;
    };
    announce_self_admitted(supervisor, &state, input)
}

pub async fn list_lxmf_peers() -> LxmfPeerListOutcome {
    match admit_running(supervisor(), Command::ListLxmfPeers) {
        Ok(receiver) => bounded_reply(receiver, LXMF_QUERY_TIMEOUT)
            .await
            .unwrap_or(LxmfPeerListOutcome::Busy),
        Err(LxmfAdmissionFailure::LocalNodeStopped) => LxmfPeerListOutcome::LocalNodeStopped,
        Err(LxmfAdmissionFailure::Busy) => LxmfPeerListOutcome::Busy,
    }
}

pub async fn measure_lxmf_text(input: MeasureLxmfTextInput) -> MeasureLxmfTextOutcome {
    match admit_running(supervisor(), |response| {
        Command::MeasureLxmfText(input, response)
    }) {
        Ok(receiver) => bounded_reply(receiver, LXMF_QUERY_TIMEOUT)
            .await
            .unwrap_or(MeasureLxmfTextOutcome::Busy),
        Err(LxmfAdmissionFailure::LocalNodeStopped) => MeasureLxmfTextOutcome::LocalNodeStopped,
        Err(LxmfAdmissionFailure::Busy) => MeasureLxmfTextOutcome::Busy,
    }
}

pub async fn announce_lxmf() -> AnnounceLxmfOutcome {
    match admit_running(supervisor(), Command::AnnounceLxmf) {
        Ok(receiver) => bounded_reply(receiver, LXMF_QUERY_TIMEOUT)
            .await
            .unwrap_or(AnnounceLxmfOutcome::Failed),
        Err(LxmfAdmissionFailure::LocalNodeStopped) => AnnounceLxmfOutcome::LocalNodeStopped,
        Err(LxmfAdmissionFailure::Busy) => AnnounceLxmfOutcome::Busy,
    }
}

pub async fn send_direct_text(input: SendDirectTextInput) -> SendDirectTextOutcome {
    send_direct_text_with_supervisor(supervisor(), input).await
}

pub(super) async fn send_direct_text_with_supervisor(
    supervisor: &Supervisor,
    input: SendDirectTextInput,
) -> SendDirectTextOutcome {
    let receiver = match admit_running(supervisor, |response| {
        Command::SendDirectText(input, response)
    }) {
        Ok(receiver) => receiver,
        Err(failure) => {
            return SendDirectTextOutcome::DevelopmentUnavailable {
                detail: match failure {
                    LxmfAdmissionFailure::Busy => "The native command lane is busy.",
                    LxmfAdmissionFailure::LocalNodeStopped => {
                        "The native Prns node is not running."
                    }
                }
                .to_owned(),
            }
        }
    };
    // No synthetic timeout after durable insertion admission. Dropping this
    // receiver does not abort the actor-owned, stop-drained commit task.
    receiver
        .await
        .unwrap_or_else(|_| SendDirectTextOutcome::DevelopmentUnavailable {
            detail: "The native response owner stopped without reporting the queue commit."
                .to_owned(),
        })
}

fn prepared_owner(
    state: &SupervisorState,
) -> Result<&DevelopmentStoreOwner, DevelopmentStoreFailure> {
    let owner = state
        .application_owner
        .as_ref()
        .ok_or_else(|| DevelopmentStoreFailure::unavailable(STORAGE_NOT_PREPARED))?;
    if state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.storage_root != owner.root)
    {
        return Err(DevelopmentStoreFailure::unavailable(
            "The active generation owns a different storage root.",
        ));
    }
    Ok(owner)
}

fn admit_directory(
    supervisor: &Supervisor,
    request: DirectoryRequest,
) -> Result<oneshot::Receiver<StoreReply>, DevelopmentStoreFailure> {
    let state = try_state(supervisor).map_err(DevelopmentStoreFailure::unavailable)?;
    prepared_owner(&state)?.admit_directory_async(request)
}

async fn mutation(supervisor: &Supervisor, request: DirectoryRequest) -> ContactMutationOutcome {
    let receiver = match admit_directory(supervisor, request) {
        Ok(receiver) => receiver,
        Err(failure) => return mutation_store_failure(failure),
    };
    receive_mutation(receiver).await
}

async fn receive_mutation(receiver: oneshot::Receiver<StoreReply>) -> ContactMutationOutcome {
    match receiver.await {
        Ok(Ok(DirectoryResponse::Mutation(outcome))) => outcome,
        Ok(Err(failure)) => mutation_store_failure(failure),
        _ => ContactMutationOutcome::DevelopmentUnavailable {
            detail: "The database owner did not report a definitive contact mutation.".to_owned(),
        },
    }
}

pub async fn create_manual_contact(input: CreateManualContactInput) -> ContactMutationOutcome {
    mutation(
        supervisor(),
        DirectoryRequest::CreateManual {
            destination: input.destination,
            identity: input.identity,
            alias: input.alias,
        },
    )
    .await
}
pub async fn set_contact_alias(input: SetContactAliasInput) -> ContactMutationOutcome {
    mutation(
        supervisor(),
        DirectoryRequest::SetAlias {
            destination: input.destination,
            alias: input.alias,
        },
    )
    .await
}
pub async fn set_contact_pinned(input: SetContactPinnedInput) -> ContactMutationOutcome {
    mutation(
        supervisor(),
        DirectoryRequest::SetPinned {
            destination: input.destination,
            pinned: input.pinned,
        },
    )
    .await
}
pub async fn delete_contact(input: ContactDestinationInput) -> ContactMutationOutcome {
    mutation(
        supervisor(),
        DirectoryRequest::Delete {
            destination: input.destination,
        },
    )
    .await
}
pub async fn get_contact(input: ContactDestinationInput) -> ContactLookupOutcome {
    let receiver = match admit_directory(
        supervisor(),
        DirectoryRequest::Get {
            destination: input.destination,
        },
    ) {
        Ok(receiver) => receiver,
        Err(failure) => return lookup_store_failure(failure),
    };
    match bounded_reply(receiver, DIRECTORY_TIMEOUT).await {
        Ok(Ok(DirectoryResponse::Lookup(outcome))) => outcome,
        Ok(Err(failure)) => lookup_store_failure(failure),
        _ => ContactLookupOutcome::DevelopmentUnavailable {
            detail: "The database owner did not return the contact lookup.".to_owned(),
        },
    }
}
pub async fn list_contacts() -> ContactListOutcome {
    list_contacts_with_supervisor(supervisor()).await
}
pub(super) async fn list_contacts_with_supervisor(supervisor: &Supervisor) -> ContactListOutcome {
    let receiver = match admit_directory(supervisor, DirectoryRequest::List) {
        Ok(receiver) => receiver,
        Err(failure) => return list_store_failure(failure),
    };
    match bounded_reply(receiver, DIRECTORY_TIMEOUT).await {
        Ok(Ok(DirectoryResponse::List(outcome))) => outcome,
        Ok(Err(failure)) => list_store_failure(failure),
        _ => ContactListOutcome::DevelopmentUnavailable {
            detail: "The database owner did not return the contact list.".to_owned(),
        },
    }
}

pub async fn save_observed_destination(input: ContactDestinationInput) -> ContactMutationOutcome {
    save_observed_destination_with_supervisor(supervisor(), input).await
}

pub(super) async fn save_observed_destination_with_supervisor(
    supervisor: &Supervisor,
    input: ContactDestinationInput,
) -> ContactMutationOutcome {
    let admitted = (|| {
        let state = try_state(supervisor).map_err(|detail| {
            ContactMutationOutcome::DevelopmentUnavailable {
                detail: detail.to_owned(),
            }
        })?;
        let worker = running_worker(supervisor, &state)
            .map_err(|_| ContactMutationOutcome::LocalNodeStopped)?;
        prepared_owner(&state).map_err(mutation_store_failure)?;
        let (response, receiver) = oneshot::channel();
        worker
            .commands
            .try_send(Command::ObservedIdentity(input.destination, response))
            .map_err(|_| ContactMutationOutcome::DevelopmentUnavailable {
                detail: "The local observation lane is full or closed.".to_owned(),
            })?;
        Ok::<_, ContactMutationOutcome>((receiver, worker.commands.clone()))
    })();
    let (receiver, generation_commands) = match admitted {
        Ok(admitted) => admitted,
        Err(outcome) => return outcome,
    };
    let identity =
        match bounded_reply(receiver, HOST_INSPECTION_TIMEOUT + Duration::from_secs(1)).await {
            Ok(Ok(Some(identity))) => identity,
            Ok(Ok(None)) => return ContactMutationOutcome::NotObserved,
            Ok(Err(detail)) => return ContactMutationOutcome::DevelopmentUnavailable { detail },
            Err(detail) => {
                return ContactMutationOutcome::DevelopmentUnavailable {
                    detail: detail.to_owned(),
                }
            }
        };
    let admitted = (|| {
        let state = try_state(supervisor).map_err(DevelopmentStoreFailure::unavailable)?;
        let worker =
            running_worker(supervisor, &state).map_err(DevelopmentStoreFailure::unavailable)?;
        if !worker.commands.same_channel(&generation_commands) {
            return Err(DevelopmentStoreFailure::unavailable(
                "The observed association belongs to a generation that has stopped.",
            ));
        }
        prepared_owner(&state)?.admit_directory_async(DirectoryRequest::SaveObserved {
            destination: input.destination,
            identity,
        })
    })();
    match admitted {
        Ok(receiver) => receive_mutation(receiver).await,
        Err(failure) => mutation_store_failure(failure),
    }
}

enum MailboxResponse<T> {
    Offline(oneshot::Receiver<MailboxStoreReply>),
    Running(oneshot::Receiver<T>),
}

fn admit_mailbox<T>(
    supervisor: &Supervisor,
    request: MailboxRequest,
    command: impl FnOnce(Reply<T>) -> Command,
) -> Result<MailboxResponse<T>, MailboxFailure> {
    let state =
        try_state(supervisor).map_err(|detail| MailboxFailure::Unavailable(detail.to_owned()))?;
    let owner = prepared_owner(&state).map_err(|failure| match failure {
        DevelopmentStoreFailure::Unavailable(detail) => MailboxFailure::Unavailable(detail),
        DevelopmentStoreFailure::ResetRequired(reason) => MailboxFailure::ResetRequired(reason),
    })?;
    // A terminal thread owns no live network work. Its native lifecycle owner
    // still retains the JoinHandle for cleanup; a foreign read never joins it.
    // Failed-stop authority is deliberately excluded from this offline path.
    let active_worker = state.worker.as_ref().is_some_and(|worker| {
        worker.incomplete_stop.is_some()
            || !worker
                .join
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
    });
    match durable_mailbox_access(supervisor.snapshots.read().runtime, active_worker) {
        DurableMailboxAccess::Offline => owner
            .admit_mailbox_async(request)
            .map(MailboxResponse::Offline),
        DurableMailboxAccess::GenerationTransition => {
            Err(MailboxFailure::Unavailable(TRANSITION.to_owned()))
        }
        DurableMailboxAccess::RunningGeneration => {
            let worker = running_worker(supervisor, &state)
                .map_err(|detail| MailboxFailure::Unavailable(detail.to_owned()))?;
            let (response, receiver) = oneshot::channel();
            worker
                .commands
                .try_send(command(response))
                .map_err(|_| MailboxFailure::Busy)?;
            Ok(MailboxResponse::Running(receiver))
        }
    }
}

pub async fn list_lxmf_messages(input: ListLxmfMessagesInput) -> LxmfMessageListOutcome {
    list_lxmf_messages_with_supervisor(supervisor(), input).await
}

pub(super) async fn list_lxmf_messages_with_supervisor(
    supervisor: &Supervisor,
    input: ListLxmfMessagesInput,
) -> LxmfMessageListOutcome {
    let request = match crate::lxmf::mailbox_list_request(input) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    match admit_mailbox(supervisor, MailboxRequest::List(request), |response| {
        Command::ListLxmfMessages(request, response)
    }) {
        Ok(MailboxResponse::Running(receiver)) => bounded_reply(receiver, LXMF_QUERY_TIMEOUT)
            .await
            .unwrap_or_else(|detail| LxmfMessageListOutcome::DevelopmentUnavailable {
                detail: detail.to_owned(),
            }),
        Ok(MailboxResponse::Offline(receiver)) => match bounded_reply(receiver, LXMF_QUERY_TIMEOUT)
            .await
        {
            Ok(Ok(MailboxReply::Listed { messages, .. })) => {
                crate::lxmf::project_messages(&messages)
            }
            Ok(Err(failure)) => crate::lxmf::message_list_failure(failure),
            _ => LxmfMessageListOutcome::DevelopmentUnavailable {
                detail: "The database owner did not return the durable mailbox query.".to_owned(),
            },
        },
        Err(failure) => crate::lxmf::message_list_failure(failure),
    }
}

pub async fn retry_lxmf_message(input: RetryLxmfMessageInput) -> RetryLxmfMessageOutcome {
    retry_lxmf_message_with_supervisor(supervisor(), input).await
}

pub(super) async fn retry_lxmf_message_with_supervisor(
    supervisor: &Supervisor,
    input: RetryLxmfMessageInput,
) -> RetryLxmfMessageOutcome {
    let id = input.local_record_id;
    match admit_mailbox(
        supervisor,
        MailboxRequest::Retry {
            local_record_id: id,
        },
        |response| Command::RetryLxmfMessage(id, response),
    ) {
        Ok(MailboxResponse::Running(receiver)) => {
            receiver
                .await
                .unwrap_or_else(|_| RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail:
                        "The native response owner stopped without reporting the durable retry."
                            .to_owned(),
                })
        }
        Ok(MailboxResponse::Offline(receiver)) => match receiver.await {
            Ok(Ok(MailboxReply::Retry(transition))) => project_offline_retry_transition(transition),
            Ok(Err(failure)) => crate::lxmf::retry_failure(failure),
            _ => RetryLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The database owner stopped without reporting the durable retry."
                    .to_owned(),
            },
        },
        Err(failure) => crate::lxmf::retry_failure(failure),
    }
}

pub async fn cancel_lxmf_message(input: CancelLxmfMessageInput) -> CancelLxmfMessageOutcome {
    cancel_lxmf_message_with_supervisor(supervisor(), input).await
}

pub(super) async fn cancel_lxmf_message_with_supervisor(
    supervisor: &Supervisor,
    input: CancelLxmfMessageInput,
) -> CancelLxmfMessageOutcome {
    let id = input.local_record_id;
    let cancelled_at_millis = wall_clock_millis();
    match admit_mailbox(
        supervisor,
        MailboxRequest::Cancel {
            local_record_id: id,
            cancelled_at_millis,
        },
        |response| Command::CancelLxmfMessage(id, cancelled_at_millis, response),
    ) {
        Ok(MailboxResponse::Running(receiver)) => {
            receiver
                .await
                .unwrap_or_else(|_| {
                    CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail:
                    "The native response owner stopped without reporting the durable cancellation."
                        .to_owned(),
            }
                })
        }
        Ok(MailboxResponse::Offline(receiver)) => match receiver.await {
            Ok(Ok(MailboxReply::Cancel(transition))) => {
                project_offline_cancel_transition(transition)
            }
            Ok(Err(failure)) => crate::lxmf::cancel_failure(failure),
            _ => CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The database owner stopped without reporting the durable cancellation."
                    .to_owned(),
            },
        },
        Err(failure) => crate::lxmf::cancel_failure(failure),
    }
}

#[cfg(test)]
mod tests;
