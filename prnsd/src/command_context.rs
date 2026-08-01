use std::fmt;
use std::path::{Path, PathBuf};

use prns_config::DiscoveredConfig;

#[derive(Debug)]
pub(crate) enum CommandContextError {
    Configuration(prns_config::DiscoveryError),
    StateDirectory(prnsd_control::StateDirectoryError),
    ActiveConfiguration(prnsd_control::ServiceError),
}

impl fmt::Display for CommandContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => write!(formatter, "config discovery failed: {error}"),
            Self::StateDirectory(error) => write!(formatter, "daemon discovery failed: {error}"),
            Self::ActiveConfiguration(error) => {
                write!(
                    formatter,
                    "could not resolve the running daemon's config: {error}"
                )
            }
        }
    }
}

impl std::error::Error for CommandContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::StateDirectory(error) => Some(error),
            Self::ActiveConfiguration(error) => Some(error),
        }
    }
}

pub(crate) fn discover(explicit: Option<&Path>) -> Result<DiscoveredConfig, CommandContextError> {
    resolve_with(explicit, || {
        let paths =
            prnsd_control::ServicePaths::discover().map_err(CommandContextError::StateDirectory)?;
        prnsd_control::active_config_dir(&paths).map_err(CommandContextError::ActiveConfiguration)
    })
}

pub(crate) fn resolve_with(
    explicit: Option<&Path>,
    active_config: impl FnOnce() -> Result<Option<PathBuf>, CommandContextError>,
) -> Result<DiscoveredConfig, CommandContextError> {
    if let Some(explicit) = explicit {
        return prns_config::discover(Some(explicit)).map_err(CommandContextError::Configuration);
    }
    match active_config()? {
        Some(directory) => {
            prns_config::discover(Some(&directory)).map_err(CommandContextError::Configuration)
        }
        None => prns_config::discover(None).map_err(CommandContextError::Configuration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_wins_without_consulting_managed_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let discovered = resolve_with(Some(directory.path()), || {
            panic!("explicit config must not inspect managed state")
        })
        .expect("explicit config");
        assert_eq!(discovered.dir, directory.path());
    }

    #[test]
    fn active_config_precedes_the_platform_fallback() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let discovered =
            resolve_with(None, || Ok(Some(directory.path().to_path_buf()))).expect("active config");
        assert_eq!(discovered.dir, directory.path());
    }

    #[test]
    fn active_config_failures_do_not_fall_back_silently() {
        let result = resolve_with(None, || {
            Err(CommandContextError::ActiveConfiguration(
                prnsd_control::ServiceError::InvalidManagedConfigRecord,
            ))
        });
        assert!(matches!(
            result,
            Err(CommandContextError::ActiveConfiguration(
                prnsd_control::ServiceError::InvalidManagedConfigRecord
            ))
        ));
    }

    #[test]
    fn no_active_daemon_uses_the_platform_reticulum_directory() {
        let expected = prns_config::discover(None).expect("platform config");
        let discovered = resolve_with(None, || Ok(None)).expect("fallback config");
        assert_eq!(discovered, expected);
    }
}
