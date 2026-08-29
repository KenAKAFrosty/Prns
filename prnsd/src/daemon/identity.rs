use std::path::{Path, PathBuf};

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::remote_control::RemoteControlNodeIdentityBootstrap;
use personal_rns::runtime::{
    load_or_create_identity_secret, IdentitySecretFileError,
    RemoteControlFileIdentityBootstrapError, RemoteControlIdentityDirectory,
};

const REMOTE_CONTROL_IDENTITY_DIRECTORY: &str = "remote_control";

pub fn load_or_create_transport_identity(
    storage_dir: &Path,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, IdentitySecretFileError> {
    load_or_create_identity_secret(&storage_dir.join("transport_identity"))
}

pub fn load_or_create_remote_control_identities(
    storage_dir: &Path,
) -> Result<RemoteControlNodeIdentityBootstrap, RemoteControlFileIdentityBootstrapError> {
    RemoteControlIdentityDirectory::new(storage_dir.join(REMOTE_CONTROL_IDENTITY_DIRECTORY))
        .load_or_generate()
}

pub fn load_or_seed_network_identity(
    configured_path: Option<&Path>,
) -> Result<Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>, NetworkIdentityError> {
    let Some(configured_path) = configured_path else {
        return Ok(None);
    };
    let path = expand_user_path(configured_path, std::env::var_os("HOME").as_deref())?;
    load_or_create_identity_secret(&path)
        .map(Some)
        .map_err(|source| NetworkIdentityError::Secret { path, source })
}

fn expand_user_path(
    path: &Path,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, NetworkIdentityError> {
    let Ok(rest) = path.strip_prefix("~") else {
        return Ok(path.to_path_buf());
    };
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return Err(NetworkIdentityError::HomeUnavailable {
            path: path.to_path_buf(),
        });
    };
    Ok(Path::new(home).join(rest))
}

#[derive(Debug)]
pub enum NetworkIdentityError {
    HomeUnavailable {
        path: PathBuf,
    },
    Secret {
        path: PathBuf,
        source: IdentitySecretFileError,
    },
}

impl core::fmt::Display for NetworkIdentityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HomeUnavailable { path } => write!(
                formatter,
                "network identity path {} needs a home directory, but HOME is unavailable",
                path.display()
            ),
            Self::Secret { path, source } => {
                write!(formatter, "network identity {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for NetworkIdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HomeUnavailable { .. } => None,
            Self::Secret { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_relative_network_identity_paths_expand_from_the_supplied_home() {
        assert_eq!(
            expand_user_path(
                Path::new("~/.reticulum/network_identity"),
                Some(std::ffi::OsStr::new("/home/operator")),
            )
            .unwrap(),
            PathBuf::from("/home/operator/.reticulum/network_identity")
        );
        assert_eq!(
            expand_user_path(
                Path::new("/var/lib/reticulum/network_identity"),
                Some(std::ffi::OsStr::new("/home/operator")),
            )
            .unwrap(),
            PathBuf::from("/var/lib/reticulum/network_identity")
        );
    }

    #[test]
    fn a_user_relative_path_requires_a_home_directory() {
        assert!(matches!(
            expand_user_path(Path::new("~/network_identity"), None),
            Err(NetworkIdentityError::HomeUnavailable { .. })
        ));
    }

    #[test]
    fn a_transport_identity_error_is_never_replaced_with_an_ephemeral_identity() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("transport_identity")).unwrap();

        assert!(load_or_create_transport_identity(directory.path()).is_err());
    }

    #[test]
    fn a_remote_control_identity_error_is_never_replaced_with_ephemeral_identities() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(REMOTE_CONTROL_IDENTITY_DIRECTORY), []).unwrap();

        assert!(load_or_create_remote_control_identities(directory.path()).is_err());
    }
}
