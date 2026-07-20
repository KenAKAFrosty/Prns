use std::path::{Path, PathBuf};

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::runtime::{
    generate_identity_secret, load_or_create_identity_secret, IdentitySecretFileError,
};

#[must_use]
pub fn load_or_seed_transport_identity(
    storage_dir: &Path,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    match load_or_create_identity_secret(&storage_dir.join("transport_identity")) {
        Ok(key) => key,
        Err(error) => {
            tracing::error!(event = "identity_ephemeral", error = %error);
            generate_identity_secret()
        }
    }
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
}
