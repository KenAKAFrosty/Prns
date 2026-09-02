use crate::identity::IdentityPublicKeys;
use crate::persistence::SnapshotSealError;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerGrant, RemoteControlPairingAttemptId,
    RemoteControlRequestSet, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
    SetRemoteControlTargetAccessOutcome,
};

use super::embedded_persistence::RemoteControlAuthorizationSnapshot;
use super::AssembledRemoteControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteControlPairingAuthorization {
    ControllerGrant(RemoteControlControllerGrant),
    TargetAccess {
        target_public_keys: IdentityPublicKeys,
        permitted_requests: RemoteControlRequestSet,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlTargetAccessSpec {
    Access {
        target_public_keys: IdentityPublicKeys,
        permitted_requests: RemoteControlRequestSet,
    },
}

impl RemoteControlTargetAccessSpec {
    fn from_access(access: &RemoteControlTargetAccess) -> Self {
        Self::Access {
            target_public_keys: *access.target().public_keys(),
            permitted_requests: *access.permitted_requests(),
        }
    }

    fn into_access(self) -> Result<RemoteControlTargetAccess, ()> {
        match self {
            Self::Access {
                target_public_keys,
                permitted_requests,
            } => RemoteControlTargetAccess::new(
                RemoteControlTargetIdentity::new(target_public_keys),
                permitted_requests,
            )
            .map_err(|_| ()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlAuthorizationMutation {
    ControllerAdded {
        desired: RemoteControlControllerGrant,
    },
    ControllerUnchanged,
    ControllerUpdated {
        desired: RemoteControlControllerGrant,
        previous: RemoteControlControllerGrant,
    },
    TargetAdded {
        desired: RemoteControlTargetAccessSpec,
    },
    TargetUnchanged,
    TargetUpdated {
        desired: RemoteControlTargetAccessSpec,
        previous: RemoteControlTargetAccessSpec,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RemoteControlPairingAuthorizationTransaction {
    attempt_id: RemoteControlPairingAttemptId,
    mutation: RemoteControlAuthorizationMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteControlPairingAuthorizationTransactionState {
    Available,
    Prepared(RemoteControlPairingAuthorizationTransaction),
    Activated(RemoteControlPairingAuthorizationTransaction),
    RolledBack(RemoteControlPairingAuthorizationTransaction),
}

impl RemoteControlPairingAuthorizationTransactionState {
    pub(super) const fn new() -> Self {
        Self::Available
    }

    pub(super) const fn is_active(&self) -> bool {
        !matches!(self, Self::Available)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAuthorizationTransactionFailure {
    TransactionInProgress {
        active: RemoteControlPairingAttemptId,
    },
    NoTransaction,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    Unavailable,
    CapacityExhausted,
    Snapshot(SnapshotSealError),
    RuntimeState,
}

pub(super) fn snapshot_rollback(
    remote_control: &AssembledRemoteControl,
    state: &RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<RemoteControlAuthorizationSnapshot, RemoteControlPairingAuthorizationTransactionFailure>
{
    let transaction = prepared_transaction(state, attempt_id)?;
    authorization_snapshot(remote_control, transaction.mutation)
}

pub(super) fn prepare(
    remote_control: &mut AssembledRemoteControl,
    state: &mut RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
    authorization: RemoteControlPairingAuthorization,
) -> Result<RemoteControlAuthorizationSnapshot, RemoteControlPairingAuthorizationTransactionFailure>
{
    if let Some(active) = active_attempt(state) {
        return Err(
            RemoteControlPairingAuthorizationTransactionFailure::TransactionInProgress { active },
        );
    }
    let mutation = apply_authorization(remote_control, authorization)?;
    let snapshot = match authorization_snapshot(remote_control, mutation) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            reverse_authorization(remote_control, mutation)?;
            return Err(error);
        }
    };
    reverse_authorization(remote_control, mutation)?;
    *state = RemoteControlPairingAuthorizationTransactionState::Prepared(
        RemoteControlPairingAuthorizationTransaction {
            attempt_id,
            mutation,
        },
    );
    Ok(snapshot)
}

pub(super) fn activate(
    remote_control: &mut AssembledRemoteControl,
    state: &mut RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<(), RemoteControlPairingAuthorizationTransactionFailure> {
    let transaction = prepared_transaction(state, attempt_id)?;
    if matches!(
        transaction.mutation,
        RemoteControlAuthorizationMutation::ControllerUnchanged
            | RemoteControlAuthorizationMutation::TargetUnchanged
    ) {
        *state = RemoteControlPairingAuthorizationTransactionState::Activated(transaction);
        return Ok(());
    }
    let authorization = authorization_from_mutation(transaction.mutation)?;
    let applied = apply_authorization(remote_control, authorization)?;
    if applied != transaction.mutation {
        reverse_authorization(remote_control, applied)?;
        return Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState);
    }
    *state = RemoteControlPairingAuthorizationTransactionState::Activated(transaction);
    Ok(())
}

pub(super) fn roll_back(
    remote_control: &mut AssembledRemoteControl,
    state: &mut RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<RemoteControlAuthorizationSnapshot, RemoteControlPairingAuthorizationTransactionFailure>
{
    let transaction = match state {
        RemoteControlPairingAuthorizationTransactionState::Prepared(transaction)
            if transaction.attempt_id == attempt_id =>
        {
            let transaction = *transaction;
            *state = RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction);
            transaction
        }
        RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction)
            if transaction.attempt_id == attempt_id =>
        {
            *transaction
        }
        _ => {
            let transaction = activated_transaction(state, attempt_id)?;
            reverse_authorization(remote_control, transaction.mutation)?;
            *state = RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction);
            transaction
        }
    };
    authorization_snapshot(remote_control, transaction.mutation)
}

pub(super) fn release(
    state: &mut RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<(), RemoteControlPairingAuthorizationTransactionFailure> {
    let active = active_attempt(state)
        .ok_or(RemoteControlPairingAuthorizationTransactionFailure::NoTransaction)?;
    if active != attempt_id {
        return Err(
            RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested: attempt_id,
                active,
            },
        );
    }
    *state = RemoteControlPairingAuthorizationTransactionState::Available;
    Ok(())
}

fn active_attempt(
    state: &RemoteControlPairingAuthorizationTransactionState,
) -> Option<RemoteControlPairingAttemptId> {
    match state {
        RemoteControlPairingAuthorizationTransactionState::Available => None,
        RemoteControlPairingAuthorizationTransactionState::Prepared(transaction)
        | RemoteControlPairingAuthorizationTransactionState::Activated(transaction) => {
            Some(transaction.attempt_id)
        }
        RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction) => {
            Some(transaction.attempt_id)
        }
    }
}

fn prepared_transaction(
    state: &RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<
    RemoteControlPairingAuthorizationTransaction,
    RemoteControlPairingAuthorizationTransactionFailure,
> {
    match state {
        RemoteControlPairingAuthorizationTransactionState::Prepared(transaction)
            if transaction.attempt_id == attempt_id =>
        {
            Ok(*transaction)
        }
        RemoteControlPairingAuthorizationTransactionState::Prepared(transaction)
        | RemoteControlPairingAuthorizationTransactionState::Activated(transaction) => Err(
            RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested: attempt_id,
                active: transaction.attempt_id,
            },
        ),
        RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction) => Err(
            RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested: attempt_id,
                active: transaction.attempt_id,
            },
        ),
        RemoteControlPairingAuthorizationTransactionState::Available => {
            Err(RemoteControlPairingAuthorizationTransactionFailure::NoTransaction)
        }
    }
}

fn activated_transaction(
    state: &RemoteControlPairingAuthorizationTransactionState,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<
    RemoteControlPairingAuthorizationTransaction,
    RemoteControlPairingAuthorizationTransactionFailure,
> {
    match state {
        RemoteControlPairingAuthorizationTransactionState::Activated(transaction)
            if transaction.attempt_id == attempt_id =>
        {
            Ok(*transaction)
        }
        RemoteControlPairingAuthorizationTransactionState::Prepared(transaction)
        | RemoteControlPairingAuthorizationTransactionState::Activated(transaction) => Err(
            RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested: attempt_id,
                active: transaction.attempt_id,
            },
        ),
        RemoteControlPairingAuthorizationTransactionState::RolledBack(transaction) => Err(
            RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested: attempt_id,
                active: transaction.attempt_id,
            },
        ),
        RemoteControlPairingAuthorizationTransactionState::Available => {
            Err(RemoteControlPairingAuthorizationTransactionFailure::NoTransaction)
        }
    }
}

fn apply_authorization(
    remote_control: &mut AssembledRemoteControl,
    authorization: RemoteControlPairingAuthorization,
) -> Result<RemoteControlAuthorizationMutation, RemoteControlPairingAuthorizationTransactionFailure>
{
    match authorization {
        RemoteControlPairingAuthorization::ControllerGrant(grant) => {
            match remote_control.set_controller_grant(grant) {
                Ok(SetRemoteControlControllerGrantOutcome::Added) => {
                    Ok(RemoteControlAuthorizationMutation::ControllerAdded { desired: grant })
                }
                Ok(SetRemoteControlControllerGrantOutcome::Unchanged) => {
                    Ok(RemoteControlAuthorizationMutation::ControllerUnchanged)
                }
                Ok(SetRemoteControlControllerGrantOutcome::Updated { previous }) => {
                    Ok(RemoteControlAuthorizationMutation::ControllerUpdated {
                        desired: grant,
                        previous,
                    })
                }
                Err(super::SetRemoteControlControllerGrantServiceError::Unavailable) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::Unavailable)
                }
                Err(super::SetRemoteControlControllerGrantServiceError::CapacityExhausted) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::CapacityExhausted)
                }
                Err(super::SetRemoteControlControllerGrantServiceError::TransactionInProgress) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
                }
            }
        }
        RemoteControlPairingAuthorization::TargetAccess {
            target_public_keys,
            permitted_requests,
        } => {
            let desired = RemoteControlTargetAccessSpec::Access {
                target_public_keys,
                permitted_requests,
            };
            let access = desired
                .into_access()
                .map_err(|()| RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)?;
            match remote_control.set_target_access(access) {
                Ok(SetRemoteControlTargetAccessOutcome::Added) => {
                    Ok(RemoteControlAuthorizationMutation::TargetAdded { desired })
                }
                Ok(SetRemoteControlTargetAccessOutcome::Unchanged) => {
                    Ok(RemoteControlAuthorizationMutation::TargetUnchanged)
                }
                Ok(SetRemoteControlTargetAccessOutcome::Updated { previous }) => {
                    Ok(RemoteControlAuthorizationMutation::TargetUpdated {
                        desired,
                        previous: RemoteControlTargetAccessSpec::from_access(&previous),
                    })
                }
                Err(super::SetRemoteControlTargetAccessServiceError::Unavailable) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::Unavailable)
                }
                Err(super::SetRemoteControlTargetAccessServiceError::CapacityExhausted) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::CapacityExhausted)
                }
                Err(super::SetRemoteControlTargetAccessServiceError::TransactionInProgress) => {
                    Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
                }
            }
        }
    }
}

fn authorization_from_mutation(
    mutation: RemoteControlAuthorizationMutation,
) -> Result<RemoteControlPairingAuthorization, RemoteControlPairingAuthorizationTransactionFailure>
{
    match mutation {
        RemoteControlAuthorizationMutation::ControllerAdded { desired }
        | RemoteControlAuthorizationMutation::ControllerUpdated { desired, .. } => {
            Ok(RemoteControlPairingAuthorization::ControllerGrant(desired))
        }
        RemoteControlAuthorizationMutation::ControllerUnchanged
        | RemoteControlAuthorizationMutation::TargetUnchanged => {
            Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
        }
        RemoteControlAuthorizationMutation::TargetAdded { desired }
        | RemoteControlAuthorizationMutation::TargetUpdated { desired, .. } => match desired {
            RemoteControlTargetAccessSpec::Access {
                target_public_keys,
                permitted_requests,
            } => Ok(RemoteControlPairingAuthorization::TargetAccess {
                target_public_keys,
                permitted_requests,
            }),
        },
    }
}

fn reverse_authorization(
    remote_control: &mut AssembledRemoteControl,
    mutation: RemoteControlAuthorizationMutation,
) -> Result<(), RemoteControlPairingAuthorizationTransactionFailure> {
    match mutation {
        RemoteControlAuthorizationMutation::ControllerAdded { desired } => {
            match remote_control.revoke_controller(desired.controller()) {
                Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }) if grant == desired => {
                    Ok(())
                }
                Ok(RevokeRemoteControlControllerOutcome::Revoked { .. })
                | Ok(RevokeRemoteControlControllerOutcome::NotFound)
                | Err(
                    super::RevokeRemoteControlControllerServiceError::Unavailable
                    | super::RevokeRemoteControlControllerServiceError::TransactionInProgress,
                ) => Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState),
            }
        }
        RemoteControlAuthorizationMutation::ControllerUnchanged
        | RemoteControlAuthorizationMutation::TargetUnchanged => Ok(()),
        RemoteControlAuthorizationMutation::ControllerUpdated { desired, previous } => {
            match remote_control.set_controller_grant(previous) {
                Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: replaced })
                    if replaced == desired =>
                {
                    Ok(())
                }
                Ok(
                    SetRemoteControlControllerGrantOutcome::Added
                    | SetRemoteControlControllerGrantOutcome::Unchanged
                    | SetRemoteControlControllerGrantOutcome::Updated { .. },
                )
                | Err(
                    super::SetRemoteControlControllerGrantServiceError::Unavailable
                    | super::SetRemoteControlControllerGrantServiceError::CapacityExhausted
                    | super::SetRemoteControlControllerGrantServiceError::TransactionInProgress,
                ) => Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState),
            }
        }
        RemoteControlAuthorizationMutation::TargetAdded { desired } => {
            let access = desired
                .into_access()
                .map_err(|()| RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)?;
            match remote_control.forget_target(access.target()) {
                Ok(ForgetRemoteControlTargetOutcome::Forgotten { access: forgotten })
                    if RemoteControlTargetAccessSpec::from_access(&forgotten) == desired =>
                {
                    Ok(())
                }
                Ok(ForgetRemoteControlTargetOutcome::Forgotten { .. })
                | Ok(ForgetRemoteControlTargetOutcome::NotFound)
                | Err(
                    super::ForgetRemoteControlTargetServiceError::Unavailable
                    | super::ForgetRemoteControlTargetServiceError::TransactionInProgress,
                ) => Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState),
            }
        }
        RemoteControlAuthorizationMutation::TargetUpdated { desired, previous } => {
            let previous_access = previous
                .into_access()
                .map_err(|()| RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)?;
            match remote_control.set_target_access(previous_access) {
                Ok(SetRemoteControlTargetAccessOutcome::Updated { previous: replaced })
                    if RemoteControlTargetAccessSpec::from_access(&replaced) == desired =>
                {
                    Ok(())
                }
                Ok(
                    SetRemoteControlTargetAccessOutcome::Added
                    | SetRemoteControlTargetAccessOutcome::Unchanged
                    | SetRemoteControlTargetAccessOutcome::Updated { .. },
                )
                | Err(
                    super::SetRemoteControlTargetAccessServiceError::Unavailable
                    | super::SetRemoteControlTargetAccessServiceError::CapacityExhausted
                    | super::SetRemoteControlTargetAccessServiceError::TransactionInProgress,
                ) => Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState),
            }
        }
    }
}

fn authorization_snapshot(
    remote_control: &AssembledRemoteControl,
    mutation: RemoteControlAuthorizationMutation,
) -> Result<RemoteControlAuthorizationSnapshot, RemoteControlPairingAuthorizationTransactionFailure>
{
    let mut bytes =
        [0u8; super::embedded_persistence::REMOTE_CONTROL_AUTHORIZATION_SNAPSHOT_CAPACITY];
    let written = match mutation {
        RemoteControlAuthorizationMutation::ControllerAdded { .. }
        | RemoteControlAuthorizationMutation::ControllerUnchanged
        | RemoteControlAuthorizationMutation::ControllerUpdated { .. } => remote_control
            .write_controller_grants_snapshot(&mut bytes)
            .map_err(RemoteControlPairingAuthorizationTransactionFailure::Snapshot)?,
        RemoteControlAuthorizationMutation::TargetAdded { .. }
        | RemoteControlAuthorizationMutation::TargetUnchanged
        | RemoteControlAuthorizationMutation::TargetUpdated { .. } => remote_control
            .write_target_accesses_snapshot(&mut bytes)
            .map_err(RemoteControlPairingAuthorizationTransactionFailure::Snapshot)?,
    }
    .ok_or(RemoteControlPairingAuthorizationTransactionFailure::Unavailable)?;
    RemoteControlAuthorizationSnapshot::from_slice(&bytes[..written])
        .map_err(|_| RemoteControlPairingAuthorizationTransactionFailure::CapacityExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::IdentitySigner;
    use crate::persistence::{
        read_remote_control_controller_grants_snapshot,
        read_remote_control_target_accesses_snapshot,
    };
    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind, RemoteControlTargetAccessTable,
    };
    use crate::storage::GrowableHeap;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = crate::engine::EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    fn signer(fill: u8) -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ))
    }

    fn controller(fill: u8) -> crate::remote_control::RemoteControlControllerIdentity {
        let signer = signer(fill);
        crate::remote_control::RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn target(fill: u8) -> crate::remote_control::RemoteControlTargetIdentity {
        let signer = signer(fill);
        crate::remote_control::RemoteControlTargetIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn attempt(endpoint_fill: u8) -> RemoteControlPairingAttemptId {
        super::super::node_facade::test_remote_control_pairing_attempt(endpoint_fill)
    }

    #[test]
    fn controller_grant_prepare_is_inert_until_activation_and_rollback_is_idempotent() {
        let mut remote_control = remote_control();
        let mut state = RemoteControlPairingAuthorizationTransactionState::new();
        let attempt_id = attempt(0x73);
        let grant = RemoteControlControllerGrant::new(
            controller(0x44),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();

        let prepared = prepare(
            &mut remote_control,
            &mut state,
            attempt_id,
            RemoteControlPairingAuthorization::ControllerGrant(grant),
        )
        .unwrap();
        assert!(remote_control.controller_grants().unwrap().is_empty());
        assert_eq!(
            read_remote_control_controller_grants_snapshot(&prepared)
                .unwrap()
                .collect::<std::vec::Vec<_>>(),
            vec![grant],
        );
        let rollback = snapshot_rollback(&remote_control, &state, attempt_id).unwrap();
        assert!(read_remote_control_controller_grants_snapshot(&rollback)
            .unwrap()
            .next()
            .is_none());
        activate(&mut remote_control, &mut state, attempt_id).unwrap();
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[grant],
        );
        let first = roll_back(&mut remote_control, &mut state, attempt_id).unwrap();
        let second = roll_back(&mut remote_control, &mut state, attempt_id).unwrap();
        assert_eq!(first, second);
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }

    #[test]
    fn target_update_rollback_restores_the_exact_prior_access() {
        let mut remote_control = remote_control();
        let target_public_keys = *target(0x61).public_keys();
        let previous = RemoteControlTargetAccess::new(
            crate::remote_control::RemoteControlTargetIdentity::new(target_public_keys),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        remote_control.set_target_access(previous).unwrap();
        let mut state = RemoteControlPairingAuthorizationTransactionState::new();
        let attempt_id = attempt(0x74);

        let prepared = prepare(
            &mut remote_control,
            &mut state,
            attempt_id,
            RemoteControlPairingAuthorization::TargetAccess {
                target_public_keys,
                permitted_requests: RemoteControlRequestSet::only(
                    RemoteControlRequestKind::AnnounceSelf,
                ),
            },
        )
        .unwrap();
        assert_eq!(
            remote_control
                .target_accesses()
                .unwrap()
                .accesses_in_identity_hash_order()[0]
                .permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
        assert_eq!(
            read_remote_control_target_accesses_snapshot(&prepared)
                .unwrap()
                .next()
                .unwrap()
                .permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf),
        );
        let rollback = snapshot_rollback(&remote_control, &state, attempt_id).unwrap();
        assert_eq!(
            read_remote_control_target_accesses_snapshot(&rollback)
                .unwrap()
                .next()
                .unwrap()
                .permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );

        activate(&mut remote_control, &mut state, attempt_id).unwrap();
        roll_back(&mut remote_control, &mut state, attempt_id).unwrap();
        assert_eq!(
            remote_control
                .target_accesses()
                .unwrap()
                .accesses_in_identity_hash_order()[0]
                .permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
    }

    #[test]
    fn mismatched_attempt_cannot_activate_or_release_the_active_transaction() {
        let mut remote_control = remote_control();
        let mut state = RemoteControlPairingAuthorizationTransactionState::new();
        let active = attempt(0x75);
        let interfering = attempt(0x76);
        let grant = RemoteControlControllerGrant::new(
            controller(0x45),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        prepare(
            &mut remote_control,
            &mut state,
            active,
            RemoteControlPairingAuthorization::ControllerGrant(grant),
        )
        .unwrap();

        assert!(matches!(
            activate(&mut remote_control, &mut state, interfering),
            Err(RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested,
                active: retained,
            }) if requested == interfering && retained == active
        ));
        assert!(matches!(
            release(&mut state, interfering),
            Err(RemoteControlPairingAuthorizationTransactionFailure::AttemptMismatch {
                requested,
                active: retained,
            }) if requested == interfering && retained == active
        ));
        assert!(state.is_active());
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }
}
