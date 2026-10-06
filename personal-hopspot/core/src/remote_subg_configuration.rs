use embedded_storage_async::nor_flash::NorFlash;
use personal_rns::interfaces::subghz::SubGConfigurationState;
use personal_rns::runtime::RemoteControlHostCommandError;

use crate::{SubGConfigurationCommitOutcome, SubGConfigurationStore};

/// A successful remote change means both the radio and its reboot configuration agree.
pub async fn apply_remote_subg_configuration<F: NorFlash>(
    mut apply: impl AsyncFnMut(SubGConfigurationState) -> Result<(), RemoteControlHostCommandError>,
    store: &mut SubGConfigurationStore<F>,
    active: &mut SubGConfigurationState,
    requested: SubGConfigurationState,
) -> Result<(), RemoteControlHostCommandError> {
    let previous = *active;
    if previous != requested {
        apply(requested).await?;
        *active = requested;
    }
    // Even an unchanged running profile needs durable confirmation: an earlier rollback
    // may have failed after changing the radio or after an unconfirmed flash write.
    let persistence = match requested {
        SubGConfigurationState::Configured(configuration) => store.save(configuration).await,
        SubGConfigurationState::Unconfigured => store.clear().await,
    };
    match persistence {
        SubGConfigurationCommitOutcome::Committed => Ok(()),
        SubGConfigurationCommitOutcome::NotCommitted(_) => {
            apply(previous)
                .await
                .map_err(|_| RemoteControlHostCommandError::RollbackFailed)?;
            *active = previous;
            Err(RemoteControlHostCommandError::PersistenceFailed)
        }
        SubGConfigurationCommitOutcome::Indeterminate(_) => {
            apply(previous)
                .await
                .map_err(|_| RemoteControlHostCommandError::RollbackFailed)?;
            *active = previous;
            // An unconfirmed write may already be durable. Recommit the old state before
            // reporting a successful rollback; changing only the radio is insufficient.
            let rollback = match previous {
                SubGConfigurationState::Configured(configuration) => {
                    store.save(configuration).await
                }
                SubGConfigurationState::Unconfigured => store.clear().await,
            };
            match rollback {
                SubGConfigurationCommitOutcome::Committed => {
                    Err(RemoteControlHostCommandError::PersistenceFailed)
                }
                SubGConfigurationCommitOutcome::NotCommitted(_)
                | SubGConfigurationCommitOutcome::Indeterminate(_) => {
                    Err(RemoteControlHostCommandError::RollbackFailed)
                }
            }
        }
    }
}
