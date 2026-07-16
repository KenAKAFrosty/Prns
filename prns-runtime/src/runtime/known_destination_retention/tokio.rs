use tokio::sync::oneshot;

use crate::engine::{EngineState, WakeSchedules};
use crate::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::storage::StorageLayout;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::super::KnownDestinationRetentionControlError;

pub enum KnownDestinationRetentionHostCommand {
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

pub(crate) fn apply_known_destination_retention_command<S: StorageLayout>(
    engine: &mut EngineState<S>,
    command: KnownDestinationRetentionHostCommand,
    now: InstantMillis,
) -> WakeSchedules {
    let changed = match command {
        KnownDestinationRetentionHostCommand::MarkUsed { destination, reply } => {
            let outcome = engine.mark_destination_used(&destination, now);
            let changed = matches!(
                outcome,
                MarkDestinationUsedOutcome::Recorded | MarkDestinationUsedOutcome::Refreshed
            );
            let _ = reply.send(outcome);
            changed
        }
        KnownDestinationRetentionHostCommand::RetainDestination { destination, reply } => {
            let outcome = engine.retain_destination(&destination);
            let changed = outcome == RetainDestinationOutcome::Retained;
            let _ = reply.send(outcome);
            changed
        }
        KnownDestinationRetentionHostCommand::ReleaseDestination { destination, reply } => {
            let outcome = engine.release_destination(&destination, now);
            let changed = outcome != ReleaseDestinationOutcome::NotFound;
            let _ = reply.send(outcome);
            changed
        }
        KnownDestinationRetentionHostCommand::RetainIdentity { identity, reply } => {
            let outcome = engine.retain_identity(&identity);
            let changed = outcome.newly_retained_destination_count != 0;
            let _ = reply.send(outcome);
            changed
        }
    };
    if changed {
        WakeSchedules {
            expired_known_destinations: engine.known_destination_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    } else {
        WakeSchedules::UNCHANGED
    }
}

pub(crate) async fn settle_known_destination_retention<T>(
    commands: tokio::sync::mpsc::UnboundedSender<crate::reactor::impls::tokio_reactor::HostCommand>,
    build: impl FnOnce(oneshot::Sender<T>) -> KnownDestinationRetentionHostCommand,
) -> Result<T, KnownDestinationRetentionControlError> {
    let (reply, settled) = oneshot::channel();
    commands
        .send(
            crate::reactor::impls::tokio_reactor::HostCommand::KnownDestinationRetention(build(
                reply,
            )),
        )
        .map_err(|_| KnownDestinationRetentionControlError::NodeStopped)?;
    settled
        .await
        .map_err(|_| KnownDestinationRetentionControlError::NodeStopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::engine::test_support::TestStorageLayout;
    use crate::engine::{KnownDestinationSeedOutcome, WakeSchedule};
    use crate::identity::known::{KnownDestinationRetentionState, KnownDestinationSeed};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };

    fn known(retention: KnownDestinationRetentionState) -> KnownDestinationSeed<'static> {
        let public_keys = IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0x31; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0x41; 32])),
        };
        KnownDestinationSeed {
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
            engine.seed_known_destination(
                known(KnownDestinationRetentionState::NeverUsed),
                InstantMillis(1_000),
            ),
            KnownDestinationSeedOutcome::Seeded,
        );
        let destination = known(KnownDestinationRetentionState::NeverUsed).destination;

        let (reply, settled) = oneshot::channel();
        let delta = apply_known_destination_retention_command(
            &mut engine,
            KnownDestinationRetentionHostCommand::MarkUsed { destination, reply },
            InstantMillis(2_000),
        );
        assert_eq!(settled.await, Ok(MarkDestinationUsedOutcome::Recorded));
        assert!(matches!(
            delta.expired_known_destinations,
            WakeSchedule::At(_)
        ));

        let (reply, settled) = oneshot::channel();
        let delta = apply_known_destination_retention_command(
            &mut engine,
            KnownDestinationRetentionHostCommand::RetainDestination { destination, reply },
            InstantMillis(3_000),
        );
        assert_eq!(settled.await, Ok(RetainDestinationOutcome::Retained));
        assert_eq!(delta.expired_known_destinations, WakeSchedule::Idle);

        let (reply, settled) = oneshot::channel();
        let delta = apply_known_destination_retention_command(
            &mut engine,
            KnownDestinationRetentionHostCommand::ReleaseDestination { destination, reply },
            InstantMillis(4_000),
        );
        assert_eq!(settled.await, Ok(ReleaseDestinationOutcome::Released));
        assert!(matches!(
            delta.expired_known_destinations,
            WakeSchedule::At(_)
        ));
    }

    #[tokio::test]
    async fn identity_retention_reports_all_matching_destinations() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let known = known(KnownDestinationRetentionState::NeverUsed);
        assert_eq!(
            engine.seed_known_destination(known, InstantMillis(1_000)),
            KnownDestinationSeedOutcome::Seeded,
        );
        let (reply, settled) = oneshot::channel();
        let delta = apply_known_destination_retention_command(
            &mut engine,
            KnownDestinationRetentionHostCommand::RetainIdentity {
                identity: known.public_keys.identity_hash(),
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
        assert_eq!(delta.expired_known_destinations, WakeSchedule::Idle);
    }
}
