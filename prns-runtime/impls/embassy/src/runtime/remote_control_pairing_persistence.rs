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
    RemoteControlControllerGrant, RemoteControlPairingAttemptId, RemoteControlRequestSet,
};
use crate::storage::StorageLayout;

use super::embedded_persistence::{
    EmbeddedPersistenceFailure, ManifoldPersistence, RemoteControlAuthorizationSnapshot,
    RemoteControlAuthorizationSnapshotKind, StoreRemoteControlAuthorizationSnapshotOutcome,
};
use super::node_facade::PrnsNodeHandle;
use super::remote_control_pairing_authorizations::{
    RemoteControlPairingAuthorization, RemoteControlPairingAuthorizationTransactionFailure,
};
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
        operation: EmbeddedRemoteControlPairingPersistenceOperation,
        finalization: RemoteControlTargetPairingFinalization,
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
                permitted_requests,
            } => {
                self.target_access
                    .signal(RemoteControlTargetAccessPersistenceRequired {
                        attempt_id,
                        target_public_keys,
                        permitted_requests,
                    });
            }
        }
    }

    async fn receive(&self) -> RemoteControlPairingPersistenceRequired {
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
                permitted_requests,
                ..
            } => RemoteControlPairingAuthorization::TargetAccess {
                target_public_keys: *target_public_keys,
                permitted_requests: *permitted_requests,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlAuthorizationStoreRequirement {
    Initial,
    Rollback,
}

struct RemoteControlAuthorizationStoreRequest {
    kind: RemoteControlAuthorizationSnapshotKind,
    snapshot: RemoteControlAuthorizationSnapshot,
    requirement: RemoteControlAuthorizationStoreRequirement,
}

pub(super) struct RemoteControlAuthorizationStoreExchange<M: RawMutex> {
    requests: Channel<M, RemoteControlAuthorizationStoreRequest, 1>,
    failures: Channel<M, EmbeddedRemoteControlPairingPersistenceFailure, 1>,
    completed: Signal<M, Result<(), EmbeddedPersistenceFailure>>,
}

impl<M: RawMutex> RemoteControlAuthorizationStoreExchange<M> {
    pub(super) const fn new() -> Self {
        Self {
            requests: Channel::new(),
            failures: Channel::new(),
            completed: Signal::new(),
        }
    }

    async fn store(
        &self,
        kind: RemoteControlAuthorizationSnapshotKind,
        snapshot: RemoteControlAuthorizationSnapshot,
        requirement: RemoteControlAuthorizationStoreRequirement,
    ) -> Result<(), EmbeddedPersistenceFailure> {
        self.completed.reset();
        self.requests
            .send(RemoteControlAuthorizationStoreRequest {
                kind,
                snapshot,
                requirement,
            })
            .await;
        self.completed.wait().await
    }

    async fn report_failure(&self, failure: EmbeddedRemoteControlPairingPersistenceFailure) {
        self.failures.send(failure).await;
    }

    fn try_take_request(&self) -> Option<RemoteControlAuthorizationStoreRequest> {
        self.requests.try_receive().ok()
    }

    fn try_take_failure(&self) -> Option<EmbeddedRemoteControlPairingPersistenceFailure> {
        self.failures.try_receive().ok()
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
    pending_request: Option<RemoteControlAuthorizationStoreRequest>,
    pending_failure: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
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
            pending_failure: None,
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
        if self.pending_failure.is_none() {
            self.pending_failure = self.stores.try_take_failure();
        }
        if self.pending_request.is_none() {
            self.pending_request = self.stores.try_take_request();
        }
        if self.pending_failure.is_some() || self.pending_request.is_some() {
            self.persistence.deadline(now).or(Some(now))
        } else {
            self.persistence.deadline(now)
        }
    }

    async fn wait_for_work(&self) {
        embassy_futures::select::select(
            self.stores.requests.ready_to_receive(),
            self.stores.failures.ready_to_receive(),
        )
        .await;
    }

    async fn progress(
        &mut self,
        engine: &mut crate::engine::EngineState<S>,
        now: crate::engine::InstantMillis,
    ) {
        if let Some(failure) = self.pending_failure.take() {
            self.persistence
                .observe_remote_control_pairing_failure(failure);
            return;
        }
        let Some(request) = self.pending_request.as_ref() else {
            self.persistence.progress(engine, now).await;
            return;
        };
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
            StoreRemoteControlAuthorizationSnapshotOutcome::CompactionInProgress => {}
            StoreRemoteControlAuthorizationSnapshotOutcome::Failed(failure) => {
                match request.requirement {
                    RemoteControlAuthorizationStoreRequirement::Initial => {
                        self.pending_request = None;
                        self.stores.settle(Err(failure));
                    }
                    RemoteControlAuthorizationStoreRequirement::Rollback => {}
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

pub(super) async fn run_remote_control_pairing_persistence<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    events: &RemoteControlPairingPersistenceEvents<M>,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    M: RawMutex,
{
    loop {
        let required = events.receive().await;
        match stores {
            Some(stores) => {
                if let Err(failure) = persist_remote_control_pairing(required, stores, node).await {
                    stores.report_failure(failure).await;
                }
            }
            None => {
                if let Err(EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped { .. }) =
                    settle_persistence_failure(required, node).await
                {
                    return;
                }
            }
        }
    }
}

async fn persist_remote_control_pairing<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    required: RemoteControlPairingPersistenceRequired,
    stores: &RemoteControlAuthorizationStoreExchange<M>,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
where
    M: RawMutex,
{
    let attempt_id = required.attempt_id();
    let projected = match node
        .prepare_remote_control_pairing_authorization(attempt_id, required.authorization())
        .await
    {
        Ok(prepared) => prepared,
        Err(failure) => {
            stores
                .report_failure(
                    EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::PrepareAuthorization,
                        failure,
                    },
                )
                .await;
            return settle_persistence_failure(required, node).await;
        }
    };
    let rollback = match node
        .snapshot_remote_control_pairing_authorization_rollback(attempt_id)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            stores
                .report_failure(
                    EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                        attempt_id,
                        operation:
                            EmbeddedRemoteControlPairingPersistenceOperation::SnapshotRollback,
                        failure,
                    },
                )
                .await;
            release_authorization(attempt_id, node).await?;
            return settle_persistence_failure(required, node).await;
        }
    };
    if let Err(failure) = stores
        .store(
            required.snapshot_kind(),
            projected,
            RemoteControlAuthorizationStoreRequirement::Initial,
        )
        .await
    {
        stores
            .report_failure(EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreAuthorization,
                failure,
            })
            .await;
        release_authorization(attempt_id, node).await?;
        return settle_persistence_failure(required, node).await;
    }
    if let Err(failure) = node
        .activate_remote_control_pairing_authorization(attempt_id)
        .await
    {
        stores
            .report_failure(
                EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                    attempt_id,
                    operation:
                        EmbeddedRemoteControlPairingPersistenceOperation::ActivateAuthorization,
                    failure,
                },
            )
            .await;
        roll_back_durably(required.snapshot_kind(), attempt_id, rollback, stores, node).await?;
        return settle_persistence_failure(required, node).await;
    }
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
                    release_authorization(attempt_id, node).await
                }
                Ok(
                    RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired {
                        ..
                    },
                ) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await
                }
                Ok(finalization @ RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded { .. }) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedTargetFinalization {
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        finalization,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::Failed(failure)) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::TargetSettlement {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        failure,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::NodeStopped) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::Busy) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(
                        EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                            attempt_id,
                            operation:
                                EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        },
                    )
                }
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
                    release_authorization(attempt_id, node).await
                }
                Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded {
                    attempt_id,
                    ..
                }) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedControllerFinalization {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        finalization: EmbeddedRemoteControlControllerPairingFinalization::PersistenceFailureRecorded,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::Failed(failure)) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::ControllerSettlement {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        failure,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::NodeStopped) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                    })
                }
                Err(RemoteControlPairingSettlementFailure::Busy) => {
                    roll_back_durably(
                        required.snapshot_kind(),
                        attempt_id,
                        rollback,
                        stores,
                        node,
                    )
                    .await?;
                    Err(
                        EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                            attempt_id,
                            operation:
                                EmbeddedRemoteControlPairingPersistenceOperation::SettlePersisted,
                        },
                    )
                }
            }
        }
    }
}

async fn roll_back_durably<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    kind: RemoteControlAuthorizationSnapshotKind,
    attempt_id: RemoteControlPairingAttemptId,
    expected_snapshot: RemoteControlAuthorizationSnapshot,
    stores: &RemoteControlAuthorizationStoreExchange<M>,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
where
    M: RawMutex,
{
    let live_rollback = node
        .roll_back_remote_control_pairing_authorization(attempt_id)
        .await;
    let live_rollback_failure = match live_rollback {
        Ok(snapshot) if snapshot == expected_snapshot => None,
        Ok(_) => Some(
            EmbeddedRemoteControlPairingPersistenceFailure::RollbackSnapshotMismatch { attempt_id },
        ),
        Err(failure) => Some(
            EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::RollBackAuthorization,
                failure,
            },
        ),
    };
    stores
        .store(
            kind,
            expected_snapshot,
            RemoteControlAuthorizationStoreRequirement::Rollback,
        )
        .await
        .map_err(
            |failure| EmbeddedRemoteControlPairingPersistenceFailure::Storage {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::StoreRollback,
                failure,
            },
        )?;
    if let Some(failure) = live_rollback_failure {
        return Err(failure);
    }
    release_authorization(attempt_id, node).await
}

async fn release_authorization<
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
>(
    attempt_id: RemoteControlPairingAttemptId,
    node: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) -> Result<(), EmbeddedRemoteControlPairingPersistenceFailure>
where
    M: RawMutex,
{
    node.release_remote_control_pairing_authorization(attempt_id)
        .await
        .map_err(|failure| {
            EmbeddedRemoteControlPairingPersistenceFailure::AuthorizationTransaction {
                attempt_id,
                operation: EmbeddedRemoteControlPairingPersistenceOperation::ReleaseAuthorization,
                failure,
            }
        })
}

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
                }) => Ok(()),
                Ok(finalization) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::UnexpectedTargetFinalization {
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                        finalization,
                    },
                ),
                Err(RemoteControlPairingSettlementFailure::Failed(failure)) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::TargetSettlement {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                        failure,
                    },
                ),
                Err(RemoteControlPairingSettlementFailure::NodeStopped) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    },
                ),
                Err(RemoteControlPairingSettlementFailure::Busy) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    },
                ),
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
                Err(RemoteControlPairingSettlementFailure::Failed(failure)) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::ControllerSettlement {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                        failure,
                    },
                ),
                Err(RemoteControlPairingSettlementFailure::NodeStopped) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::NodeStopped {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    },
                ),
                Err(RemoteControlPairingSettlementFailure::Busy) => Err(
                    EmbeddedRemoteControlPairingPersistenceFailure::SettlementBusy {
                        attempt_id,
                        operation: EmbeddedRemoteControlPairingPersistenceOperation::SettleFailed,
                    },
                ),
            }
        }
    }
}

enum RemoteControlPairingSettlementFailure<F> {
    Failed(F),
    Busy,
    NodeStopped,
}

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
    use crate::engine::{EngineState, InstantMillis};
    use crate::storage::GrowableHeap;
    use embassy_futures::join::join;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    struct ScriptedPersistence {
        fail_first_store: bool,
        store_attempts: u8,
        observed_failure: Option<EmbeddedRemoteControlPairingPersistenceFailure>,
    }

    impl ManifoldPersistence<GrowableHeap> for ScriptedPersistence {
        fn observe(&mut self, _journaled: &Journaled<'_>, _now: InstantMillis) {}

        fn deadline(&mut self, _now: InstantMillis) -> Option<InstantMillis> {
            None
        }

        fn observe_remote_control_pairing_failure(
            &mut self,
            failure: EmbeddedRemoteControlPairingPersistenceFailure,
        ) {
            self.observed_failure = Some(failure);
        }

        async fn progress(&mut self, _engine: &mut EngineState<GrowableHeap>, _now: InstantMillis) {
        }

        async fn store_remote_control_authorization_snapshot(
            &mut self,
            _engine: &EngineState<GrowableHeap>,
            _kind: RemoteControlAuthorizationSnapshotKind,
            _snapshot: &RemoteControlAuthorizationSnapshot,
            _now: InstantMillis,
        ) -> StoreRemoteControlAuthorizationSnapshotOutcome {
            self.store_attempts = self.store_attempts.saturating_add(1);
            if self.fail_first_store && self.store_attempts == 1 {
                StoreRemoteControlAuthorizationSnapshotOutcome::Failed(
                    EmbeddedPersistenceFailure::Flash,
                )
            } else {
                StoreRemoteControlAuthorizationSnapshotOutcome::Stored
            }
        }
    }

    #[test]
    fn initial_store_wakes_the_manifold_and_returns_the_exact_failure() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                fail_first_store: true,
                store_attempts: 0,
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
                fail_first_store: true,
                store_attempts: 0,
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
    fn pairing_failure_wakes_the_manifold_and_preserves_its_exact_cause() {
        embassy_futures::block_on(async {
            let stores = RemoteControlAuthorizationStoreExchange::<CriticalSectionRawMutex>::new();
            let mut persistence = ScriptedPersistence {
                fail_first_store: false,
                store_attempts: 0,
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
}
