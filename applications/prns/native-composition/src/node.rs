use std::path::{Component, Path, PathBuf};

const PRIVATE_STORAGE_SUFFIX: &str = "prns/development";

pub struct NodeStoragePaths {
    pub root: PathBuf,
    pub identities: PathBuf,
    pub remote_control_identities: PathBuf,
    pub bluetooth_identity: PathBuf,
    pub network: PathBuf,
    pub application: PathBuf,
}

pub fn prepare_storage(root: &Path) -> Result<NodeStoragePaths, String> {
    validate_private_root(root)?;
    std::fs::create_dir_all(root)
        .map_err(|error| format!("could not create the private application directory: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve the private application directory: {error}"))?;
    validate_resolved_private_root(&root)?;
    let identities = root.join("identities");
    std::fs::create_dir_all(&identities)
        .map_err(|error| format!("could not create the identity directory: {error}"))?;
    Ok(NodeStoragePaths {
        remote_control_identities: identities.join("remote-control"),
        bluetooth_identity: identities.join("bluetooth-auto.identity"),
        network: root.join("network"),
        application: root.join("application.redb"),
        identities,
        root,
    })
}

pub fn reset_storage(root: &Path) -> Result<(), String> {
    validate_private_root(root)?;
    let resolved = match root.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not resolve the private development directory: {error}"
            ))
        }
    };
    validate_resolved_private_root(&resolved)?;
    match std::fs::remove_dir_all(resolved) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not remove the private development directory: {error}"
        )),
    }
}

fn validate_private_root(root: &Path) -> Result<(), String> {
    if root.as_os_str().len() > crate::input::MAX_PATH_BYTES {
        return Err("the native storage directory exceeds the path size limit".to_owned());
    }
    if !root.is_absolute() {
        return Err("the native storage directory must be absolute".to_owned());
    }
    for component in root.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::Prefix(_) if cfg!(windows) => {}
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(
                    "the native storage directory must not contain relative path components"
                        .to_owned(),
                )
            }
        }
    }
    if !root.ends_with(PRIVATE_STORAGE_SUFFIX) {
        return Err("the native storage directory must end in prns/development".to_owned());
    }
    Ok(())
}

fn validate_resolved_private_root(root: &Path) -> Result<(), String> {
    validate_private_root(root).map_err(|_| {
        "the resolved native storage directory must end in prns/development".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_scope_rejects_broad_or_ambiguous_paths() {
        assert!(validate_private_root(Path::new("prns/development")).is_err());
        assert!(validate_private_root(Path::new("/")).is_err());
        assert!(validate_private_root(Path::new("/tmp/development")).is_err());
        assert!(validate_private_root(Path::new("/tmp/prns/development/../..")).is_err());
        assert!(validate_private_root(Path::new("/tmp/prns/development")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn reset_scope_rejects_a_resolved_symlink_escape() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let escaped = temporary.path().join("escaped").join("development");
        std::fs::create_dir_all(&escaped).expect("escaped directory");
        let requested_parent = temporary.path().join("requested");
        std::fs::create_dir_all(&requested_parent).expect("requested parent");
        std::os::unix::fs::symlink(
            escaped.parent().expect("escaped parent"),
            requested_parent.join("prns"),
        )
        .expect("storage symlink");
        let requested = requested_parent.join("prns").join("development");

        assert!(reset_storage(&requested).is_err());
        assert!(escaped.exists());
    }
}
