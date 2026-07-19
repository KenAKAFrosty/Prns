use std::path::{Path, PathBuf};

use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::rnx::APP_NAME;
use personal_rns::runtime::load_or_create_identity_secret;

use crate::utilities::configuration::LoadedConfiguration;

use super::RnxError;

pub(super) fn load_identity(
    configuration: &LoadedConfiguration,
    explicit: Option<&Path>,
) -> Result<IdentitySecretKey, RnxError> {
    let path = match explicit {
        Some(path) => expand_user_path(path)?,
        None => configuration
            .discovered
            .dir
            .join("storage")
            .join("identities")
            .join(APP_NAME),
    };
    load_or_create_identity_secret(&path).map_err(|source| RnxError::Identity { path, source })
}

fn expand_user_path(path: &Path) -> Result<PathBuf, RnxError> {
    let Some(text) = path.to_str() else {
        return Ok(path.to_owned());
    };
    let suffix = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\"));
    if text != "~" && suffix.is_none() {
        return Ok(path.to_owned());
    }
    let home = home_directory().ok_or_else(|| RnxError::HomeUnavailable(path.to_owned()))?;
    Ok(suffix.map_or(home.clone(), |suffix| home.join(suffix)))
}

pub(super) fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

pub(super) fn pretty_hash(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("<{hex}>")
}
