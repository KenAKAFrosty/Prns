use std::path::{Component, Path, PathBuf};

const PRIVATE_STORAGE_SUFFIX: [&str; 2] = ["prns", "development"];

pub struct NodeStoragePaths {
    pub root: PathBuf,
    pub remote_control_identities: PathBuf,
    pub bluetooth_identity: PathBuf,
    pub network: PathBuf,
}

pub fn prepare_storage(root: &Path) -> Result<NodeStoragePaths, String> {
    validate_private_root(root)?;
    std::fs::create_dir_all(root)
        .map_err(|error| format!("could not create the private application directory: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve the private application directory: {error}"))?;
    let identities = root.join("identities");
    std::fs::create_dir_all(&identities)
        .map_err(|error| format!("could not create the identity directory: {error}"))?;
    Ok(NodeStoragePaths {
        remote_control_identities: identities.join("remote-control"),
        bluetooth_identity: identities.join("bluetooth-auto.identity"),
        network: root.join("network"),
        root,
    })
}

pub fn reset_storage(root: &Path) -> Result<(), String> {
    validate_private_root(root)?;
    match std::fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not remove the private development directory: {error}"
        )),
    }
}

fn validate_private_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("the native storage directory must be absolute".to_owned());
    }
    let normal_components: Vec<_> = root
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    if !normal_components.ends_with(&PRIVATE_STORAGE_SUFFIX) {
        return Err("the native storage directory must end in prns/development".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_scope_rejects_broad_or_ambiguous_paths() {
        assert!(validate_private_root(Path::new("prns/development")).is_err());
        assert!(validate_private_root(Path::new("/")).is_err());
        assert!(validate_private_root(Path::new("/tmp/development")).is_err());
        assert!(validate_private_root(Path::new("/tmp/prns/development")).is_ok());
    }
}
