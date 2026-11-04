use super::Error;
use std::{fs, path::Path};

pub(super) fn private_directory(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::PrivatePath(path.into()));
    }
    private_permissions(path, &metadata)
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), Error> {
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(path)?;
    private_directory(path)
}

pub(super) fn private_file(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::PrivatePath(path.into()));
    }
    private_permissions(path, &metadata)
}

pub(super) fn regular_file_if_present(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(Error::PrivatePath(path.into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_permissions(path: &Path, metadata: &fs::Metadata) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::PrivatePath(path.into()));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, metadata);
    Ok(())
}
