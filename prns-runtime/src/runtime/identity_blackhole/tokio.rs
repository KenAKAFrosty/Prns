use std::string::String;
use std::vec::Vec;

use tokio::sync::oneshot;

use crate::engine::{EngineState, WakeSchedules};
use crate::identity::IdentityHash;
use crate::routing::blackhole::BlackholeTable;
use crate::routing::{
    BlackholeIdentityOutcome, BlackholeInsertFailure, BlackholedIdentity, RemovedRoute,
    UnblackholeIdentityOutcome,
};
use crate::storage::StorageLayout;

use super::{IdentityBlackholeControlError, IdentityBlackholeSourceError};

pub enum IdentityBlackholeHostCommand {
    ReadAll {
        reply: oneshot::Sender<Vec<BlackholedIdentity<String>>>,
    },
    IsBlackholed {
        identity: IdentityHash,
        reply: oneshot::Sender<bool>,
    },
    Blackhole {
        entry: BlackholedIdentity<String>,
        reply: oneshot::Sender<Result<BlackholeIdentityOutcome, IdentityBlackholeControlError>>,
    },
    Unblackhole {
        identity: IdentityHash,
        reply: oneshot::Sender<Result<UnblackholeIdentityOutcome, IdentityBlackholeControlError>>,
    },
}

pub(crate) fn apply_identity_blackhole_command<S: StorageLayout>(
    engine: &mut EngineState<S>,
    command: IdentityBlackholeHostCommand,
    on_removed: &mut impl FnMut(RemovedRoute),
) -> WakeSchedules {
    match command {
        IdentityBlackholeHostCommand::ReadAll { reply } => {
            let entries = engine
                .blackholed_identities()
                .map(|entry| BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.map(String::from),
                })
                .collect();
            let _ = reply.send(entries);
            WakeSchedules::UNCHANGED
        }
        IdentityBlackholeHostCommand::IsBlackholed { identity, reply } => {
            let _ = reply.send(engine.is_identity_blackholed(&identity));
            WakeSchedules::UNCHANGED
        }
        IdentityBlackholeHostCommand::Blackhole { entry, reply } => {
            let result = engine
                .blackhole_identity(
                    BlackholedIdentity {
                        identity: entry.identity,
                        source: entry.source,
                        expiry: entry.expiry,
                        reason: entry.reason.as_deref(),
                    },
                    on_removed,
                )
                .map_err(|error| {
                    match <S::Blackholes as BlackholeTable>::classify_insert_error(error) {
                        BlackholeInsertFailure::CapacityExhausted => {
                            IdentityBlackholeControlError::CapacityExhausted
                        }
                        BlackholeInsertFailure::ReasonTooLong => {
                            IdentityBlackholeControlError::ReasonTooLong
                        }
                    }
                });
            let changed = result == Ok(BlackholeIdentityOutcome::Added);
            let _ = reply.send(result);
            if changed {
                blackhole_wake(engine)
            } else {
                WakeSchedules::UNCHANGED
            }
        }
        IdentityBlackholeHostCommand::Unblackhole { identity, reply } => {
            let outcome = engine.unblackhole_identity(&identity);
            let changed = outcome == UnblackholeIdentityOutcome::Removed;
            let _ = reply.send(Ok(outcome));
            if changed {
                blackhole_wake(engine)
            } else {
                WakeSchedules::UNCHANGED
            }
        }
    }
}

fn blackhole_wake<S: StorageLayout>(engine: &EngineState<S>) -> WakeSchedules {
    WakeSchedules {
        expired_blackholes: engine.blackhole_expiry_wake(),
        ..WakeSchedules::UNCHANGED
    }
}

pub(crate) async fn settle_source<T>(
    commands: tokio::sync::mpsc::UnboundedSender<crate::reactor::impls::tokio_reactor::HostCommand>,
    build: impl FnOnce(oneshot::Sender<T>) -> IdentityBlackholeHostCommand,
) -> Result<T, IdentityBlackholeSourceError> {
    let (reply, settled) = oneshot::channel();
    commands
        .send(crate::reactor::impls::tokio_reactor::HostCommand::IdentityBlackhole(build(reply)))
        .map_err(|_| IdentityBlackholeSourceError::NodeStopped)?;
    settled
        .await
        .map_err(|_| IdentityBlackholeSourceError::NodeStopped)
}

pub(crate) async fn settle_control<T>(
    commands: tokio::sync::mpsc::UnboundedSender<crate::reactor::impls::tokio_reactor::HostCommand>,
    build: impl FnOnce(
        oneshot::Sender<Result<T, IdentityBlackholeControlError>>,
    ) -> IdentityBlackholeHostCommand,
) -> Result<T, IdentityBlackholeControlError> {
    let (reply, settled) = oneshot::channel();
    commands
        .send(crate::reactor::impls::tokio_reactor::HostCommand::IdentityBlackhole(build(reply)))
        .map_err(|_| IdentityBlackholeControlError::NodeStopped)?;
    settled
        .await
        .map_err(|_| IdentityBlackholeControlError::NodeStopped)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use crate::engine::WakeSchedule;
    use crate::routing::BlackholeExpiry;
    use crate::units::InstantMillis;

    fn entry(reason: Option<String>) -> BlackholedIdentity<String> {
        BlackholedIdentity {
            identity: IdentityHash::new([0x31; 16]),
            source: IdentityHash::new([0x41; 16]),
            expiry: BlackholeExpiry::At(InstantMillis(1_500)),
            reason,
        }
    }

    #[tokio::test]
    async fn commands_mutate_read_and_reschedule_the_engine_table() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (reply, settled) = oneshot::channel();
        let delta = apply_identity_blackhole_command(
            &mut engine,
            IdentityBlackholeHostCommand::Blackhole {
                entry: entry(Some("operator".into())),
                reply,
            },
            &mut |_| {},
        );
        assert_eq!(settled.await, Ok(Ok(BlackholeIdentityOutcome::Added)));
        assert_eq!(
            delta.expired_blackholes,
            WakeSchedule::At(InstantMillis(1_501))
        );

        let (reply, settled) = oneshot::channel();
        assert_eq!(
            apply_identity_blackhole_command(
                &mut engine,
                IdentityBlackholeHostCommand::ReadAll { reply },
                &mut |_| {},
            ),
            WakeSchedules::UNCHANGED,
        );
        assert_eq!(settled.await, Ok(vec![entry(Some("operator".into()))]));

        let (reply, settled) = oneshot::channel();
        let delta = apply_identity_blackhole_command(
            &mut engine,
            IdentityBlackholeHostCommand::Unblackhole {
                identity: IdentityHash::new([0x31; 16]),
                reply,
            },
            &mut |_| {},
        );
        assert_eq!(settled.await, Ok(Ok(UnblackholeIdentityOutcome::Removed)));
        assert_eq!(delta.expired_blackholes, WakeSchedule::Idle);
    }

    #[tokio::test]
    async fn storage_rejections_cross_the_runtime_as_typed_control_errors() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let (reply, settled) = oneshot::channel();
        let _ = apply_identity_blackhole_command(
            &mut engine,
            IdentityBlackholeHostCommand::Blackhole {
                entry: entry(Some("x".repeat(65))),
                reply,
            },
            &mut |_| {},
        );
        assert_eq!(
            settled.await,
            Ok(Err(IdentityBlackholeControlError::ReasonTooLong))
        );
        assert_eq!(engine.blackholed_identity_count(), 0);

        for identity_byte in 0..8 {
            let (reply, settled) = oneshot::channel();
            let mut entry = entry(None);
            entry.identity = IdentityHash::new([identity_byte; 16]);
            let _ = apply_identity_blackhole_command(
                &mut engine,
                IdentityBlackholeHostCommand::Blackhole { entry, reply },
                &mut |_| {},
            );
            assert_eq!(settled.await, Ok(Ok(BlackholeIdentityOutcome::Added)));
        }
        let (reply, settled) = oneshot::channel();
        let mut overflow = entry(None);
        overflow.identity = IdentityHash::new([0x80; 16]);
        let _ = apply_identity_blackhole_command(
            &mut engine,
            IdentityBlackholeHostCommand::Blackhole {
                entry: overflow,
                reply,
            },
            &mut |_| {},
        );
        assert_eq!(
            settled.await,
            Ok(Err(IdentityBlackholeControlError::CapacityExhausted))
        );
        assert_eq!(engine.blackholed_identity_count(), 8);
    }
}
