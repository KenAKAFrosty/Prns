use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;

use crate::record::ServiceRecord;
use crate::{ServiceError, ServicePaths};

pub(crate) fn runtime_is_locked(paths: &ServicePaths) -> Result<bool, ServiceError> {
    let file = open_lock(&paths.runtime_lock, "could not open prnsd runtime lock")?;
    match file.try_lock() {
        Ok(()) => {
            file.unlock().map_err(|source| ServiceError::Io {
                operation: "could not unlock the prnsd runtime probe",
                source,
            })?;
            Ok(false)
        }
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(source)) => Err(ServiceError::Io {
            operation: "could not inspect the prnsd runtime lock",
            source,
        }),
    }
}

pub(crate) fn ready_generation(paths: &ServicePaths) -> Result<Option<u128>, ServiceError> {
    read_generation(&paths.ready, "could not read prnsd readiness marker")
}

pub(crate) fn read_generation(
    path: &Path,
    operation: &'static str,
) -> Result<Option<u128>, ServiceError> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| ServiceError::InvalidRecord),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

pub(crate) fn write_generation(
    path: &Path,
    generation: u128,
    operation: &'static str,
) -> Result<(), ServiceError> {
    atomic_write(path, format!("{generation}\n").as_bytes(), operation)
}

pub(crate) fn read_record(paths: &ServicePaths) -> Result<Option<ServiceRecord>, ServiceError> {
    match fs::read_to_string(&paths.record) {
        Ok(text) => ServiceRecord::decode(&text)
            .map(Some)
            .map_err(|_| ServiceError::InvalidRecord),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ServiceError::Io {
            operation: "could not read the prnsd session record",
            source,
        }),
    }
}

pub(crate) fn write_record(
    paths: &ServicePaths,
    record: &ServiceRecord,
) -> Result<(), ServiceError> {
    atomic_write(
        &paths.record,
        record.encode().as_bytes(),
        "could not write the prnsd session record",
    )
}

pub(crate) fn atomic_write(
    path: &Path,
    bytes: &[u8],
    operation: &'static str,
) -> Result<(), ServiceError> {
    let parent = path.parent().ok_or_else(|| ServiceError::Io {
        operation,
        source: io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|source| ServiceError::Io { operation, source })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| ServiceError::Io { operation, source })?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| ServiceError::Io {
            operation,
            source: error.error,
        })
}

pub(crate) fn cleanup_stale(paths: &ServicePaths) -> Result<(), ServiceError> {
    remove_if_present(&paths.record, "could not remove stale prnsd session record")?;
    remove_if_present(
        &paths.ready,
        "could not remove stale prnsd readiness marker",
    )?;
    remove_if_present(&paths.stop, "could not remove stale prnsd stop request")
}

pub(crate) fn remove_generation_if_matching(path: &Path, generation: u128) {
    if fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        == Some(generation)
    {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn open_lock(path: &Path, operation: &'static str) -> Result<File, ServiceError> {
    open_secure(path, false, true, operation)
}

pub(crate) fn open_secure(
    path: &Path,
    truncate: bool,
    create: bool,
    operation: &'static str,
) -> Result<File, ServiceError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .truncate(truncate)
        .create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|source| ServiceError::Io { operation, source })
}

pub(crate) fn prepare_state_dir(paths: &ServicePaths) -> Result<(), ServiceError> {
    fs::create_dir_all(&paths.state_dir).map_err(|source| ServiceError::Io {
        operation: "could not create the prnsd state directory",
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.state_dir, fs::Permissions::from_mode(0o700)).map_err(
            |source| ServiceError::Io {
                operation: "could not protect the prnsd state directory",
                source,
            },
        )?;
    }
    Ok(())
}

pub(crate) fn remove_if_present(path: &Path, operation: &'static str) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(name: &str) -> ServicePaths {
        ServicePaths::in_dir(
            std::env::temp_dir().join(format!("prnsd-control-{name}-{}", std::process::id())),
        )
    }
    #[test]
    fn runtime_lock_is_the_liveness_source() {
        let paths = test_paths("liveness");
        prepare_state_dir(&paths).unwrap();
        assert!(!runtime_is_locked(&paths).unwrap());
        let lock = open_lock(&paths.runtime_lock, "test lock").unwrap();
        lock.try_lock().unwrap();
        assert!(runtime_is_locked(&paths).unwrap());
        lock.unlock().unwrap();
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn stop_requests_are_generation_scoped() {
        let paths = test_paths("generation");
        prepare_state_dir(&paths).unwrap();
        write_generation(&paths.stop, 41, "test stop").unwrap();
        assert_eq!(read_generation(&paths.stop, "test stop").unwrap(), Some(41));
        remove_generation_if_matching(&paths.stop, 42);
        assert!(paths.stop.exists());
        remove_generation_if_matching(&paths.stop, 41);
        assert!(!paths.stop.exists());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }
}
