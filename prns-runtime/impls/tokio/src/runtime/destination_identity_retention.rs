use tokio::sync::oneshot;

use crate::engine::{EngineState, WakeSchedules};
use crate::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::storage::StorageLayout;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::DestinationIdentityRetentionControlError;

pub enum DestinationIdentityRetentionHostCommand {
    MarkUsed {
        destination: DestinationHash,
        reply: oneshot::Sender<MarkDestinationUsedOutcome>,
    },
    RetainDestination {
        destination: DestinationHash,
        reply: oneshot::Sender<RetainDestinationOutcome>,
    },
    ReleaseDestination {
        destination: DestinationHash,
        reply: oneshot::Sender<ReleaseDestinationOutcome>,
    },
    RetainIdentity {
        identity: IdentityHash,
        reply: oneshot::Sender<RetainIdentityOutcome>,
    },
}

pub(crate) fn apply_destination_identity_retention_command<S: StorageLayout>(
    engine: &mut EngineState<S>,
    command: DestinationIdentityRetentionHostCommand,
    now: InstantMillis,
) -> WakeSchedules {
    match command {
        DestinationIdentityRetentionHostCommand::MarkUsed { destination, reply } => {
            let effect = engine.mark_destination_used(&destination, now);
            let _ = reply.send(effect.outcome());
            effect.wake_schedules()
        }
        DestinationIdentityRetentionHostCommand::RetainDestination { destination, reply } => {
            let effect = engine.retain_destination(&destination);
            let _ = reply.send(effect.outcome());
            effect.wake_schedules()
        }
        DestinationIdentityRetentionHostCommand::ReleaseDestination { destination, reply } => {
            let effect = engine.release_destination(&destination, now);
            let _ = reply.send(effect.outcome());
            effect.wake_schedules()
        }
        DestinationIdentityRetentionHostCommand::RetainIdentity { identity, reply } => {
            let effect = engine.retain_identity(&identity);
            let _ = reply.send(effect.outcome());
            effect.wake_schedules()
        }
    }
}

pub(crate) async fn settle_destination_identity_retention<T>(
    commands: tokio::sync::mpsc::UnboundedSender<crate::manifold::driver::HostCommand>,
    build: impl FnOnce(oneshot::Sender<T>) -> DestinationIdentityRetentionHostCommand,
) -> Result<T, DestinationIdentityRetentionControlError> {
    let (reply, settled) = oneshot::channel();
    commands
        .send(crate::manifold::driver::HostCommand::DestinationIdentityRetention(build(reply)))
        .map_err(|_| DestinationIdentityRetentionControlError::NodeStopped)?;
    settled
        .await
        .map_err(|_| DestinationIdentityRetentionControlError::NodeStopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::engine::test_support::TestStorageLayout;
    use crate::engine::{DestinationIdentitySeedOutcome, WakeSchedule};
    use crate::identity::destination_identity::{
        DestinationIdentityRetentionState, DestinationIdentitySeed,
    };
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };

    fn identity_seed(
        retention: DestinationIdentityRetentionState,
    ) -> DestinationIdentitySeed<'static> {
        let public_keys = IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0x31; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0x41; 32])),
        };
        DestinationIdentitySeed {
            destination: DestinationHash::new([0x21; 16]),
            public_keys,
            announced_at: InstantMillis(1_000),
            retention,
            app_data: b"app",
        }
    }

    #[tokio::test]
    async fn commands_preserve_retention_semantics_and_rearm_expiry() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        assert_eq!(
            engine.seed_destination_identity(
                identity_seed(DestinationIdentityRetentionState::NeverUsed),
                InstantMillis(1_000),
            ),
            DestinationIdentitySeedOutcome::Seeded,
        );
        let destination = identity_seed(DestinationIdentityRetentionState::NeverUsed).destination;

        let (reply, settled) = oneshot::channel();
        let delta = apply_destination_identity_retention_command(
            &mut engine,
            DestinationIdentityRetentionHostCommand::MarkUsed { destination, reply },
            InstantMillis(2_000),
        );
        assert_eq!(settled.await, Ok(MarkDestinationUsedOutcome::Recorded));
        assert!(matches!(
            delta.expired_destination_identities,
            WakeSchedule::At(_)
        ));

        let (reply, settled) = oneshot::channel();
        let delta = apply_destination_identity_retention_command(
            &mut engine,
            DestinationIdentityRetentionHostCommand::RetainDestination { destination, reply },
            InstantMillis(3_000),
        );
        assert_eq!(settled.await, Ok(RetainDestinationOutcome::Retained));
        assert_eq!(delta.expired_destination_identities, WakeSchedule::Idle);

        let (reply, settled) = oneshot::channel();
        let delta = apply_destination_identity_retention_command(
            &mut engine,
            DestinationIdentityRetentionHostCommand::ReleaseDestination { destination, reply },
            InstantMillis(4_000),
        );
        assert_eq!(settled.await, Ok(ReleaseDestinationOutcome::Released));
        assert!(matches!(
            delta.expired_destination_identities,
            WakeSchedule::At(_)
        ));
    }

    #[tokio::test]
    async fn identity_retention_reports_all_matching_destinations() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let identity = identity_seed(DestinationIdentityRetentionState::NeverUsed);
        assert_eq!(
            engine.seed_destination_identity(identity, InstantMillis(1_000)),
            DestinationIdentitySeedOutcome::Seeded,
        );
        let (reply, settled) = oneshot::channel();
        let delta = apply_destination_identity_retention_command(
            &mut engine,
            DestinationIdentityRetentionHostCommand::RetainIdentity {
                identity: identity.public_keys.identity_hash(),
                reply,
            },
            InstantMillis(2_000),
        );
        assert_eq!(
            settled.await,
            Ok(RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 0,
            }),
        );
        assert_eq!(delta.expired_destination_identities, WakeSchedule::Idle);
    }
}
