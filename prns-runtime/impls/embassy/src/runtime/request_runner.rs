use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use heapless::Vec as HeaplessVec;

use crate::engine::{CommandId, InstantMillis, Journaled, RespondData};
use crate::identity::IdentityHash;
use crate::remote_control::{
    RemoteControlAuthorizeControllerOutcome, RemoteControlControllerGrantTable,
    RemoteControlResponse, RemoteControlRevokeControllerOutcome,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::request::RESPONSE_WIRE_OVERHEAD;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_runtime::runtime::placement::{
    admit_verified_remote_control_request, dispatch_verified_admitted_remote_control_request,
    VerifiedAdmittedRemoteControlRequest,
};

use super::embedded_persistence::RemoteControlAuthorizationSnapshotKind;
use super::node_facade::PrnsNodeHandle;
use super::remote_control_controller_grants::{
    RemoteControlControllerGrantCommand, RemoteControlControllerGrantCompletion,
};
use super::remote_control_pairing_authorizations::{
    prepare_controller_grant, prepare_controller_revocation, snapshot_controller_grants,
    AppliedRemoteControlControllerGrantActivation, PendingRemoteControlControllerGrantActivation,
    PreparedRemoteControlControllerGrantTransaction,
    RemoteControlPairingAuthorizationTransactionFailure,
    RemoteControlPairingAuthorizationTransactionState,
};
use super::remote_control_pairing_persistence::{
    RemoteControlAuthorizationStoreExchange, RemoteControlAuthorizationStoreRequirement,
    RemoteControlPairingPersistenceEvents, RemoteControlPairingPersistenceProgress,
    RemoteControlPairingPersistenceRequired,
};
use super::remote_control_target_accesses::{
    RemoteControlTargetAccessCommand, RemoteControlTargetAccessCompletion,
};
use super::request_endpoints::{
    dispatch_request, Decline, InboundRequest, RequestEndpointPolicy, RequestEndpointSet,
    RespondToken, ResponseCapacityExceeded, ResponseSink,
};
use super::AssembledRemoteControl;

#[allow(clippy::large_enum_variant)]
enum RunnerResponse<const N: usize> {
    Buffered(RespondData),
    Resource(HeaplessVec<u8, N>),
    StaticBytes(&'static [u8]),
    #[cfg(feature = "large-static-responses")]
    StaticFile {
        name: &'static str,
        bytes: &'static [u8],
    },
}

enum PreparedRunnerRequest {
    RemoteControl(VerifiedAdmittedRemoteControlRequest),
    Application,
    Declined(Decline),
}

impl<const N: usize> ResponseSink for RunnerResponse<N> {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => body
                .extend_from_slice(bytes)
                .map_err(|()| ResponseCapacityExceeded),
            RunnerResponse::Resource(_) => Err(ResponseCapacityExceeded),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_resource(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                let mut resource = HeaplessVec::new();
                resource
                    .resize(RESPONSE_WIRE_OVERHEAD, 0)
                    .map_err(|()| ResponseCapacityExceeded)?;
                if bytes.is_empty() {
                    resource.push(0xc0).map_err(|_| ResponseCapacityExceeded)?;
                } else {
                    resource
                        .extend_from_slice(bytes)
                        .map_err(|()| ResponseCapacityExceeded)?;
                }
                *self = RunnerResponse::Resource(resource);
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => ResponseSink::put_bytes(body, bytes),
            RunnerResponse::Resource(_) => Err(ResponseCapacityExceeded),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_static_bytes(&mut self, bytes: &'static [u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticBytes(bytes);
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    #[cfg(feature = "large-static-responses")]
    fn put_static_file(
        &mut self,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticFile { name, bytes };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }
}

pub(super) struct RunnerRequest<const N: usize> {
    destination: DestinationHash,
    link_id: LinkId,
    request_id: RequestId,
    requester: Option<IdentityHash>,
    path_hash: RequestPathHash,
    requested_at: InstantMillis,
    rtt: RttMillis,
    data: HeaplessVec<u8, N>,
}

impl<const N: usize> RunnerRequest<N> {
    pub(super) fn copy_from(journaled: &Journaled<'_>) -> Option<Self> {
        let Journaled::RequestReceived {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            requested_at,
            rtt,
            data,
        } = journaled
        else {
            return None;
        };
        Some(Self {
            destination: *destination,
            link_id: *link_id,
            request_id: *request_id,
            requester: *requester,
            path_hash: *path_hash,
            requested_at: *requested_at,
            rtt: *rtt,
            data: HeaplessVec::from_slice(data).ok()?,
        })
    }

    /// Retain only the already-admitted context needed to route a response.
    ///
    /// Remote Control admission consumes and verifies the request body before
    /// durable authorization work begins. Keeping its full bounded capacity
    /// alive across that flash transaction would charge every embedded router
    /// task for bytes that can no longer affect the admitted operation.
    fn without_data(self) -> RunnerRequest<0> {
        RunnerRequest {
            destination: self.destination,
            link_id: self.link_id,
            request_id: self.request_id,
            requester: self.requester,
            path_hash: self.path_hash,
            requested_at: self.requested_at,
            rtt: self.rtt,
            data: HeaplessVec::new(),
        }
    }

    fn respond_token(&self) -> RespondToken {
        RespondToken {
            link_id: self.link_id,
            request_id: self.request_id,
            rtt: self.rtt,
        }
    }

    #[allow(dead_code)]
    pub(super) fn try_enqueue<M, const CAP: usize>(
        journaled: &Journaled<'_>,
        sender: &Sender<'_, M, Self, CAP>,
    ) where
        M: RawMutex,
    {
        let Journaled::RequestReceived { data, .. } = journaled else {
            return;
        };
        #[cfg_attr(not(feature = "log"), allow(unused_variables))]
        let inbound_len = data.len();
        let Some(request) = Self::copy_from(journaled) else {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop copy_failed bytes={inbound_len}"
            );
            return;
        };
        if sender.try_send(request).is_err() {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop queue_full bytes={inbound_len}"
            );
        } else {
            #[cfg(feature = "log")]
            if let Journaled::RequestReceived {
                destination,
                link_id,
                requester,
                path_hash,
                data,
                ..
            } = journaled
            {
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: enqueue dest={} link={} path={} ctrl={} kind={:?} bytes={}",
                    hex4(destination.as_bytes()),
                    hex4(link_id.as_bytes()),
                    hex4(path_hash.as_bytes()),
                    hash4(*requester),
                    data.get(1).copied(),
                    data.len()
                );
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn trace_journaled(journaled: &Journaled<'_>) {
    #[cfg(feature = "log")]
    match journaled {
        Journaled::LinkEstablished(established) => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link up id={} rtt={}",
                hex4(established.link_id.as_bytes()),
                established.rtt_millis
            );
        }
        Journaled::PeerIdentified { link_id, identity } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: identified link={} peer={}",
                hex4(link_id.as_bytes()),
                hex4(identity.as_bytes())
            );
        }
        Journaled::RequestReceived {
            destination,
            link_id,
            requester,
            path_hash,
            data,
            ..
        } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: request dest={} link={} path={} ctrl={} kind={:?} bytes={}",
                hex4(destination.as_bytes()),
                hex4(link_id.as_bytes()),
                hex4(path_hash.as_bytes()),
                hash4(*requester),
                data.get(1).copied(),
                data.len()
            );
        }
        Journaled::LinkClosed { link_id, reason } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link close id={} reason={reason:?}",
                hex4(link_id.as_bytes())
            );
        }
        Journaled::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link mismatch id={} attached={} arrived={}",
                hex4(link_id.as_bytes()),
                hex4(attached_interface.as_bytes()),
                hex4(arrived_on.as_bytes())
            );
        }
        _ => {}
    }
    #[cfg(not(feature = "log"))]
    let _ = journaled;
}

#[inline(never)]
fn submit_controller_grant_rollback<M: RawMutex>(
    remote_control: &AssembledRemoteControl,
    stores: &RemoteControlAuthorizationStoreExchange<M>,
) -> bool {
    let Ok(rollback) = snapshot_controller_grants(remote_control) else {
        return false;
    };
    stores.submit(
        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
        rollback,
        RemoteControlAuthorizationStoreRequirement::Rollback,
    );
    true
}

struct PendingControllerGrantPersistence {
    activation: PendingRemoteControlControllerGrantActivation,
    settlement: ControllerGrantSettlement,
}

#[derive(Clone, Copy)]
enum ControllerGrantOperation {
    Set,
    Revoke,
}

impl ControllerGrantOperation {
    fn unavailable(self) -> RemoteControlControllerGrantCompletion {
        match self {
            Self::Set => RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                super::SetRemoteControlControllerGrantServiceError::Unavailable,
            )),
            Self::Revoke => RemoteControlControllerGrantCompletion::ControllerRevoked(Err(
                super::RevokeRemoteControlControllerServiceError::Unavailable,
            )),
        }
    }

    fn success(
        self,
        activation: &AppliedRemoteControlControllerGrantActivation,
    ) -> Option<RemoteControlControllerGrantCompletion> {
        match self {
            Self::Set => activation.set_outcome().map(|outcome| {
                RemoteControlControllerGrantCompletion::ControllerGrantSet(Ok(outcome))
            }),
            Self::Revoke => activation.revoke_outcome().map(|outcome| {
                RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(outcome))
            }),
        }
    }

    fn prepared_success(
        self,
        prepared: &PreparedRemoteControlControllerGrantTransaction,
    ) -> Option<RemoteControlControllerGrantCompletion> {
        match self {
            Self::Set => prepared.set_outcome().map(|outcome| {
                RemoteControlControllerGrantCompletion::ControllerGrantSet(Ok(outcome))
            }),
            Self::Revoke => prepared.revoke_outcome().map(|outcome| {
                RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(outcome))
            }),
        }
    }
}

struct PendingVerifiedControllerGrant {
    responder: RespondToken,
    completion: VerifiedControllerGrantCompletion,
}

struct ReadyVerifiedControllerGrant {
    responder: RespondToken,
    completion: VerifiedControllerGrantCompletion,
}

enum VerifiedControllerGrantStart {
    Pending,
    Dispatch,
    Respond(ReadyVerifiedControllerGrant),
}

enum ControllerGrantSettlement {
    Local {
        id: CommandId,
        operation: ControllerGrantOperation,
    },
    Remote(PendingVerifiedControllerGrant),
}

enum ControllerGrantPersistenceState {
    Ready,
    Unrecoverable,
    WaitingInitialStore(PendingControllerGrantPersistence),
    WaitingRollbackStore {
        settlement: Option<ControllerGrantSettlement>,
    },
}

pub(super) struct ControllerGrantPersistenceProgress {
    state: ControllerGrantPersistenceState,
}

impl ControllerGrantPersistenceProgress {
    pub(super) const fn new() -> Self {
        Self {
            state: ControllerGrantPersistenceState::Ready,
        }
    }

    const fn is_ready(&self) -> bool {
        matches!(self.state, ControllerGrantPersistenceState::Ready)
    }

    const fn is_waiting_for_store(&self) -> bool {
        matches!(
            self.state,
            ControllerGrantPersistenceState::WaitingInitialStore(_)
                | ControllerGrantPersistenceState::WaitingRollbackStore { .. }
        )
    }
}

// Pairing carries signed transcript state while controller-grant persistence carries a much smaller
// prepared mutation. They are mutually exclusive by authorization admission, so one bounded enum
// avoids permanently reserving both state machines on RAM-constrained firmware.
#[allow(clippy::large_enum_variant)]
enum RemoteControlAuthorizationPersistenceProgress {
    Pairing(RemoteControlPairingPersistenceProgress),
    ControllerGrant(ControllerGrantPersistenceProgress),
}

impl RemoteControlAuthorizationPersistenceProgress {
    const fn new() -> Self {
        Self::Pairing(RemoteControlPairingPersistenceProgress::new())
    }

    fn is_ready(&self) -> bool {
        matches!(self, Self::Pairing(progress) if progress.is_ready())
    }
}

#[inline(never)]
fn begin_controller_grant_change<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    remote_control: &mut AssembledRemoteControl,
    authorization_transaction: &RemoteControlPairingAuthorizationTransactionState,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    command: RemoteControlControllerGrantCommand,
) -> Option<PendingControllerGrantPersistence>
where
    M: RawMutex + Sync,
{
    let (id, prepared, operation) = match command {
        RemoteControlControllerGrantCommand::SetControllerGrant { id, grant } => {
            if authorization_transaction.is_active() {
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                        super::SetRemoteControlControllerGrantServiceError::TransactionInProgress,
                    )),
                );
                return None;
            }
            let prepared = match prepare_controller_grant(remote_control, grant) {
                Ok(prepared) => prepared,
                Err(RemoteControlPairingAuthorizationTransactionFailure::CapacityExhausted) => {
                    let _settled = commands.settle_remote_control_controller_grant(
                        id,
                        RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                            super::SetRemoteControlControllerGrantServiceError::CapacityExhausted,
                        )),
                    );
                    return None;
                }
                Err(_) => {
                    let _settled = commands.settle_remote_control_controller_grant(
                        id,
                        RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                            super::SetRemoteControlControllerGrantServiceError::Unavailable,
                        )),
                    );
                    return None;
                }
            };
            if prepared.set_outcome().is_none() {
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                        super::SetRemoteControlControllerGrantServiceError::Unavailable,
                    )),
                );
                return None;
            }
            (id, prepared, ControllerGrantOperation::Set)
        }
        RemoteControlControllerGrantCommand::RevokeController { id, controller } => {
            if authorization_transaction.is_active() {
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerRevoked(Err(
                        super::RevokeRemoteControlControllerServiceError::TransactionInProgress,
                    )),
                );
                return None;
            }
            let prepared = match prepare_controller_revocation(remote_control, controller) {
                Ok(prepared) => prepared,
                Err(_) => {
                    let _settled = commands.settle_remote_control_controller_grant(
                        id,
                        RemoteControlControllerGrantCompletion::ControllerRevoked(Err(
                            super::RevokeRemoteControlControllerServiceError::Unavailable,
                        )),
                    );
                    return None;
                }
            };
            if prepared.revoke_outcome().is_none() {
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerRevoked(Err(
                        super::RevokeRemoteControlControllerServiceError::Unavailable,
                    )),
                );
                return None;
            }
            (id, prepared, ControllerGrantOperation::Revoke)
        }
    };

    if prepared.is_unchanged() {
        let Some(success) = operation.prepared_success(&prepared) else {
            let _settled =
                commands.settle_remote_control_controller_grant(id, operation.unavailable());
            return None;
        };
        let _settled = commands.settle_remote_control_controller_grant(id, success);
        return None;
    }
    let Some(stores) = stores else {
        let _settled = commands.settle_remote_control_controller_grant(id, operation.unavailable());
        return None;
    };
    let Ok((activation, projected)) = prepared.into_projection() else {
        let _settled = commands.settle_remote_control_controller_grant(id, operation.unavailable());
        return None;
    };
    stores.submit(
        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
        projected,
        RemoteControlAuthorizationStoreRequirement::Initial,
    );
    Some(PendingControllerGrantPersistence {
        activation,
        settlement: ControllerGrantSettlement::Local { id, operation },
    })
}

#[inline(never)]
fn begin_controller_grant_persistence<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    progress: &mut ControllerGrantPersistenceProgress,
    remote_control: &mut AssembledRemoteControl,
    authorization_transaction: &RemoteControlPairingAuthorizationTransactionState,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    command: RemoteControlControllerGrantCommand,
) where
    M: RawMutex + Sync,
{
    if !progress.is_ready() {
        settle_controller_grant_busy(commands, command);
        return;
    }
    let Some(pending) = begin_controller_grant_change(
        remote_control,
        authorization_transaction,
        stores,
        commands,
        command,
    ) else {
        return;
    };
    progress.state = ControllerGrantPersistenceState::WaitingInitialStore(pending);
}

fn settle_controller_grant_busy<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    command: RemoteControlControllerGrantCommand,
) where
    M: RawMutex + Sync,
{
    let (id, completion) = match command {
        RemoteControlControllerGrantCommand::SetControllerGrant { id, .. } => (
            id,
            RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                super::SetRemoteControlControllerGrantServiceError::TransactionInProgress,
            )),
        ),
        RemoteControlControllerGrantCommand::RevokeController { id, .. } => (
            id,
            RemoteControlControllerGrantCompletion::ControllerRevoked(Err(
                super::RevokeRemoteControlControllerServiceError::TransactionInProgress,
            )),
        ),
    };
    let _settled = commands.settle_remote_control_controller_grant(id, completion);
}

#[inline(never)]
fn progress_controller_grant_persistence<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    progress: &mut ControllerGrantPersistenceProgress,
    remote_control: &mut AssembledRemoteControl,
    stores: &RemoteControlAuthorizationStoreExchange<M>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    stored: Result<(), super::embedded_persistence::EmbeddedPersistenceFailure>,
) -> Option<ReadyVerifiedControllerGrant>
where
    M: RawMutex + Sync,
{
    let state = core::mem::replace(&mut progress.state, ControllerGrantPersistenceState::Ready);
    let ControllerGrantPersistenceState::WaitingInitialStore(pending) = state else {
        let ControllerGrantPersistenceState::WaitingRollbackStore { settlement } = state else {
            return None;
        };
        if stored.is_err() {
            if submit_controller_grant_rollback(remote_control, stores) {
                progress.state =
                    ControllerGrantPersistenceState::WaitingRollbackStore { settlement };
                return None;
            }
            progress.state = ControllerGrantPersistenceState::Unrecoverable;
            return settlement
                .and_then(|settlement| fail_controller_grant_settlement(commands, settlement));
        }
        return settlement.and_then(|settlement| match settlement {
            ControllerGrantSettlement::Local { id, operation } => {
                let _settled =
                    commands.settle_remote_control_controller_grant(id, operation.unavailable());
                None
            }
            ControllerGrantSettlement::Remote(pending) => {
                Some(complete_pending_verified_controller_grant(
                    pending,
                    VerifiedControllerGrantPersistenceOutcome::Rejected,
                ))
            }
        });
    };

    let PendingControllerGrantPersistence {
        activation,
        settlement,
    } = pending;
    if stored.is_err() {
        if !submit_controller_grant_rollback(remote_control, stores) {
            progress.state = ControllerGrantPersistenceState::Unrecoverable;
            return fail_controller_grant_settlement(commands, settlement);
        }
        progress.state = ControllerGrantPersistenceState::WaitingRollbackStore {
            settlement: Some(settlement),
        };
        return None;
    }
    let applied = match activation.activate(remote_control) {
        Ok(applied) => applied,
        Err(_) => {
            if !submit_controller_grant_rollback(remote_control, stores) {
                progress.state = ControllerGrantPersistenceState::Unrecoverable;
                return fail_controller_grant_settlement(commands, settlement);
            }
            progress.state = ControllerGrantPersistenceState::WaitingRollbackStore {
                settlement: Some(settlement),
            };
            return None;
        }
    };
    match settlement {
        ControllerGrantSettlement::Local { id, operation } => {
            let Some(success) = operation.success(&applied) else {
                if applied.roll_back(remote_control).is_err() {
                    progress.state = ControllerGrantPersistenceState::Unrecoverable;
                    return fail_controller_grant_settlement(
                        commands,
                        ControllerGrantSettlement::Local { id, operation },
                    );
                }
                if !submit_controller_grant_rollback(remote_control, stores) {
                    progress.state = ControllerGrantPersistenceState::Unrecoverable;
                    return fail_controller_grant_settlement(
                        commands,
                        ControllerGrantSettlement::Local { id, operation },
                    );
                }
                progress.state = ControllerGrantPersistenceState::WaitingRollbackStore {
                    settlement: Some(ControllerGrantSettlement::Local { id, operation }),
                };
                return None;
            };
            if !commands.settle_remote_control_controller_grant(id, success) {
                if applied.roll_back(remote_control).is_err() {
                    progress.state = ControllerGrantPersistenceState::Unrecoverable;
                    return None;
                }
                if submit_controller_grant_rollback(remote_control, stores) {
                    progress.state =
                        ControllerGrantPersistenceState::WaitingRollbackStore { settlement: None };
                } else {
                    progress.state = ControllerGrantPersistenceState::Unrecoverable;
                }
            }
            None
        }
        ControllerGrantSettlement::Remote(pending) => {
            let ready = complete_pending_verified_controller_grant(
                pending,
                VerifiedControllerGrantPersistenceOutcome::Committed,
            );
            if respond_verified_controller_grant(commands, ready) {
                return None;
            }
            if applied.roll_back(remote_control).is_err() {
                progress.state = ControllerGrantPersistenceState::Unrecoverable;
                return None;
            }
            if submit_controller_grant_rollback(remote_control, stores) {
                progress.state =
                    ControllerGrantPersistenceState::WaitingRollbackStore { settlement: None };
            } else {
                progress.state = ControllerGrantPersistenceState::Unrecoverable;
            }
            None
        }
    }
}

fn fail_controller_grant_settlement<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    settlement: ControllerGrantSettlement,
) -> Option<ReadyVerifiedControllerGrant>
where
    M: RawMutex + Sync,
{
    match settlement {
        ControllerGrantSettlement::Local { id, operation } => {
            let _settled =
                commands.settle_remote_control_controller_grant(id, operation.unavailable());
            None
        }
        ControllerGrantSettlement::Remote(pending) => {
            Some(complete_pending_verified_controller_grant(
                pending,
                VerifiedControllerGrantPersistenceOutcome::Rejected,
            ))
        }
    }
}

async fn progress_pairing_persistence<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    input: RemoteControlPairingPersistenceInput,
    pairing_persistence: &mut RemoteControlPairingPersistenceProgress,
    remote_control: &mut AssembledRemoteControl,
    authorization_transaction: &mut RemoteControlPairingAuthorizationTransactionState,
    authorization_stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    M: RawMutex + Sync,
{
    let result = match input {
        RemoteControlPairingPersistenceInput::Required(required) => {
            pairing_persistence
                .accept_required(
                    required,
                    remote_control,
                    authorization_transaction,
                    authorization_stores,
                    commands,
                )
                .await
        }
        RemoteControlPairingPersistenceInput::StoreCompleted(stored) => {
            let Some(stores) = authorization_stores else {
                return;
            };
            pairing_persistence
                .accept_store_completion(
                    stored,
                    remote_control,
                    authorization_transaction,
                    stores,
                    commands,
                )
                .await
        }
    };
    if let (Err(failure), Some(stores)) = (result, authorization_stores) {
        stores.report_failure(failure).await;
    }
}

fn settle_target_access_command<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    command: RemoteControlTargetAccessCommand,
    remote_control: &mut AssembledRemoteControl,
    authorization_ready: bool,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    M: RawMutex + Sync,
{
    match command {
        RemoteControlTargetAccessCommand::Inventory { id } => {
            let outcome = if !authorization_ready {
                Err(super::RemoteControlTargetInventoryServiceError::TransactionInProgress)
            } else {
                remote_control.target_inventory()
            };
            let _settled = commands.settle_remote_control_target_access(
                id,
                RemoteControlTargetAccessCompletion::Inventory(outcome),
            );
        }
        RemoteControlTargetAccessCommand::ResolveTarget { id, target } => {
            let outcome = if !authorization_ready {
                Err(super::ResolveRemoteControlTargetServiceError::TransactionInProgress)
            } else {
                remote_control.resolve_target(&target)
            };
            let _settled = commands.settle_remote_control_target_access(
                id,
                RemoteControlTargetAccessCompletion::Resolved(outcome),
            );
        }
        RemoteControlTargetAccessCommand::SetTargetAccess { id, access } => {
            let outcome = if !authorization_ready {
                Err(super::SetRemoteControlTargetAccessServiceError::TransactionInProgress)
            } else {
                remote_control.set_target_access(access)
            };
            let _settled = commands.settle_remote_control_target_access(
                id,
                RemoteControlTargetAccessCompletion::AccessSet(outcome),
            );
        }
        RemoteControlTargetAccessCommand::ForgetTarget { id, target } => {
            let outcome = if !authorization_ready {
                Err(super::ForgetRemoteControlTargetServiceError::TransactionInProgress)
            } else {
                remote_control.forget_target_by_hash(target)
            };
            let _settled = commands.settle_remote_control_target_access(
                id,
                RemoteControlTargetAccessCompletion::Forgotten(outcome),
            );
        }
    }
}

fn prepare_runner_request<St, R, const REQUEST_BYTES: usize>(
    remote_control: &mut AssembledRemoteControl,
    request: &RunnerRequest<REQUEST_BYTES>,
) -> PreparedRunnerRequest
where
    R: RequestEndpointSet<St>,
{
    let inbound = InboundRequest::new(
        request.destination,
        request.link_id,
        request.request_id,
        request.requester,
        request.requested_at,
        request.rtt,
        &request.data,
    );
    if let Some((controller_grants, available_requests, self_announcement)) =
        remote_control.request_configuration_mut(request.destination, request.path_hash)
    {
        match admit_verified_remote_control_request(
            controller_grants,
            available_requests,
            self_announcement,
            &inbound,
        ) {
            Ok(verified) => PreparedRunnerRequest::RemoteControl(verified),
            Err(reason) => PreparedRunnerRequest::Declined(reason.into()),
        }
    } else if R::REGISTRATIONS.iter().any(|(path, policy)| {
        RequestPathHash::of(path) == request.path_hash
            && *policy == RequestEndpointPolicy::AllowRemoteControlControllers
    }) {
        let authorized = request.requester.is_some_and(|requester| {
            remote_control
                .controller_grants()
                .is_some_and(|grants| grants.contains_controller(&requester))
        });
        match authorized {
            true => PreparedRunnerRequest::Application,
            false => PreparedRunnerRequest::Declined(Decline::Ignore),
        }
    } else {
        PreparedRunnerRequest::Application
    }
}

enum VerifiedControllerGrantCompletion {
    Authorize(RemoteControlAuthorizeControllerOutcome),
    Revoke(RemoteControlRevokeControllerOutcome),
}

enum VerifiedControllerGrantPersistenceOutcome {
    Committed,
    Rejected,
}

impl VerifiedControllerGrantCompletion {
    fn failed(self) -> Self {
        match self {
            Self::Authorize(_) => Self::Authorize(RemoteControlAuthorizeControllerOutcome::Failed),
            Self::Revoke(_) => Self::Revoke(RemoteControlRevokeControllerOutcome::Failed),
        }
    }
}

fn complete_pending_verified_controller_grant(
    pending: PendingVerifiedControllerGrant,
    persistence: VerifiedControllerGrantPersistenceOutcome,
) -> ReadyVerifiedControllerGrant {
    let completion = match persistence {
        VerifiedControllerGrantPersistenceOutcome::Committed => pending.completion,
        VerifiedControllerGrantPersistenceOutcome::Rejected => pending.completion.failed(),
    };
    ReadyVerifiedControllerGrant {
        responder: pending.responder,
        completion,
    }
}

fn ready_verified_controller_grant(
    responder: RespondToken,
    completion: VerifiedControllerGrantCompletion,
) -> ReadyVerifiedControllerGrant {
    ReadyVerifiedControllerGrant {
        responder,
        completion,
    }
}

fn respond_verified_controller_grant<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    ready: ReadyVerifiedControllerGrant,
) -> bool
where
    M: RawMutex + Sync,
{
    let response = match ready.completion {
        VerifiedControllerGrantCompletion::Authorize(outcome) => {
            RemoteControlResponse::AuthorizeController(outcome)
        }
        VerifiedControllerGrantCompletion::Revoke(outcome) => {
            RemoteControlResponse::RevokeController(outcome)
        }
    };
    let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let Ok(encoded_len) = response.write_into(&mut encoded) else {
        commands.close_link(ready.responder.link_id);
        return false;
    };
    let Ok(body) = RespondData::from_slice(&encoded[..encoded_len]) else {
        commands.close_link(ready.responder.link_id);
        return false;
    };
    commands.respond_owned_packed(ready.responder, body)
}

#[inline(never)]
fn begin_verified_controller_grant_persistence<M: RawMutex>(
    progress: &mut ControllerGrantPersistenceProgress,
    remote_control: &mut AssembledRemoteControl,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    request: &RunnerRequest<0>,
    verified: &VerifiedAdmittedRemoteControlRequest,
) -> VerifiedControllerGrantStart {
    let responder = request.respond_token();
    let (prepared, completion) = if let Some(grant) = verified.authorize_controller_grant() {
        let prepared = match prepare_controller_grant(remote_control, grant) {
            Ok(prepared) => prepared,
            Err(RemoteControlPairingAuthorizationTransactionFailure::CapacityExhausted) => {
                return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
                    responder,
                    VerifiedControllerGrantCompletion::Authorize(
                        RemoteControlAuthorizeControllerOutcome::CapacityExhausted,
                    ),
                ));
            }
            Err(_) => {
                return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
                    responder,
                    VerifiedControllerGrantCompletion::Authorize(
                        RemoteControlAuthorizeControllerOutcome::Failed,
                    ),
                ));
            }
        };
        (
            prepared,
            VerifiedControllerGrantCompletion::Authorize(
                RemoteControlAuthorizeControllerOutcome::Applied,
            ),
        )
    } else if let Some(controller) = verified.revoke_controller_grant() {
        let prepared = match prepare_controller_revocation(remote_control, controller) {
            Ok(prepared) => prepared,
            Err(_) => {
                return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
                    responder,
                    VerifiedControllerGrantCompletion::Revoke(
                        RemoteControlRevokeControllerOutcome::Failed,
                    ),
                ));
            }
        };
        let completion = if prepared.is_unchanged() {
            RemoteControlRevokeControllerOutcome::NotFound
        } else {
            RemoteControlRevokeControllerOutcome::Applied
        };
        (
            prepared,
            VerifiedControllerGrantCompletion::Revoke(completion),
        )
    } else {
        return VerifiedControllerGrantStart::Dispatch;
    };

    if prepared.is_unchanged() {
        return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
            responder, completion,
        ));
    }
    let Some(stores) = stores else {
        return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
            responder,
            completion.failed(),
        ));
    };
    let Ok((activation, projected)) = prepared.into_projection() else {
        return VerifiedControllerGrantStart::Respond(ready_verified_controller_grant(
            responder,
            completion.failed(),
        ));
    };
    stores.submit(
        RemoteControlAuthorizationSnapshotKind::ControllerGrants,
        projected,
        RemoteControlAuthorizationStoreRequirement::Initial,
    );
    progress.state =
        ControllerGrantPersistenceState::WaitingInitialStore(PendingControllerGrantPersistence {
            activation,
            settlement: ControllerGrantSettlement::Remote(PendingVerifiedControllerGrant {
                responder,
                completion,
            }),
        });
    VerifiedControllerGrantStart::Pending
}

pub(super) async fn run_router<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUESTS: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    remote_control: &mut AssembledRemoteControl,
    requests: Receiver<'_, M, RunnerRequest<REQUEST_BYTES>, REQUESTS>,
    pairing_events: &RemoteControlPairingPersistenceEvents<M>,
    authorization_stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex + Sync,
    St: prns_runtime::runtime::RemoteControlHostControls,
{
    let mut authorization_transaction = RemoteControlPairingAuthorizationTransactionState::new();
    let mut authorization_persistence = RemoteControlAuthorizationPersistenceProgress::new();
    #[cfg(feature = "log")]
    log::info!(target: "personal_hopspot_esp32", "rc: runner up");
    loop {
        match select4(
            next_authorization_persistence_input(
                &authorization_persistence,
                pairing_events,
                authorization_stores,
            ),
            commands.next_remote_control_controller_grant_command(),
            commands.next_remote_control_target_access_command(),
            next_request_when_authorizations_ready(&authorization_persistence, requests),
        )
        .await
        {
            Either4::First(input) => match input {
                RemoteControlAuthorizationPersistenceInput::Pairing(input) => {
                    let RemoteControlAuthorizationPersistenceProgress::Pairing(pairing_persistence) =
                        &mut authorization_persistence
                    else {
                        continue;
                    };
                    progress_pairing_persistence(
                        input,
                        pairing_persistence,
                        remote_control,
                        &mut authorization_transaction,
                        authorization_stores,
                        commands,
                    )
                    .await;
                }
                RemoteControlAuthorizationPersistenceInput::ControllerGrantStoreCompleted(
                    stored,
                ) => {
                    let Some(stores) = authorization_stores else {
                        continue;
                    };
                    let (ready, finished) = {
                        let RemoteControlAuthorizationPersistenceProgress::ControllerGrant(
                            controller_grant_persistence,
                        ) = &mut authorization_persistence
                        else {
                            continue;
                        };
                        let ready = progress_controller_grant_persistence(
                            controller_grant_persistence,
                            remote_control,
                            stores,
                            commands,
                            stored,
                        );
                        (ready, controller_grant_persistence.is_ready())
                    };
                    if finished {
                        authorization_persistence =
                            RemoteControlAuthorizationPersistenceProgress::Pairing(
                                RemoteControlPairingPersistenceProgress::new(),
                            );
                    }
                    if let Some(ready) = ready {
                        let _responded = respond_verified_controller_grant(commands, ready);
                    }
                }
            },
            Either4::Second(command) => {
                if authorization_persistence.is_ready() {
                    let mut controller_grant_persistence =
                        ControllerGrantPersistenceProgress::new();
                    begin_controller_grant_persistence(
                        &mut controller_grant_persistence,
                        remote_control,
                        &authorization_transaction,
                        authorization_stores,
                        commands,
                        command,
                    );
                    if !controller_grant_persistence.is_ready() {
                        authorization_persistence =
                            RemoteControlAuthorizationPersistenceProgress::ControllerGrant(
                                controller_grant_persistence,
                            );
                    }
                } else {
                    settle_controller_grant_busy(commands, command);
                }
            }
            Either4::Third(command) => settle_target_access_command(
                command,
                remote_control,
                authorization_persistence.is_ready() && !authorization_transaction.is_active(),
                commands,
            ),
            Either4::Fourth(request) => {
                #[cfg(feature = "log")]
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: dequeue kind={:?} bytes={} link={}",
                    request.data.get(1).copied(),
                    request.data.len(),
                    hex4(request.link_id.as_bytes())
                );
                let prepared = match prepare_runner_request::<St, R, REQUEST_BYTES>(
                    remote_control,
                    &request,
                ) {
                    PreparedRunnerRequest::RemoteControl(verified) => {
                        let request = request.without_data();
                        let mut controller_grant_persistence =
                            ControllerGrantPersistenceProgress::new();
                        match begin_verified_controller_grant_persistence(
                            &mut controller_grant_persistence,
                            remote_control,
                            authorization_stores,
                            &request,
                            &verified,
                        ) {
                            VerifiedControllerGrantStart::Pending => {
                                authorization_persistence =
                                    RemoteControlAuthorizationPersistenceProgress::ControllerGrant(
                                        controller_grant_persistence,
                                    );
                            }
                            VerifiedControllerGrantStart::Respond(ready) => {
                                let _responded = respond_verified_controller_grant(commands, ready);
                            }
                            VerifiedControllerGrantStart::Dispatch => {
                                dispatch_prepared::<
                                    St,
                                    R,
                                    M,
                                    COMMANDS,
                                    COMPLETIONS,
                                    REQUEST_COMPLETIONS,
                                    RESPONSE_BYTES,
                                    0,
                                >(
                                    state,
                                    commands,
                                    request,
                                    PreparedRunnerRequest::RemoteControl(verified),
                                )
                                .await;
                            }
                        }
                        continue;
                    }
                    prepared => prepared,
                };
                dispatch_prepared::<
                    St,
                    R,
                    M,
                    COMMANDS,
                    COMPLETIONS,
                    REQUEST_COMPLETIONS,
                    RESPONSE_BYTES,
                    REQUEST_BYTES,
                >(state, commands, request, prepared)
                .await;
            }
        }
    }
}

#[inline(never)]
async fn next_request_when_authorizations_ready<
    M: RawMutex,
    const REQUESTS: usize,
    const REQUEST_BYTES: usize,
>(
    authorization: &RemoteControlAuthorizationPersistenceProgress,
    requests: Receiver<'_, M, RunnerRequest<REQUEST_BYTES>, REQUESTS>,
) -> RunnerRequest<REQUEST_BYTES> {
    if authorization.is_ready() {
        return requests.receive().await;
    }
    core::future::pending().await
}

enum RemoteControlPairingPersistenceInput {
    Required(RemoteControlPairingPersistenceRequired),
    StoreCompleted(Result<(), super::embedded_persistence::EmbeddedPersistenceFailure>),
}

enum RemoteControlAuthorizationPersistenceInput {
    Pairing(RemoteControlPairingPersistenceInput),
    ControllerGrantStoreCompleted(
        Result<(), super::embedded_persistence::EmbeddedPersistenceFailure>,
    ),
}

#[inline(never)]
async fn next_authorization_persistence_input<M: RawMutex>(
    progress: &RemoteControlAuthorizationPersistenceProgress,
    events: &RemoteControlPairingPersistenceEvents<M>,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
) -> RemoteControlAuthorizationPersistenceInput {
    match progress {
        RemoteControlAuthorizationPersistenceProgress::ControllerGrant(controller_grants) => {
            if controller_grants.is_waiting_for_store() {
                if let Some(stores) = stores {
                    return RemoteControlAuthorizationPersistenceInput::ControllerGrantStoreCompleted(
                        stores.next_completion().await,
                    );
                }
            }
            core::future::pending().await
        }
        RemoteControlAuthorizationPersistenceProgress::Pairing(pairing) => {
            RemoteControlAuthorizationPersistenceInput::Pairing(
                next_pairing_persistence_input(pairing, events, stores).await,
            )
        }
    }
}

#[inline(never)]
async fn next_pairing_persistence_input<M: RawMutex>(
    progress: &RemoteControlPairingPersistenceProgress,
    events: &RemoteControlPairingPersistenceEvents<M>,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
) -> RemoteControlPairingPersistenceInput {
    if progress.is_ready() {
        return RemoteControlPairingPersistenceInput::Required(events.receive().await);
    }
    if progress.is_waiting_for_store() {
        if let Some(stores) = stores {
            return RemoteControlPairingPersistenceInput::StoreCompleted(
                stores.next_completion().await,
            );
        }
    }
    core::future::pending().await
}

#[inline(never)]
async fn dispatch_prepared<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    request: RunnerRequest<REQUEST_BYTES>,
    prepared: PreparedRunnerRequest,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex + Sync,
    St: prns_runtime::runtime::RemoteControlHostControls,
{
    let inbound = InboundRequest::new(
        request.destination,
        request.link_id,
        request.request_id,
        request.requester,
        request.requested_at,
        request.rtt,
        &request.data,
    );
    let responder = inbound.respond_token();
    let mut body = RunnerResponse::<RESPONSE_BYTES>::Buffered(RespondData::new());
    #[cfg_attr(not(feature = "log"), allow(unused_variables))]
    let kind = request.data.get(1).copied();
    let dispatched = match prepared {
        PreparedRunnerRequest::RemoteControl(verified) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: admit ok kind={kind:?} ctrl={} bytes={}",
                hash4(request.requester),
                request.data.len()
            );
            dispatch_verified_admitted_remote_control_request(
                state, &commands, inbound, &mut body, verified,
            )
            .await
        }
        PreparedRunnerRequest::Application => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: not_control_dest kind={kind:?} ctrl={} bytes={}",
                hash4(request.requester),
                request.data.len()
            );
            dispatch_request::<St, R>(state, &commands, request.path_hash, inbound, &mut body).await
        }
        PreparedRunnerRequest::Declined(reason) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: admit declined kind={kind:?} ctrl={} bytes={}",
                hash4(request.requester),
                request.data.len()
            );
            Err(reason)
        }
    };
    match dispatched {
        Ok(()) => {
            #[cfg_attr(not(feature = "log"), allow(unused_variables))]
            let reply_len = match &body {
                RunnerResponse::Buffered(body) => body.len(),
                RunnerResponse::Resource(body) => body.len().saturating_sub(RESPONSE_WIRE_OVERHEAD),
                RunnerResponse::StaticBytes(bytes) => bytes.len(),
                #[cfg(feature = "large-static-responses")]
                RunnerResponse::StaticFile { bytes, .. } => bytes.len(),
            };
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: reply queued kind={kind:?} bytes={reply_len}"
            );
            match body {
                RunnerResponse::Buffered(body) => {
                    commands.respond_owned_packed(responder, body);
                }
                RunnerResponse::Resource(body) => {
                    commands.respond_owned_resource(responder, body).await;
                }
                RunnerResponse::StaticBytes(bytes) => {
                    commands.respond_static_bytes(responder, bytes);
                }
                #[cfg(feature = "large-static-responses")]
                RunnerResponse::StaticFile { name, bytes } => {
                    commands.respond_static_file(responder, name, bytes);
                }
            }
        }
        Err(Decline::Ignore) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop Ignore kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
        }
        Err(Decline::ResponseTooLarge) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop ResponseTooLarge kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
        }
        Err(Decline::CloseLink) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop CloseLink kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
            commands.close_link(responder.link_id);
        }
    }
}

#[cfg(feature = "log")]
fn hex4(bytes: &[u8]) -> Hex4<'_> {
    Hex4(bytes)
}

#[cfg(feature = "log")]
struct Hex4<'a>(&'a [u8]);

#[cfg(feature = "log")]
impl core::fmt::Display for Hex4<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            [a, b, c, d, ..] => write!(formatter, "{a:02x}{b:02x}{c:02x}{d:02x}"),
            _ => formatter.write_str("----"),
        }
    }
}

#[cfg(feature = "log")]
fn hash4(identity: Option<IdentityHash>) -> Hash4 {
    Hash4(identity)
}

#[cfg(feature = "log")]
struct Hash4(Option<IdentityHash>);

#[cfg(feature = "log")]
impl core::fmt::Display for Hash4 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            None => formatter.write_str("none"),
            Some(identity) => {
                let bytes = identity.as_bytes();
                write!(
                    formatter,
                    "{:02x}{:02x}{:02x}{:02x}",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineState, PrnsCommand};
    use crate::runtime::request_endpoints::{
        RequestContext, RequestEndpoint, RequestEndpointPolicy,
    };
    use embassy_futures::select::{select, Either};
    use embassy_futures::{block_on, join::join};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use prns_core::storage::GrowableHeap;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    fn remote_control_with_administration() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        let capabilities = crate::remote_control::RemoteControlCapabilities::from_requests(
            crate::remote_control::RemoteControlRequestSet::all(),
        )
        .unwrap();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service_with_capabilities(capabilities),
        )
        .expect("Remote Control fits growable storage")
    }

    fn controller(fill: u8) -> crate::remote_control::RemoteControlControllerIdentity {
        use crate::identity::in_memory::InMemoryNodeIdentity;
        use crate::identity::vault::IdentitySecretKey;
        use crate::identity::{IdentityPublicKeys, IdentitySigner};

        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ));
        crate::remote_control::RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: identity.encryption_public_key(),
            signing: identity.signing_public_key(),
        })
    }

    fn authorize_controller_request(
        remote_control: &AssembledRemoteControl,
        administrator: crate::remote_control::RemoteControlControllerIdentity,
        operator: crate::remote_control::RemoteControlControllerIdentity,
    ) -> RunnerRequest<{ crate::remote_control::RemoteControlRequest::MAX_ENCODED_LEN }> {
        use crate::remote_control::{
            RemoteControlRequest, RemoteControlRequestKind, RemoteControlRequestSet,
        };

        let mut data = HeaplessVec::new();
        let mut encoded = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlRequest::AuthorizeController {
            controller: operator,
            permitted_requests: RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        }
        .write_into(&mut encoded)
        .unwrap();
        data.extend_from_slice(&encoded[..encoded_len]).unwrap();
        RunnerRequest {
            destination: remote_control.target_endpoint().unwrap().destination_hash(),
            link_id: LinkId::new([0x81; 16]),
            request_id: RequestId([0x82; 16]),
            requester: Some(administrator.identity_hash()),
            path_hash: remote_control.request_endpoint_id().unwrap(),
            requested_at: InstantMillis(83),
            rtt: RttMillis::new(84),
            data,
        }
    }

    struct DestinationEcho;
    struct DestinationRoutes;
    struct ControllerRoutes;

    impl RequestEndpoint<crate::runtime::NoRemoteControlHostControls> for DestinationEcho {
        const ENDPOINT_ID: &'static str = "/destination";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            let destination = context.destination;
            context.respond(destination.as_bytes())
        }
    }

    impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls> for DestinationRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(DestinationEcho::ENDPOINT_ID, DestinationEcho::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(DestinationEcho::ENDPOINT_ID) {
                DestinationEcho::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls> for ControllerRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[(
            "/controller",
            RequestEndpointPolicy::AllowRemoteControlControllers,
        )];

        async fn dispatch(
            _context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
            _node: &impl crate::runtime::PrnsNodeApi,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            Err(Decline::Ignore)
        }
    }

    fn prepare_controller_route<const N: usize>(
        remote_control: &mut AssembledRemoteControl,
        request: &RunnerRequest<N>,
    ) -> PreparedRunnerRequest {
        prepare_runner_request::<crate::runtime::NoRemoteControlHostControls, ControllerRoutes, N>(
            remote_control,
            request,
        )
    }

    #[test]
    fn controller_routes_follow_the_live_grant_table() {
        use crate::remote_control::{
            RemoteControlControllerAuthority, RemoteControlControllerGrant, RemoteControlRequestSet,
        };

        let mut remote_control = remote_control();
        let controller = controller(0x41);
        let mut request = RunnerRequest {
            destination: DestinationHash::new([0x5a; 16]),
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: Some(controller.identity_hash()),
            path_hash: RequestPathHash::of("/controller"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 1>::new(),
        };
        assert!(matches!(
            prepare_controller_route(&mut remote_control, &request),
            PreparedRunnerRequest::Declined(Decline::Ignore)
        ));

        let grant = RemoteControlControllerGrant::new(
            controller,
            RemoteControlControllerAuthority::Operator,
            RemoteControlRequestSet::all_operator(),
        )
        .unwrap();
        remote_control.set_controller_grant(grant).unwrap();
        assert!(matches!(
            prepare_controller_route(&mut remote_control, &request),
            PreparedRunnerRequest::Application
        ));

        request.requester = None;
        assert!(matches!(
            prepare_controller_route(&mut remote_control, &request),
            PreparedRunnerRequest::Declined(Decline::Ignore)
        ));
    }

    struct StaticPage;
    struct StaticRoutes;
    static PAGE: [u8; 1200] = [0x21; 1200];

    #[cfg(feature = "large-static-responses")]
    #[test]
    fn static_file_sink_preserves_filename_and_borrowed_bytes() {
        let mut response = RunnerResponse::<0>::Buffered(RespondData::new());
        ResponseSink::put_static_file(&mut response, "source.zip", &PAGE).unwrap();
        let RunnerResponse::StaticFile { name, bytes } = response else {
            panic!("static file response");
        };
        assert_eq!(name, "source.zip");
        assert_eq!(bytes.as_ptr(), PAGE.as_ptr());
    }

    impl RequestEndpoint<crate::runtime::NoRemoteControlHostControls> for StaticPage {
        const ENDPOINT_ID: &'static str = "/page";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            context.respond_static_messagepack_bytes(&PAGE)
        }
    }

    impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls> for StaticRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(StaticPage::ENDPOINT_ID, StaticPage::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(StaticPage::ENDPOINT_ID) {
                StaticPage::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    #[test]
    fn dispatch_hands_a_borrowed_body_to_the_borrowed_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let mut remote_control = remote_control();
        let request = RunnerRequest {
            destination: DestinationHash::new([0x5A; 16]),
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/page"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        let prepared = prepare_runner_request::<
            crate::runtime::NoRemoteControlHostControls,
            StaticRoutes,
            16,
        >(&mut remote_control, &request);
        block_on(dispatch_prepared::<
            crate::runtime::NoRemoteControlHostControls,
            StaticRoutes,
            M,
            1,
            1,
            0,
            0,
            16,
        >(
            &crate::runtime::NoRemoteControlHostControls,
            handle,
            request,
            prepared,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        assert_eq!(response.link_id, LinkId::new([1; 16]));
        assert_eq!(response.request_id, RequestId([2; 16]));
        let crate::engine::RespondPayload::StaticBytes(data) = response.payload else {
            panic!("static response");
        };
        assert_eq!(data.as_ptr(), PAGE.as_ptr());
        assert_eq!(data.len(), PAGE.len());
    }

    #[test]
    fn dispatch_answers_through_the_embassy_command_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let mut remote_control = remote_control();
        let destination = DestinationHash::new([0x5a; 16]);
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/destination"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        let prepared = prepare_runner_request::<
            crate::runtime::NoRemoteControlHostControls,
            DestinationRoutes,
            16,
        >(&mut remote_control, &request);
        block_on(dispatch_prepared::<
            crate::runtime::NoRemoteControlHostControls,
            DestinationRoutes,
            M,
            1,
            1,
            0,
            0,
            16,
        >(
            &crate::runtime::NoRemoteControlHostControls,
            handle,
            request,
            prepared,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        let crate::engine::RespondPayload::Packed(data) = response.payload else {
            panic!("packed response");
        };
        assert_eq!(data.as_slice(), destination.as_bytes());
    }

    #[test]
    fn unavailable_remote_control_rejects_controller_grant_changes() {
        use crate::remote_control::RemoteControlRequestKind;
        use crate::runtime::{
            RemoteControlControllerGrantControl, RevokeRemoteControlControllerControlError,
            SetRemoteControlControllerGrantControlError,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut remote_control = crate::runtime::configure_remote_control_service(
            &mut engine,
            crate::remote_control::RemoteControlService::Unavailable,
        )
        .expect("unavailable RemoteControl requires no storage");
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let router =
            run_router::<crate::runtime::NoRemoteControlHostControls, (), M, 1, 0, 0, 0, 1, 16>(
                &crate::runtime::NoRemoteControlHostControls,
                &mut remote_control,
                requests.receiver(),
                &pairing_events,
                None,
                handle,
            );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Err(SetRemoteControlControllerGrantControlError::Unavailable),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Err(RevokeRemoteControlControllerControlError::Unavailable),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
    }

    #[test]
    fn router_applies_ready_remote_control_controller_grants_before_a_ready_request() {
        use crate::remote_control::{
            RemoteControlControllerGrantTable, RemoteControlDescription, RemoteControlRequest,
            RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
            RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
        };
        use crate::runtime::RemoteControlControllerGrantControl;

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control();
        let destination = remote_control.target_endpoint().unwrap().destination_hash();
        let path_hash = remote_control.request_endpoint_id().unwrap();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let mut request_bytes = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let request_len = RemoteControlRequest::Describe
            .write_into(&mut request_bytes)
            .unwrap();
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([0x71; 16]),
            request_id: RequestId([0x72; 16]),
            requester: Some(grant.controller().identity_hash()),
            path_hash,
            requested_at: InstantMillis(73),
            rtt: RttMillis::new(74),
            data: HeaplessVec::from_slice(&request_bytes[..request_len]).unwrap(),
        };
        assert!(requests.try_send(request).is_ok());
        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            DestinationRoutes,
            M,
            1,
            0,
            0,
            0,
            1,
            16,
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            Some(&authorization_stores),
            handle,
        );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Ok(SetRemoteControlControllerGrantOutcome::Added),
            );
            let issued = commands.receiver().receive().await;
            let PrnsCommand::Respond(response) = issued.command else {
                panic!("RemoteControl response command")
            };
            let crate::engine::RespondPayload::Packed(data) = response.payload else {
                panic!("packed RemoteControl response")
            };
            let expected = RemoteControlDescription::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap();
            assert_eq!(
                RemoteControlResponse::parse(data.as_slice()),
                Ok(RemoteControlResponse::Describe(expected)),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
            );
        };

        let persist = async {
            authorization_stores.settle_next_test_store(Ok(())).await;
            authorization_stores.settle_next_test_store(Ok(())).await;
        };
        match block_on(select(join(exercise, persist), router)) {
            Either::First(((), ())) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }

    #[test]
    fn remote_administrator_grant_commits_after_durable_store_without_command_lane_reentry() {
        use crate::remote_control::{
            RemoteControlAuthorizeControllerOutcome, RemoteControlControllerAuthority,
            RemoteControlControllerGrant, RemoteControlControllerGrantTable, RemoteControlRequest,
            RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests =
            Channel::<M, RunnerRequest<{ RemoteControlRequest::MAX_ENCODED_LEN }>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control_with_administration();
        let administrator_identity = *super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        )
        .controller();
        let administrator = RemoteControlControllerGrant::new(
            administrator_identity,
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        remote_control.set_controller_grant(administrator).unwrap();
        let operator = controller(0x44);
        assert!(requests
            .try_send(authorize_controller_request(
                &remote_control,
                administrator_identity,
                operator,
            ))
            .is_ok());

        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            (),
            M,
            1,
            0,
            0,
            0,
            1,
            { RemoteControlRequest::MAX_ENCODED_LEN },
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            Some(&authorization_stores),
            handle,
        );
        let exercise = async {
            let issued = commands.receiver().receive().await;
            let PrnsCommand::Respond(response) = issued.command else {
                panic!("Remote Control response command")
            };
            let crate::engine::RespondPayload::Packed(data) = response.payload else {
                panic!("packed Remote Control response")
            };
            assert_eq!(
                RemoteControlResponse::parse(data.as_slice()),
                Ok(RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Applied,
                )),
            );
        };
        let persist = authorization_stores.settle_next_test_store(Ok(()));
        match block_on(select(join(exercise, persist), router)) {
            Either::First(((), ())) => {}
            Either::Second(()) => panic!("router returned"),
        }

        let granted = remote_control
            .controller_grants()
            .unwrap()
            .grant_for(&operator.identity_hash())
            .copied()
            .expect("operator grant activates after durable persistence");
        assert_eq!(
            granted.authority(),
            RemoteControlControllerAuthority::Operator
        );
        assert_eq!(
            *granted.permitted_requests(),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
        );
    }

    #[test]
    fn remote_administrator_grant_rolls_back_when_its_success_response_cannot_be_queued() {
        use crate::remote_control::{
            RemoteControlControllerAuthority, RemoteControlControllerGrant,
            RemoteControlControllerGrantTable, RemoteControlRequest, RemoteControlRequestKind,
            RemoteControlRequestSet,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests =
            Channel::<M, RunnerRequest<{ RemoteControlRequest::MAX_ENCODED_LEN }>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control_with_administration();
        let administrator_identity = *super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        )
        .controller();
        let administrator = RemoteControlControllerGrant::new(
            administrator_identity,
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        remote_control.set_controller_grant(administrator).unwrap();
        let operator = controller(0x46);
        assert!(requests
            .try_send(authorize_controller_request(
                &remote_control,
                administrator_identity,
                operator,
            ))
            .is_ok());
        assert!(handle.close_link(LinkId::new([0x91; 16])));

        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            (),
            M,
            1,
            0,
            0,
            0,
            1,
            { RemoteControlRequest::MAX_ENCODED_LEN },
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            Some(&authorization_stores),
            handle,
        );
        let persist = async {
            authorization_stores.settle_next_test_store(Ok(())).await;
            authorization_stores.settle_next_test_store(Ok(())).await;
        };
        match block_on(select(persist, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }

        assert!(remote_control
            .controller_grants()
            .unwrap()
            .grant_for(&operator.identity_hash())
            .is_none());
        let Ok(issued) = commands.try_receive() else {
            panic!("prefilled command remains queued")
        };
        assert!(matches!(
            issued.command,
            PrnsCommand::CloseLink(close) if close.link_id == LinkId::new([0x91; 16])
        ));
        assert!(commands.try_receive().is_err());
    }

    #[test]
    fn remote_administrator_grant_rolls_back_after_initial_store_failure() {
        use crate::remote_control::{
            RemoteControlAuthorizeControllerOutcome, RemoteControlControllerAuthority,
            RemoteControlControllerGrant, RemoteControlControllerGrantTable, RemoteControlRequest,
            RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests =
            Channel::<M, RunnerRequest<{ RemoteControlRequest::MAX_ENCODED_LEN }>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control_with_administration();
        let administrator_identity = *super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        )
        .controller();
        let administrator = RemoteControlControllerGrant::new(
            administrator_identity,
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        remote_control.set_controller_grant(administrator).unwrap();
        let operator = controller(0x45);
        assert!(requests
            .try_send(authorize_controller_request(
                &remote_control,
                administrator_identity,
                operator,
            ))
            .is_ok());

        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            (),
            M,
            1,
            0,
            0,
            0,
            1,
            { RemoteControlRequest::MAX_ENCODED_LEN },
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            Some(&authorization_stores),
            handle,
        );
        let exercise = async {
            let issued = commands.receiver().receive().await;
            let PrnsCommand::Respond(response) = issued.command else {
                panic!("Remote Control response command")
            };
            let crate::engine::RespondPayload::Packed(data) = response.payload else {
                panic!("packed Remote Control response")
            };
            assert_eq!(
                RemoteControlResponse::parse(data.as_slice()),
                Ok(RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Failed,
                )),
            );
        };
        let persist = async {
            authorization_stores
                .settle_next_test_store(Err(
                    super::super::embedded_persistence::EmbeddedPersistenceFailure::Flash,
                ))
                .await;
            authorization_stores.settle_next_test_store(Ok(())).await;
        };
        match block_on(select(join(exercise, persist), router)) {
            Either::First(((), ())) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert!(remote_control
            .controller_grants()
            .unwrap()
            .grant_for(&operator.identity_hash())
            .is_none());
    }

    #[test]
    fn target_access_is_busy_while_a_controller_grant_store_is_unsettled() {
        use crate::remote_control::{RemoteControlControllerGrantTable, RemoteControlRequestKind};
        use crate::runtime::{
            RemoteControlControllerGrantControl, RemoteControlTargetAccessControl,
            RemoteControlTargetInventoryControlError, ResolveRemoteControlTargetControlError,
        };
        use embassy_sync::signal::Signal;

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let store_observed = Signal::<M, ()>::new();
        let release_store = Signal::<M, ()>::new();
        let mut remote_control = remote_control();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let router =
            run_router::<crate::runtime::NoRemoteControlHostControls, (), M, 1, 0, 0, 0, 1, 16>(
                &crate::runtime::NoRemoteControlHostControls,
                &mut remote_control,
                requests.receiver(),
                &pairing_events,
                Some(&authorization_stores),
                handle,
            );
        let exercise = async {
            let setting = handle.set_remote_control_controller_grant(grant);
            let inspect_while_unsettled = async {
                store_observed.wait().await;
                let (inventory, resolution) = join(
                    handle.remote_control_target_inventory(),
                    handle.resolve_remote_control_target(crate::identity::IdentityHash::new(
                        [0x79; 16],
                    )),
                )
                .await;
                assert_eq!(
                    inventory,
                    Err(RemoteControlTargetInventoryControlError::Busy),
                );
                assert_eq!(
                    resolution,
                    Err(ResolveRemoteControlTargetControlError::Busy),
                );
                release_store.signal(());
            };
            let (setting, ()) = join(setting, inspect_while_unsettled).await;
            assert_eq!(
                setting,
                Ok(crate::remote_control::SetRemoteControlControllerGrantOutcome::Added),
            );
        };
        let persist = async {
            authorization_stores.wait_for_next_test_store().await;
            store_observed.signal(());
            release_store.wait().await;
            authorization_stores.settle_test_store(Ok(()));
        };

        match block_on(select(join(exercise, persist), router)) {
            Either::First(((), ())) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grant_for(&grant.controller().identity_hash()),
            Some(&grant),
        );
    }

    #[test]
    fn router_prioritizes_pairing_transactions_and_rejects_racing_app_mutations() {
        use crate::remote_control::{RemoteControlControllerGrantTable, RemoteControlRequestKind};
        use crate::runtime::{
            RemoteControlControllerGrantControl, RemoteControlTargetAccessControl,
            RemoteControlTargetInventoryControlError, ResolveRemoteControlTargetControlError,
            SetRemoteControlControllerGrantControlError,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control();
        let attempt_id = super::super::node_facade::test_remote_control_pairing_attempt(0x77);
        let pairing_grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let app_grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::AnnounceSelf,
        );
        pairing_events.signal(RemoteControlPairingPersistenceRequired::ControllerGrant {
            attempt_id,
            grant: pairing_grant,
        });
        let router =
            run_router::<crate::runtime::NoRemoteControlHostControls, (), M, 1, 0, 0, 0, 1, 16>(
                &crate::runtime::NoRemoteControlHostControls,
                &mut remote_control,
                requests.receiver(),
                &pairing_events,
                Some(&authorization_stores),
                handle,
            );
        let exercise = async {
            let (app_mutation, (target_inventory, target_resolution)) = join(
                handle.set_remote_control_controller_grant(app_grant),
                join(
                    handle.remote_control_target_inventory(),
                    handle.resolve_remote_control_target(crate::identity::IdentityHash::new(
                        [0x78; 16],
                    )),
                ),
            )
            .await;
            assert_eq!(
                app_mutation,
                Err(SetRemoteControlControllerGrantControlError::Busy),
            );
            assert_eq!(
                target_inventory,
                Err(RemoteControlTargetInventoryControlError::Busy),
            );
            assert_eq!(
                target_resolution,
                Err(ResolveRemoteControlTargetControlError::Busy),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[],
        );
    }
}
