use crate::{Error, Result};
use prns_storage::files as shared;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

impl From<shared::Error> for Error {
    fn from(error: shared::Error) -> Self {
        match error {
            shared::Error::Io(error) => Self::Io(error),
            shared::Error::UnsafePath => Self::UnsafePath,
            shared::Error::UnsafePermissions => Self::UnsafePermissions,
            shared::Error::UnsupportedPlatform => Self::UnsupportedPlatform,
        }
    }
}

pub(crate) fn new_directory(path: &Path) -> Result<()> {
    shared::create_private_directory(path).map_err(Into::into)
}
pub(crate) fn check_directory(path: &Path) -> Result<()> {
    shared::check_private_directory(path).map_err(Into::into)
}
pub(crate) fn check_file(path: &Path) -> Result<bool> {
    shared::check_private_file(path).map_err(Into::into)
}
pub(crate) fn new_file(path: &Path) -> Result<File> {
    shared::create_private_file(path).map_err(Into::into)
}

pub(crate) fn read_bounded(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>> {
    if !check_file(path)? {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    let size = usize::try_from(file.metadata()?.len()).map_err(|_| Error::Bounds)?;
    if size > maximum {
        return Err(Error::Bounds);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Error::Bounds)?;
    Read::by_ref(&mut file)
        .take((maximum as u64) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum || bytes.len() != size {
        return Err(Error::Corrupt);
    }
    Ok(Some(bytes))
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    shared::sync_directory(path).map_err(Into::into)
}

pub(crate) fn sync_published(path: &Path) -> Result<()> {
    if !check_file(path)? {
        return Err(Error::MissingKey);
    }
    File::open(path)?.sync_all()?;
    sync_directory(path.parent().ok_or(Error::UnsafePath)?)
}

/// The caller owns the directory lock. Retained key stages never use this
/// replacing helper: catalog stages may be superseded only after reading a
/// valid authoritative current catalog.
pub(crate) fn replace(path: &Path, stage: &Path, bytes: &[u8], family: &str) -> Result<()> {
    if check_file(stage)? {
        fs::remove_file(stage)?;
    }
    crash(family, "before_stage");
    let mut file = new_file(stage)?;
    file.write_all(bytes)?;
    crash(family, "before_file_sync");
    file.sync_all()?;
    crash(family, "after_file_sync");
    fs::rename(stage, path)?;
    crash(family, "after_rename");
    sync_directory(path.parent().ok_or(Error::UnsafePath)?)?;
    crash(family, "after_directory_sync");
    Ok(())
}

pub(crate) fn crash(family: &str, step: &str) {
    #[cfg(feature = "fault-injection")]
    if std::env::var("PRNS_IDENTITIES_CRASH_AT").ok().as_deref()
        == Some(format!("{family}.{step}").as_str())
    {
        // exit deliberately bypasses destructors, like an abruptly stopped app.
        std::process::exit(91);
    }
    #[cfg(not(feature = "fault-injection"))]
    let _ = (family, step);
}
