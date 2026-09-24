use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("file exceeds its read budget")]
    Budget,
}

pub(crate) fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit.checked_add(1).ok_or(ReadError::Budget)?)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(ReadError::Budget);
    }
    Ok(bytes)
}

pub(crate) fn private_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

pub(crate) fn private_file(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut file = private_file(path)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub(crate) fn remove_directory(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
