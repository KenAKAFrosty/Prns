use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use crate::engine::{
    Journaled, RemoteControlControllerPairingFinalization,
    RemoteControlControllerPairingPersistence, RemoteControlTargetPairingAuthorizationPersistence,
    RemoteControlTargetPairingFinalization, SettleRemoteControlControllerPairingPersistence,
    SettleRemoteControlControllerPairingPersistenceFailure,
    SettleRemoteControlTargetPairingAuthorization,
    SettleRemoteControlTargetPairingAuthorizationFailure, Settleable,
};
use crate::identity::IdentityPublicKeys;
use crate::remote_control::{
    RemoteControlControllerAuthority, RemoteControlControllerGrant, RemoteControlPairingAttemptId,
    RemoteControlRequestSet,
};
use crate::storage::StorageLayout;

use super::embedded_persistence::{
    EmbeddedPersistenceFailure, ManifoldPersistence, RemoteControlAuthorizationSnapshot,
    RemoteControlAuthorizationSnapshotKind, StoreRemoteControlAuthorizationSnapshotOutcome,
    DISCOVERY_GROUP_CONFIGURATION_STORES,
};
use super::node_facade::PrnsNodeHandle;
use super::remote_control_pairing_authorizations::{
    activate as activate_authorization, prepare as prepare_authorization,
    release as release_authorization_transaction, roll_back as roll_back_authorization,
    snapshot_rollback as snapshot_authorization_rollback,
    RemoteControlPairingAuthorizationTransactionState,
};
use super::remote_control_pairing_authorizations::{
    RemoteControlPairingAuthorization, RemoteControlPairingAuthorizationTransactionFailure,
};
use super::AssembledRemoteControl;
use super::RemoteControlPairingControlError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedRemoteControlPairingPersistenceOperation {
    PrepareAuthorization,
    SnapshotRollback,
    StoreAuthorization,
    ActivateAuthorization,
    SettlePersisted,
    RollBackAuthorization,
    StoreRollback,
    ReleaseAuthorization,
    SettleFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedRemoteControlControllerPairingFinalization {
    Completed,
    PersistenceFailureRecorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedRemoteControlTargetPairingFinalization {
    CompletionDispatched,
    AuthorizationRollbackRequired,
    AuthorizationFailureRecorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedRemoteControlPairingPersistenceFailure {
    AuthorizationTransaction {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        failure: RemoteControlPairingAuthorizationTransactionFailure,
    },
    Storage {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        failure: EmbeddedPersistenceFailure,
    },
    TargetSettlement {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        failure: SettleRemoteControlTargetPairingAuthorizationFailure,
    },
    ControllerSettlement {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        failure: SettleRemoteControlControllerPairingPersistenceFailure,
    },
    UnexpectedTargetFinalization {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        finalization: EmbeddedRemoteControlTargetPairingFinalization,
    },
    UnexpectedControllerFinalization {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        finalization: EmbeddedRemoteControlControllerPairingFinalization,
    },
    RollbackSnapshotMismatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
    SettlementBusy {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
    },
    NodeStopped {
        attempt_id: RemoteControlPairingAttemptId,
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteControlPairingPersistenceRequired {
    ControllerGrant {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    TargetAccess {
        attempt_id: RemoteControlPairingAttemptId,
        target_public_keys: IdentityPublicKeys,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteControlControllerGrantPersistenceRequired {
    attempt_id: RemoteControlPairingAttemptId,
    grant: RemoteControlControllerGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteControlTargetAccessPersistenceRequired {
    attempt_id: RemoteControlPairingAttemptId,
    target_public_keys: IdentityPublicKeys,
    authority: RemoteControlControllerAuthority,
    permitted_requests: RemoteControlRequestSet,
}

pub(super) struct RemoteControlPairingPersistenceEvents<M: RawMutex> {
    controller_grant: Signal<M, RemoteControlControllerGrantPersistenceRequired>,
    target_access: Signal<M, RemoteControlTargetAccessPersistenceRequired>,
}

impl<M: RawMutex> RemoteControlPairingPersistenceEvents<M> {
    pub(super) const fn new() -> Self {
        Self {
            controller_grant: Signal::new(),
            target_access: Signal::new(),
        }
    }

    pub(super) fn signal(&self, required: RemoteControlPairingPersistenceRequired) {
        match required {
            RemoteControlPairingPersistenceRequired::ControllerGrant { attempt_id, grant } => {
                self.controller_grant
                    .signal(RemoteControlControllerGrantPersistenceRequired { attempt_id, grant });
            }
            RemoteControlPairingPersistenceRequired::TargetAccess {
                attempt_id,
                target_public_keys,
                authority,
                permitted_requests,
            } => {
                self.target_access
                    .signal(RemoteControlTargetAccessPersistenceRequired {
                        attempt_id,
                        target_public_keys,
                        authority,
                        permitted_requests,
                    });
            }
        }
    }

    pub(super) async fn receive(&self) -> RemoteControlPairingPersistenceRequired {
        match embassy_futures::select::select(
            self.controller_grant.wait(),
            self.target_access.wait(),
        )
        .await
        {
            embassy_futures::select::Either::First(required) => {
                RemoteControlPairingPersistenceRequired::ControllerGrant {
                    attempt_id: required.attempt_id,
                    grant: required.grant,
                }
            }
            embassy_futures::select::Either::Second(required) => {
                RemoteControlPairingPersistenceRequired::TargetAccess {
                    attempt_id: required.attempt_id,
                    target_public_keys: required.target_public_keys,
                    authority: required.authority,
                    permitted_requests: required.permitted_requests,
                }
            }
        }
    }
}

impl RemoteControlPairingPersistenceRequired {
    pub(super) fn copy_from(journaled: &Journaled<'_>) -> Option<Self> {
        match journaled {
            Journaled::RemoteControlTargetPairingAuthorizationRequired { attempt_id, grant } => {
                Some(Self::ControllerGrant {
                    attempt_id: *attempt_id,
                    grant: *grant,
                })
            }
            Journaled::RemoteControlControllerPairingPersistenceRequired(pairing) => {
                Some(Self::TargetAccess {
                    attempt_id: pairing.attempt_id(),
                    target_public_keys: *pairing.access().target().public_keys(),
                    authority: pairing.access().authority(),
                    permitted_requests: *pairing.access().permitted_requests(),
                })
            }
            Journaled::AnnounceHeldDropped { .. }
            | Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::PersistenceFlushed { .. }
            | Journaled::PersistenceFlushFailed { .. }
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::AnnounceHeard { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::LinkInterfaceMismatch { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::RemoteControlPairingExpired { .. }
            | Journaled::RemoteControlPairingAvailabilityObserved(_)
            | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
            | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
            | Journaled::RemoteControlTargetPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
            | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlControllerPairingAuthorizationPersistenceFailed { .. }
            | Journaled::RemoteControlControllerPairingExpired { .. }
            | Journaled::RemoteControlControllerPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingExpired { .. }
            | Journaled::RemoteControlTargetPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
            | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
            | Journaled::RemoteControlPairingExpiryFailed { .. } => None,
        }
    }

    const fn attempt_id(&self) -> RemoteControlPairingAttemptId {
        match self {
            Self::ControllerGrant { attempt_id, .. } | Self::TargetAccess { attempt_id, .. } => {
                *attempt_id
            }
        }
    }

    const fn snapshot_kind(&self) -> RemoteControlAuthorizationSnapshotKind {
        match self {
            Self::ControllerGrant { .. } => {
                RemoteControlAuthorizationSnapshotKind::ControllerGrants
            }
            Self::TargetAccess { .. } => RemoteControlAuthorizationSnapshotKind::TargetAccesses,
        }
    }

    const fn authorization(&self) -> RemoteControlPairingAuthorization {
        match self {
            Self::ControllerGrant { grant, .. } => {
                RemoteControlPairingAuthorization::ControllerGrant(*grant)
            }
            Self::TargetAccess {
                target_public_keys,
                authority,
                permitted_requests,
                ..
            } => RemoteControlPairingAuthorization::TargetAccess {
                target_public_keys: *target_public_keys,
                authority: *authority,
                permitted_requests: *permitted_requests,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteControlAuthorizationStoreRequirement {
    Initial,
    Rollback,
}

struct RemoteControlAuthorizationStoreRequest {
    kind: RemoteControlAuthorizationSnapshotKind,
    snapshot: RemoteControlAuthorizationSnapshot,
    requirement: RemoteControlAuthorizationStoreRequirement,
}

struct PendingRemoteControlAuthorizationStore {
    request: RemoteControlAuthorizationStoreRequest,
    ready_at: Option<crate::engine::InstantMillis>,
}

pub(super) struct RemoteControlAuthorizationStoreExchange<M: RawMutex> {
    requests: Signal<M, RemoteControlAuthorizationStoreRequest>,
    failures: Channel<M, EmbeddedRemoteControlPairingPersistenceFailure, 1>,
    completed: Signal<M, Result<(), EmbeddedPersistenceFailure>>,
}

impl<M: RawMutex> RemoteControlAuthorizationStoreExchange<M> {
    pub(super) const fn new() -> Self {
        Self {
            requests: Signal::new(),
            failures: Channel::new(),
            completed: Signal::new(),
        }
    }

    #[cfg(test)]
    async fn store(
        &self,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: RemoteControlAuthorizationSnapshot,
        requirement: RemoteControlAuthorizationStoreRequirement,
    ) -> Result<(), EmbeddedPersistenceFailure> {
        self.submit(kind, snapshot, requirement);
        self.completed.wait().await
    }

    pub(super) fn submit(
        &self,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: RemoteControlAuthorizationSnapshot,
        requirement: RemoteControlAuthorizationStoreRequirement,
    ) {
        self.completed.reset();
        self.requests
            .signal(RemoteControlAuthorizationStoreRequest {
                kind,
                snapshot,
                requirement,
            });
    }

    pub(super) async fn next_completion(&self) -> Result<(), EmbeddedPersistenceFailure> {
        self.completed.wait().await
    }

    #[cfg(test)]
    pub(super) async fn settle_next_test_store(
        &self,
        result: Result<(), EmbeddedPersistenceFailure>,
    ) {
        self.wait_for_next_test_store().await;
        self.settle_test_store(result);
    }

    #[cfg(test)]
    pub(super) async fn wait_for_next_test_store(&self) {
        let _request = self.requests.wait().await;
    }

    #[cfg(test)]
    pub(super) fn settle_test_store(&self, result: Result<(), EmbeddedPersistenceFailure>) {
        self.completed.signal(result);
    }

    pub(super) async fn report_failure(
        &self,
        failure: EmbeddedRemoteControlPairingPersistenceFailure,
    ) {
        self.failures.send(failure).await;
    }

    fn try_take_request(&self) -> Option<RemoteControlAuthorizationStoreRequest> {
        self.requests.try_take()
    }

    fn try_take_failure(&self) -> Option<EmbeddedRemoteControlPairingPersistenceFailure> {
        self.failures.try_receive().ok()
    }

    fn has_failure(&self) -> bool {
        !self.failures.is_empty()
    }

    fn settle(&self, result: Result<(), EmbeddedPersistenceFailure>) {
        self.completed.signal(result);
    }
}

pub(super) struct RemoteControlPairingManifoldPersistence<'a, M, P>
where
    M: RawMutex,
{
    persistence: &'a mut P,
    stores: &'a RemoteControlAuthorizationStoreExchange<M>,
    pending_request: Option<PendingRemoteControlAuthorizationStore>,
}

impl<'a, M, P> RemoteControlPairingManifoldPersistence<'a, M, P>
where
    M: RawMutex,
{
    pub(super) const fn new(
        persistence: &'a mut P,
        stores: &'a RemoteControlAuthorizationStoreExchange<M>,
    ) -> Self {
        Self {
            persistence,
            stores,
            pending_request: None,
        }
    }
}

impl<S, M, P> ManifoldPersistence<S> for RemoteControlPairingManifoldPersistence<'_, M, P>
where
    S: StorageLayout,
    M: RawMutex,
    P: ManifoldPersistence<S>,
{
    fn observe(&mut self, journaled: &Journaled<'_>, now: crate::engine::InstantMillis) {
        self.persistence.observe(journaled, now);
    }

    fn observe_remote_control_pairing_failure(
        &mut self,
        failure: EmbeddedRemoteControlPairingPersistenceFailure,
    ) {
        self.persistence
            .observe_remote_control_pairing_failure(failure);
    }

    fn deadline(
        &mut self,
        now: crate::engine::InstantMillis,
    ) -> Option<crate::engine::InstantMillis> {
        if self.pending_request.is_none() {
            self.pending_request = self.stores.try_take_request().map(|request| {
                PendingRemoteControlAuthorizationStore {
                    request,
                    ready_at: Some(now),
                }
            });
        }
        // Leave failures in their bounded queue until progress observes them. Merely
        // checking readiness must not require another resident failure payload.
        if self.stores.has_failure() {
            return Some(now);
        }
        // New work and compaction steps are immediate, but a retained failed rollback
        // must respect the store's retry schedule. Unrelated persistence can still run.
        let store_ready = self
            .pending_request
            .as_ref()
            .and_then(|pending| pending.ready_at);
        match (store_ready, self.persistence.deadline(now)) {
            (Some(store), Some(other)) => Some(crate::engine::InstantMillis(store.0.min(other.0))),
            (Some(ready), None) | (None, Some(ready)) => Some(ready),
            (None, None) => None,
        }
    }

    async fn wait_for_work(&self) {
        match embassy_futures::select::select3(
            self.stores.requests.wait(),
            self.stores.failures.ready_to_receive(),
            self.persistence.wait_for_work(),
        )
        .await
        {
            embassy_futures::select::Either3::First(request) => {
                self.stores.requests.signal(request);
            }
            embassy_futures::select::Either3::Second(()) => {}
            embassy_futures::select::Either3::Third(()) => {}
        }
    }

    async fn progress(
        &mut self,
        engine: &mut crate::engine::EngineState<S>,
        now: crate::engine::InstantMillis,
    ) {
        if let Some(failure) = self.stores.try_take_failure() {
            self.persistence
                .observe_remote_control_pairing_failure(failure);
            return;
        }
        if DISCOVERY_GROUP_CONFIGURATION_STORES.has_pending_request() {
            self.persistence.progress(engine, now).await;
            return;
        }
        let Some(pending) = self
            .pending_request
            .as_mut()
            .filter(|pending| pending.ready_at.is_some_and(|ready| now.0 >= ready.0))
        else {
            self.persistence.progress(engine, now).await;
            return;
        };
        let request = &pending.request;
        match self
            .persistence
            .store_remote_control_authorization_snapshot(
                engine,
                request.kind,
                &request.snapshot,
                now,
            )
            .await
        {
            StoreRemoteControlAuthorizationSnapshotOutcome::Stored => {
                self.pending_request = None;
                self.stores.settle(Ok(()));
            }
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {
                pending.ready_at = Some(now);
            }
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed { failure, retry_at } => {
                match request.requirement {
                    RemoteControlAuthorizationStoreRequirement::Initial => {
                        self.pending_request = None;
                        self.stores.settle(Err(failure));
                    }
                    RemoteControlAuthorizationStoreRequirement::Rollback => {
                        pending.ready_at = retry_at;
                    }
                }
            }
        }
    }

    async fn store_remote_control_authorization_snapshot(
        &mut self,
        engine: &crate::engine::EngineState<S>,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: &RemoteControlAuthorizationSnapshot,
        now: crate::engine::InstantMillis,
    ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
        self.persistence
            .store_remote_control_authorization_snapshot(engine, kind, snapshot, now)
            .await
    }
}

pub(super) struct RemoteControlPairingPersistenceProgress {
    state: RemoteControlPairingPersistenceState,
}

// The initial-store state must own a fixed-capacity rollback snapshot until flash confirms the
// projected authorization. This runtime is `no_std` and allocation-free, so heap-indirecting the
// uncommon large variant is not available; retaining the tagged state avoids a second resident
// snapshot buffer and keeps ownership explicit across the persistence continuation.
#[allow(clippy::large_enum_variant)]
enum RemoteControlPairingPersistenceState {
    Ready,
    WaitingInitialStore {
        required: RemoteControlPairingPersistenceRequired,
        rollback: RemoteControlAuthorizationSnapshot,
    },
    WaitingRollbackStore {
        required: RemoteControlPairingPersistenceRequired,
        rollback_failure: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
        completion: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
    },
}

enum AcceptRequiredContinuation {
    AwaitingStore,
    SettleActivated,
    SettleFailure {
        failure: EmbeddedRemoteControlPairingPersistenceFailure,
        release_authorization: bool,
    },
}

impl RemoteControlPairingPersistenceProgress {
    pub(super) const fn new() -> Self {
        Self {
            state: RemoteControlPairingPersistenceState::Ready,
        }
    }

    pub(super) const fn is_ready(&self) -> bool {
        matches!(self.state, RemoteControlPairingPersistenceState::Ready)
    }

    pub(super) const fn is_waiting_for_store(&self) -> bool {
        matches!(
            self.state,
            RemoteControlPairingPersistenceState::WaitingInitialStore { .. }
                | RemoteControlPairingPersistenceState::WaitingRollbackStore { .. }
        )
    }

    #[inline(never)]
    pub(super) async fn accept_required<
        M,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    >(
        &mut self,
        required: RemoteControlPairingPersistenceRequired,
        remote_control: &mut AssembledRemoteControl,
        authorization: &mut RemoteControlPairingAuthorizationTransactionState,
        stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
        node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    ) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
    where
        M: RawMutex,
    {
        match self.begin_required(required, remote_control, authorization, stores) {
            AcceptRequiredContinuation::AwaitingStore => Ok(()),
            AcceptRequiredContinuation::SettleActivated => {
                settle_activated_authorization(required, authorization, node).await
            }
            AcceptRequiredContinuation::SettleFailure {
                failure,
                release_authorization,
            } => {
                if let Some(stores) = stores {
                    stores.report_failure(failure).await;
                }
                if release_authorization {
                    release_authorization_locally(authorization, required.attempt_id())?;
                }
                settle_persistence_failure(required, node).await
            }
        }
    }

    #[inline(never)]
    fn begin_required<M: RawMutex>(
        &mut self,
        required: RemoteControlPairingPersistenceRequired,
        remote_control: &mut AssembledRemoteControl,
        authorization: &mut RemoteControlPairingAuthorizationTransactionState,
        stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    ) -> AcceptRequiredContinuation {
        debug_assert!(self.is_ready());
        let attempt_id = required.attempt_id();
        let projected = match prepare_authorization(
            remote_control,
            authorization,
            attempt_id,
            required.authorization(),
        ) {
            Ok(projected) => projected,
            Err(failure) => {
                return AcceptRequiredContinuation::SettleFailure {
                    failure:
                        EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                            attempt_id,
                            operation:
                                EmbeddedRemoteControlPairingPersistenceOperation::PrepareAuthorization,
                            failure,
                        },
                    release_authorization: false,
                };
            }
        };
        let rollback =
            match snapshot_authorization_rollback(remote_control, authorization, attempt_id) {
                Ok(rollback) => rollback,
                Err(failure) => {
                    return AcceptRequiredContinuation::SettleFailure {
                    failure:
                        EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                            attempt_id,
                            operation:
                                EmbeddedRemoteControlPairingPersistenceOperation::SnapshotRollback,
                            failure,
                        },
                    release_authorization: true,
                };
                }
            };
        if let Some(stores) = stores {
            stores.submit(
                required.snapshot_kind(),
                projected,
                RemoteControlAuthorizationStoreRequirement::Initial,
            );
            self.state =
                RemoteControlPairingPersistenceState::WaitingInitialStore { required, rollback };
            AcceptRequiredContinuation::AwaitingStore
        } else if let Err(failure) =
            activate_authorization(remote_control, authorization, attempt_id)
        {
            AcceptRequiredContinuation::SettleFailure {
                failure: EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                    attempt_id,
                    operation:
                        EmbeddedRemoteControlPairingPersistenceOperation::ActivateAuthorization,
                    failure,
                },
                release_authorization: true,
            }
        } else {
            AcceptRequiredContinuation::SettleActivated
        }
    }

    #[inline(never)]
    pub(super) async fn accept_store_completion<
        M,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    >(
        &mut self,
        stored: Result<(), EmbeddedPersistenceFailure>,
        remote_control: &mut AssembledRemoteControl,
        authorization: &mut RemoteControlPairingAuthorizationTransactionState,
        stores: &RemoteControlAuthorizationStoreExchange<M>,
        node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    ) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
    where
        M: RawMutex,
    {
        let initial_required = match &self.state {
            RemoteControlPairingPersistenceState::Ready => return Ok(()),
            RemoteControlPairingPersistenceState::WaitingInitialStore { required, .. } => {
                Some(*required)
            }
            RemoteControlPairingPersistenceState::WaitingRollbackStore { .. } => None,
        };
        if let Some(required) = initial_required {
            let attempt_id = required.attempt_id();
            if let Err(failure) = stored {
                let store_failure = EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                    attempt_id,
                    operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreAuthorization,
                    failure,
                };
                let settlement_failure = settle_persistence_failure(required, node).await.err();
                if let Some(failure) = settlement_failure {
                    stores.report_failure(failure).await;
                }
                let rollback = self.take_initial_rollback(required);
                return self.begin_rollback(
                    required,
                    rollback,
                    Some(store_failure),
                    remote_control,
                    authorization,
                    stores,
                );
            }
            if let Err(failure) = activate_authorization(remote_control, authorization, attempt_id)
            {
                let activation_failure =
                    EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::ActivateAuthorization,
                        failure,
                    };
                let settlement_failure = settle_persistence_failure(required, node).await.err();
                if let Some(failure) = settlement_failure {
                    stores.report_failure(failure).await;
                }
                let rollback = self.take_initial_rollback(required);
                return self.begin_rollback(
                    required,
                    rollback,
                    Some(activation_failure),
                    remote_control,
                    authorization,
                    stores,
                );
            }
            let settlement = settle_persisted_authorization(required, node).await;
            let rollback = self.take_initial_rollback(required);
            return match settlement {
                ActivatedAuthorizationSettlement::Release => {
                    release_authorization_locally(authorization, attempt_id)
                }
                ActivatedAuthorizationSettlement::RollBack { failure } => self.begin_rollback(
                    required,
                    rollback,
                    failure,
                    remote_control,
                    authorization,
                    stores,
                ),
            };
        }

        let RemoteControlPairingPersistenceState::WaitingRollbackStore {
            required,
            rollback_failure,
            completion,
        } = core::mem::replace(&mut self.state, RemoteControlPairingPersistenceState::Ready)
        else {
            unreachable!("rollback completion requires a pending rollback store")
        };
        let attempt_id = required.attempt_id();
        stored.map_err(
            |failure| EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreRollback,
                failure,
            },
        )?;
        if let Some(failure) = rollback_failure {
            return Err(failure);
        }
        release_authorization_locally(authorization, attempt_id)?;
        completion.map_or(Ok(()), Err)
    }

    fn take_initial_rollback(
        &mut self,
        expected: RemoteControlPairingPersistenceRequired,
    ) -> RemoteControlAuthorizationSnapshot {
        let RemoteControlPairingPersistenceState::WaitingInitialStore { required, rollback } =
            core::mem::replace(&mut self.state, RemoteControlPairingPersistenceState::Ready)
        else {
            unreachable!("initial-store completion requires a pending initial store")
        };
        debug_assert_eq!(required, expected);
        rollback
    }

    fn begin_rollback<M>(
        &mut self,
        required: RemoteControlPairingPersistenceRequired,
        expected: RemoteControlAuthorizationSnapshot,
        completion: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
        remote_control: &mut AssembledRemoteControl,
        authorization: &mut RemoteControlPairingAuthorizationTransactionState,
        stores: &RemoteControlAuthorizationStoreExchange<M>,
    ) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
    where
        M: RawMutex,
    {
        let attempt_id = required.attempt_id();
        let rollback_failure =
            match roll_back_authorization(remote_control, authorization, attempt_id) {
                Ok(snapshot) if snapshot == expected => None,
                Ok(_) => Some(
                    EmbeddedRemoteControlPairingPersistenceFailure::RollbackSnapshotMismatch {
                        attempt_id,
                    },
                ),
                Err(failure) => Some(
                    EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::RollBackAuthorization,
                        failure,
                    },
                ),
            };
        stores.submit(
            required.snapshot_kind(),
            expected,
            RemoteControlAuthorizationStoreRequirement::Rollback,
        );
        self.state = RemoteControlPairingPersistenceState::WaitingRollbackStore {
            required,
            rollback_failure,
            completion,
        };
        Ok(())
    }
}

fn release_authorization_locally(
    authorization: &mut RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure> {
    release_authorization_transaction(authorization, attempt_id).map_err(|failure| {
        EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
            attempt_id,
            operation: EmbeddedRemoteControlPairingPersistenceOperation::ReleaseAuthorization,
            failure,
        }
    })
}

enum ActivatedAuthorizationSettlement {
    Release,
    RollBack {
        failure: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
    },
}

#[inline(never)]
async fn settle_activated_authorization<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    required: RemoteControlPairingPersistenceRequired,
    authorization: &mut RemoteControlPairingAuthorizationTransactionState,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
where
    M: RawMutex,
{
    let attempt_id = required.attempt_id();
    match settle_persisted_authorization(required, node).await {
        ActivatedAuthorizationSettlement::Release => {
            release_authorization_locally(authorization, attempt_id)
        }
        ActivatedAuthorizationSettlement::RollBack { failure } => {
            release_authorization_locally(authorization, attempt_id)?;
            failure.map_or(Ok(()), Err)
        }
    }
}

#[inline(never)]
async fn settle_persisted_authorization<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    required: RemoteControlPairingPersistenceRequired,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> ActivatedAuthorizationSettlement
where
    M: RawMutex,
{
    let attempt_id = required.attempt_id();
    match required {
        RemoteControlPairingPersistenceRequired::ControllerGrant { .. } => {
            match settle_pairing_command(
                node,
                SettleRemoteControlTargetPairingAuthorization {
                    attempt_id,
                    persistence: RemoteControlTargetPairingAuthorizationPersistence::Persisted,
                },
            )
            .await
            {
                Ok(RemoteControlTargetPairingFinalization::CompletionDispatched { .. }) => {
                    ActivatedAuthorizationSettlement::Release
                }
                Ok(
                    RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired {
                        ..
                    },
                ) => ActivatedAuthorizationSettlement::RollBack { failure: None },
                Ok(finalization) => ActivatedAuthorizationSettlement::RollBack {
                    failure: Some(unexpected_target_finalization(
                        EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        finalization,
                    )),
                },
                Err(failure) => ActivatedAuthorizationSettlement::RollBack {
                    failure: Some(target_settlement_failure(
                        attempt_id,
                        EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        failure,
                    )),
                },
            }
        }
        RemoteControlPairingPersistenceRequired::TargetAccess { .. } => {
            match settle_pairing_command(
                node,
                SettleRemoteControlControllerPairingPersistence {
                    attempt_id,
                    persistence: RemoteControlControllerPairingPersistence::Persisted,
                },
            )
            .await
            {
                Ok(RemoteControlControllerPairingFinalization::Completed { .. }) => {
                    ActivatedAuthorizationSettlement::Release
                }
                Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                    attempt_id,
                    ..
                }) => ActivatedAuthorizationSettlement::RollBack {
                    failure: Some(
                        EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedControllerFinalization {
                            attempt_id,
                            operation:
                                EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                            finalization: EmbeddedRemoteControlControllerPairingFinalization::PersistenceFailureRecorded,
                        },
                    ),
                },
                Err(failure) => ActivatedAuthorizationSettlement::RollBack {
                    failure: Some(controller_settlement_failure(
                        attempt_id,
                        EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        failure,
                    )),
                },
            }
        }
    }
}

fn target_settlement_failure(
    attempt_id: RemoteControlPairingAttemptId,
    operation: EmbeddedRemoteControlPairingPersistenceOperation,
    failure: RemoteControlPairingSettlementFailure<
        SettleRemoteControlTargetPairingAuthorizationFailure,
    >,
) -> EmbeddedRemoteControlPairingPersistenceFailure {
    match failure {
        RemoteControlPairingSettlementFailure::Failed(failure) => {
            EmbeddedRemoteControlPairingPersistenceFailure::TargetSettlement {
                attempt_id,
                operation,
                failure,
            }
        }
        RemoteControlPairingSettlementFailure::Busy => {
            EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                attempt_id,
                operation,
            }
        }
        RemoteControlPairingSettlementFailure::NodeStopped => {
            EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                attempt_id,
                operation,
            }
        }
    }
}

fn controller_settlement_failure(
    attempt_id: RemoteControlPairingAttemptId,
    operation: EmbeddedRemoteControlPairingPersistenceOperation,
    failure: RemoteControlPairingSettlementFailure<
        SettleRemoteControlControllerPairingPersistenceFailure,
    >,
) -> EmbeddedRemoteControlPairingPersistenceFailure {
    match failure {
        RemoteControlPairingSettlementFailure::Failed(failure) => {
            EmbeddedRemoteControlPairingPersistenceFailure::ControllerSettlement {
                attempt_id,
                operation,
                failure,
            }
        }
        RemoteControlPairingSettlementFailure::Busy => {
            EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                attempt_id,
                operation,
            }
        }
        RemoteControlPairingSettlementFailure::NodeStopped => {
            EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                attempt_id,
                operation,
            }
        }
    }
}

#[inline(never)]
async fn settle_persistence_failure<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    required: RemoteControlPairingPersistenceRequired,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
where
    M: RawMutex,
{
    match required {
        RemoteControlPairingPersistenceRequired::ControllerGrant { attempt_id, .. } => {
            match settle_pairing_command(
                node,
                SettleRemoteControlTargetPairingAuthorization {
                    attempt_id,
                    persistence: RemoteControlTargetPairingAuthorizationPersistence::Failed,
                },
            )
            .await
            {
                Ok(RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
                    ..
                })
                | Ok(RemoteControlTargetPairingFinalization::CompletionDispatched { .. }) => {
                    Ok(())
                }
                Ok(finalization) => Err(unexpected_target_finalization(
                    EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    finalization,
                )),
                Err(failure) => Err(target_settlement_failure(
                    attempt_id,
                    EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    failure,
                )),
            }
        }
        RemoteControlPairingPersistenceRequired::TargetAccess { attempt_id, .. } => {
            match settle_pairing_command(
                node,
                SettleRemoteControlControllerPairingPersistence {
                    attempt_id,
                    persistence: RemoteControlControllerPairingPersistence::Failed,
                },
            )
            .await
            {
                Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                    ..
                }) => Ok(()),
                Ok(RemoteControlControllerPairingFinalization::Completed { attempt_id, .. }) => {
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedControllerFinalization {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                        finalization: EmbeddedRemoteControlControllerPairingFinalization::Completed,
                    })
                }
                Err(failure) => Err(controller_settlement_failure(
                    attempt_id,
                    EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    failure,
                )),
            }
        }
    }
}

fn unexpected_target_finalization(
    operation: EmbeddedRemoteControlPairingPersistenceOperation,
    finalization: RemoteControlTargetPairingFinalization,
) -> EmbeddedRemoteControlPairingPersistenceFailure {
    let (attempt_id, finalization) = match finalization {
        RemoteControlTargetPairingFinalization::CompletionDispatched { attempt_id } => (
            attempt_id,
            EmbeddedRemoteControlTargetPairingFinalization::CompletionDispatched,
        ),
        RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired {
            attempt_id,
            ..
        } => (
            attempt_id,
            EmbeddedRemoteControlTargetPairingFinalization::AuthorizationRollbackRequired,
        ),
        RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
            attempt_id, ..
        } => (
            attempt_id,
            EmbeddedRemoteControlTargetPairingFinalization::AuthorizationFailureRecorded,
        ),
    };
    EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedTargetFinalization {
        attempt_id,
        operation,
        finalization,
    }
}

enum RemoteControlPairingSettlementFailure<F> {
    Failed(F),
    Busy,
    NodeStopped,
}

#[inline(never)]
async fn settle_pairing_command<
    C,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    command: C,
) -> Result<C::Success, RemoteControlPairingSettlementFailure<C::Failure>>
where
    C: Settleable + Copy,
    M: RawMutex,
{
    match node.settle_pairing_command(command).await {
        Ok(success) => Ok(success),
        Err(RemoteControlPairingControlError::Failed(failure)) => {
            Err(RemoteControlPairingSettlementFailure::Failed(failure))
        }
        Err(RemoteControlPairingControlError::Busy) => {
            Err(RemoteControlPairingSettlementFailure::Busy)
        }
        Err(RemoteControlPairingControlError::NodeStopped) => {
            Err(RemoteControlPairingSettlementFailure::NodeStopped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        EngineState, InstantMillis, IssuedCommand, Journaled, PrnsCommand,
        RemoteControlTargetPairingAuthorizationPersistence, RemoteControlTargetPairingFinalization,
        Settlement,
    };
    use crate::persistence::read_remote_control_controller_grants_snapshot;
    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind,
        RemoteControlTargetPairingResponder,
    };
    use crate::routing::links::request::RequestId;
    use crate::routing::links::LinkId;
    use crate::runtime::CompletionPool;
    use crate::storage::GrowableHeap;
    use embassy_futures::join::join;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("Remote Control fits growable storage")
    }

    struct ScriptedPersistence {
        deadline: Option<InstantMillis>,
        fail_store_attempt: Option<u8>,
        retry_at: Option<InstantMillis>,
        compaction_steps: u8,
        store_attempts: u8,
        progress_calls: u8,
        observed_failure: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
    }

    impl ManifoldPersistence<GrowableHeap> for ScriptedPersistence {
        fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

        fn deadline(&mut self, _now: InstantMillis) -> Option<InstantMillis> {
            self.deadline
        }

        fn observe_remote_control_pairing_failure(
            &mut self,
            failure: EmbeddedRemoteControlPairingPersistenceFailure,
        ) {
            self.observed_failure = Some(failure);
        }

        async fn progress(&mut self, _engine: &mut EngineState<GrowableHeap>, _now: InstantMillis) {
            self.progress_calls += 1;
            self.deadline = None;
        }

        async fn store_remote_control_authorization_snapshot(
            &mut self,
            _engine: &EngineState<GrowableHeap>,
            _kind: RemoteControlAuthorizationSnapshotKind,
            _snapshot: &RemoteControlAuthorizationSnapshot,
            now: InstantMillis,
        ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
            self.store_attempts = self.store_attempts.saturating_add(1);
            if self.fail_store_attempt.is_some_and(|attempt| {
                self.store_attempts == attempt
                    || (self.store_attempts > attempt
                        && self.retry_at.is_some_and(|retry| now.0 < retry.0))
            }) {
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed {
                    failure: EmbeddedPersistenceFailure::Flash,
                    retry_at: self.retry_at,
                }
            } else if self.compaction_steps > 0 {
                self.compaction_steps -= 1;
                StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress
            } else {
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored
            }
        }
    }

    #[test]
    fn pending_initial_store_preempts_a_future_deadline_and_returns_the_exact_failure() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(60_000)),
                fail_store_attempt: Some(1),
                retry_at: Some(InstantMillis(60_000)),
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            let submit = stores.store(
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Initial,
            );
            let drive = async {
                manifold.wait_for_work().await;
                assert_eq!(manifold.deadline(InstantMillis(7)), Some(InstantMillis(7)));
                manifold.progress(&mut engine, InstantMillis(7)).await;
            };

            let (result, ()) = join(submit, drive).await;
            assert_eq!(result, Err(EmbeddedPersistenceFailure::Flash));
            drop(manifold);
            assert_eq!(persistence.store_attempts, 1);
        });
    }

    #[test]
    fn rollback_store_holds_completion_until_durability_recovers() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(60_000)),
                fail_store_attempt: Some(1),
                retry_at: Some(InstantMillis(9)),
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            let submit = stores.store(
                RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            let drive = async {
                manifold.wait_for_work().await;
                assert_eq!(manifold.deadline(InstantMillis(8)), Some(InstantMillis(8)));
                manifold.progress(&mut engine, InstantMillis(8)).await;
                assert_eq!(manifold.deadline(InstantMillis(9)), Some(InstantMillis(9)));
                manifold.progress(&mut engine, InstantMillis(9)).await;
            };

            let (result, ()) = join(submit, drive).await;
            assert_eq!(result, Ok(()));
            drop(manifold);
            assert_eq!(persistence.store_attempts, 2);
        });
    }

    #[test]
    fn pending_pairing_failure_preempts_a_future_deadline_and_preserves_its_exact_cause() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(60_000)),
                fail_store_attempt: None,
                retry_at: None,
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            let failure = EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                attempt_id: super::super::node_facade::test_remote_control_pairing_attempt(0x91),
                operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
            };
            let report = stores.report_failure(failure);
            let drive = async {
                manifold.wait_for_work().await;
                assert_eq!(
                    manifold.deadline(InstantMillis(10)),
                    Some(InstantMillis(10))
                );
                manifold.progress(&mut engine, InstantMillis(10)).await;
            };

            let ((), ()) = join(report, drive).await;
            drop(manifold);
            assert_eq!(persistence.observed_failure, Some(failure));
        });
    }

    #[test]
    fn failure_readiness_preserves_the_bounded_queue_until_progress() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(60_000)),
                fail_store_attempt: None,
                retry_at: None,
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            let first = EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                attempt_id: super::super::node_facade::test_remote_control_pairing_attempt(0x91),
                operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
            };
            let second = EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                attempt_id: super::super::node_facade::test_remote_control_pairing_attempt(0x92),
                operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
            };
            stores.report_failure(first).await;
            for now in [InstantMillis(10), InstantMillis(11)] {
                manifold.wait_for_work().await;
                assert_eq!(manifold.deadline(now), Some(now));
                assert!(stores.failures.is_full());
            }

            let report_second = stores.report_failure(second);
            let drive = async {
                manifold.progress(&mut engine, InstantMillis(11)).await;
                assert_eq!(manifold.persistence.observed_failure, Some(first));
                manifold.wait_for_work().await;
                assert_eq!(
                    manifold.deadline(InstantMillis(12)),
                    Some(InstantMillis(12))
                );
                manifold.progress(&mut engine, InstantMillis(12)).await;
                assert_eq!(manifold.persistence.observed_failure, Some(second));
            };
            let ((), ()) = join(report_second, drive).await;
            assert!(!stores.has_failure());
            assert_eq!(
                manifold.deadline(InstantMillis(13)),
                Some(InstantMillis(60_000))
            );
            assert_eq!(manifold.persistence.progress_calls, 0);
        });
    }

    #[test]
    fn failed_initial_store_restores_and_durably_persists_the_prior_authorization() {
        embassy_futures::block_on(async {
            type M = CriticalSectionRawMutex;
            let commands = Channel::<M, IssuedCommand, 1>::new();
            let completions = CompletionPool::<M, 0>::new();
            let handle = PrnsNodeHandle::new(commands.sender(), &completions);
            let stores = RemoteControlAuthorizationStoreExchange::<M>::new();
            let mut remote_control = remote_control();
            let mut authorization = RemoteControlPairingAuthorizationTransactionState::new();
            let mut progress = RemoteControlPairingPersistenceProgress::new();
            let attempt_id = super::super::node_facade::test_remote_control_pairing_attempt(0x92);
            let grant = super::super::node_facade::test_remote_control_grant(
                RemoteControlRequestKind::Describe,
            );
            let required =
                RemoteControlPairingPersistenceRequired::ControllerGrant { attempt_id, grant };

            assert_eq!(
                progress
                    .accept_required(
                        required,
                        &mut remote_control,
                        &mut authorization,
                        Some(&stores),
                        handle,
                    )
                    .await,
                Ok(()),
            );
            let initial = stores.try_take_request().expect("initial store request");
            assert_eq!(
                initial.requirement,
                RemoteControlAuthorizationStoreRequirement::Initial,
            );
            assert_eq!(
                read_remote_control_controller_grants_snapshot(&initial.snapshot)
                    .unwrap()
                    .collect::<std::vec::Vec<_>>(),
                vec![grant],
            );
            assert_eq!(
                remote_control
                    .controller_grants()
                    .unwrap()
                    .grants_in_identity_hash_order(),
                &[],
            );

            let fail_store = progress.accept_store_completion(
                Err(EmbeddedPersistenceFailure::Flash),
                &mut remote_control,
                &mut authorization,
                &stores,
                handle,
            );
            let settle_failure = async {
                let issued = commands.receiver().receive().await;
                assert_eq!(
                    issued.command,
                    PrnsCommand::SettleRemoteControlTargetPairingAuthorization(
                        SettleRemoteControlTargetPairingAuthorization {
                            attempt_id,
                            persistence: RemoteControlTargetPairingAuthorizationPersistence::Failed,
                        },
                    ),
                );
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::SettleRemoteControlTargetPairingAuthorization(Ok(
                            RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
                                attempt_id,
                                retired_link: LinkId::new([0x93; 16]),
                                responder: RemoteControlTargetPairingResponder::new(
                                    LinkId::new([0x93; 16]),
                                    RequestId([0x94; 16]),
                                ),
                            },
                        )),
                    },
                    |_| panic!("pairing settlement should route to its awaiter"),
                );
            };
            let (failed, ()) = join(fail_store, settle_failure).await;
            assert_eq!(failed, Ok(()));
            assert!(remote_control.controller_grants().unwrap().is_empty());
            assert!(progress.is_waiting_for_store());

            let rollback = stores.try_take_request().expect("rollback store request");
            assert_eq!(
                rollback.requirement,
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            assert!(
                read_remote_control_controller_grants_snapshot(&rollback.snapshot)
                    .unwrap()
                    .next()
                    .is_none()
            );

            assert_eq!(
                progress
                    .accept_store_completion(
                        Ok(()),
                        &mut remote_control,
                        &mut authorization,
                        &stores,
                        handle,
                    )
                    .await,
                Err(EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                    attempt_id,
                    operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreAuthorization,
                    failure: EmbeddedPersistenceFailure::Flash,
                }),
            );
            assert!(progress.is_ready());
        });
    }

    #[test]
    fn failed_rollback_honors_store_backoff_without_delaying_failure_reports() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                // An unrelated batching deadline must not postpone the store's retry.
                deadline: Some(InstantMillis(80_000)),
                fail_store_attempt: Some(1),
                retry_at: Some(InstantMillis(60_000)),
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            stores.submit(
                RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            assert_eq!(manifold.deadline(InstantMillis(8)), Some(InstantMillis(8)));
            manifold.progress(&mut engine, InstantMillis(8)).await;
            // Bound the former driver hot loop and count calls, not just deadlines.
            for _ in 0..32 {
                if manifold
                    .deadline(InstantMillis(9))
                    .is_some_and(|due| due.0 <= 9)
                {
                    manifold.progress(&mut engine, InstantMillis(9)).await;
                }
            }
            assert_eq!(manifold.persistence.store_attempts, 1);
            assert_eq!(
                manifold.deadline(InstantMillis(9)),
                Some(InstantMillis(60_000))
            );
            assert_eq!(stores.completed.try_take(), None);

            let failure = EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                attempt_id: super::super::node_facade::test_remote_control_pairing_attempt(0x92),
                operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
            };
            stores.report_failure(failure).await;
            assert_eq!(
                manifold.deadline(InstantMillis(10)),
                Some(InstantMillis(10))
            );
            manifold.progress(&mut engine, InstantMillis(10)).await;
            assert_eq!(manifold.persistence.observed_failure, Some(failure));
            assert_eq!(manifold.persistence.store_attempts, 1);
            assert_eq!(
                manifold.deadline(InstantMillis(59_999)),
                Some(InstantMillis(60_000))
            );

            manifold.progress(&mut engine, InstantMillis(60_000)).await;
            assert_eq!(manifold.persistence.store_attempts, 2);
            assert_eq!(stores.completed.try_take(), Some(Ok(())));
            assert!(manifold.pending_request.is_none());
        });
    }

    #[test]
    fn unrelated_persistence_can_progress_without_retrying_rollback_early() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(12)),
                fail_store_attempt: Some(1),
                retry_at: Some(InstantMillis(60_000)),
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            stores.submit(
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            assert_eq!(manifold.deadline(InstantMillis(8)), Some(InstantMillis(8)));
            manifold.progress(&mut engine, InstantMillis(8)).await;
            assert_eq!(manifold.deadline(InstantMillis(9)), Some(InstantMillis(12)));
            manifold.progress(&mut engine, InstantMillis(12)).await;
            assert_eq!(manifold.persistence.progress_calls, 1);
            assert_eq!(manifold.persistence.store_attempts, 1);
            assert_eq!(
                manifold.deadline(InstantMillis(12)),
                Some(InstantMillis(60_000))
            );
            assert_eq!(stores.completed.try_take(), None);
            manifold.progress(&mut engine, InstantMillis(60_000)).await;
            assert_eq!(stores.completed.try_take(), Some(Ok(())));
        });
    }

    #[test]
    fn unavailable_rollback_store_does_not_invent_a_retry_or_report_success() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: None,
                fail_store_attempt: Some(1),
                retry_at: None,
                compaction_steps: 0,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            stores.submit(
                RemoteControlAuthorizationSnapshotKind::TargetAccesses,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            assert_eq!(manifold.deadline(InstantMillis(8)), Some(InstantMillis(8)));
            manifold.progress(&mut engine, InstantMillis(8)).await;
            assert_eq!(manifold.deadline(InstantMillis(9)), None);
            // Even a wake for unrelated work must not retry an unavailable store.
            manifold.progress(&mut engine, InstantMillis(60_000)).await;
            assert_eq!(manifold.persistence.store_attempts, 1);
            assert_eq!(stores.completed.try_take(), None);
            assert!(manifold.pending_request.is_some());
        });
    }

    #[test]
    fn compaction_steps_remain_immediately_runnable_until_the_store_completes() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(60_000)),
                fail_store_attempt: None,
                retry_at: None,
                compaction_steps: 2,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            stores.submit(
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Initial,
            );
            for now in 8..10 {
                assert!(manifold
                    .deadline(InstantMillis(now))
                    .is_some_and(|due| due.0 <= now));
                manifold.progress(&mut engine, InstantMillis(now)).await;
                assert_eq!(stores.completed.try_take(), None);
            }
            assert!(manifold
                .deadline(InstantMillis(10))
                .is_some_and(|due| due.0 <= 10));
            manifold.progress(&mut engine, InstantMillis(10)).await;
            assert_eq!(manifold.persistence.store_attempts, 3);
            assert_eq!(stores.completed.try_take(), Some(Ok(())));
        });
    }

    #[test]
    fn failed_compaction_enters_rollback_cooldown_on_the_next_store_outcome() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                deadline: Some(InstantMillis(80_000)),
                fail_store_attempt: Some(2),
                retry_at: Some(InstantMillis(60_000)),
                compaction_steps: 1,
                store_attempts: 0,
                progress_calls: 0,
                observed_failure: None,
            };
            let mut manifold =
                RemoteControlPairingManifoldPersistence::new(&mut persistence, &stores);
            let mut engine = EngineState::<GrowableHeap>::default();
            stores.submit(
                RemoteControlAuthorizationSnapshotKind::ControllerGrants,
                RemoteControlAuthorizationSnapshot::new(),
                RemoteControlAuthorizationStoreRequirement::Rollback,
            );
            assert_eq!(manifold.deadline(InstantMillis(8)), Some(InstantMillis(8)));
            manifold.progress(&mut engine, InstantMillis(8)).await;
            assert!(manifold
                .deadline(InstantMillis(9))
                .is_some_and(|due| due.0 <= 9));
            manifold.progress(&mut engine, InstantMillis(9)).await;
            for _ in 0..32 {
                if manifold
                    .deadline(InstantMillis(10))
                    .is_some_and(|due| due.0 <= 10)
                {
                    manifold.progress(&mut engine, InstantMillis(10)).await;
                }
            }
            assert_eq!(manifold.persistence.store_attempts, 2);
            assert_eq!(
                manifold.deadline(InstantMillis(10)),
                Some(InstantMillis(60_000))
            );
            assert_eq!(stores.completed.try_take(), None);
            manifold.progress(&mut engine, InstantMillis(60_000)).await;
            assert_eq!(manifold.persistence.store_attempts, 3);
            assert_eq!(stores.completed.try_take(), Some(Ok(())));
        });
    }
}
