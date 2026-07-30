use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::record::{decode_os, encode_os};
use crate::state::atomic_write;
use crate::{ServiceError, ServicePaths};

const ACTIVE_CONFIG_VERSION: &str = "prnsd-active-config-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveConfigRecord {
    pub(crate) generation: u128,
    pub(crate) directory: PathBuf,
}

impl ActiveConfigRecord {
    fn encode(&self) -> String {
        format!(
            "{ACTIVE_CONFIG_VERSION}\n{:032x}\n{}\n",
            self.generation,
            encode_os(self.directory.as_os_str()),
        )
    }

    fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(ACTIVE_CONFIG_VERSION) {
            return None;
        }
        let generation = lines
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())?;
        let directory = lines.next().and_then(decode_os).map(PathBuf::from)?;
        if lines.next().is_some() || !directory.is_absolute() {
            return None;
        }
        Some(Self {
            generation,
            directory,
        })
    }
}

pub(crate) fn read(paths: &ServicePaths) -> Result<Option<ActiveConfigRecord>, ServiceError> {
    match fs::read_to_string(&paths.active_config) {
        Ok(text) => ActiveConfigRecord::decode(&text)
            .map(Some)
            .ok_or(ServiceError::InvalidManagedConfigRecord),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ServiceError::Io {
            operation: "could not read the managed prnsd configuration record",
            source,
        }),
    }
}

pub(crate) fn write(paths: &ServicePaths, record: &ActiveConfigRecord) -> Result<(), ServiceError> {
    atomic_write(
        &paths.active_config,
        record.encode().as_bytes(),
        "could not publish the managed prnsd configuration directory",
    )
}

pub(crate) fn remove_if_matching(paths: &ServicePaths, generation: u128) {
    if read(paths)
        .ok()
        .flatten()
        .is_some_and(|record| record.generation == generation)
    {
        let _ = fs::remove_file(&paths.active_config);
    }
}

pub(crate) fn absolute(directory: &Path) -> Result<PathBuf, ServiceError> {
    std::path::absolute(directory).map_err(|source| ServiceError::Io {
        operation: "could not resolve the managed prnsd configuration directory",
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_encoded_paths() {
        let directory = std::path::absolute("configuration").expect("absolute path");
        let record = ActiveConfigRecord {
            generation: 42,
            directory,
        };
        assert_eq!(ActiveConfigRecord::decode(&record.encode()), Some(record));
    }

    #[test]
    fn relative_and_malformed_records_are_rejected() {
        let relative = ActiveConfigRecord {
            generation: 42,
            directory: PathBuf::from("relative"),
        }
        .encode();
        assert!(ActiveConfigRecord::decode(&relative).is_none());
        assert!(ActiveConfigRecord::decode("not-a-record").is_none());
    }

    #[test]
    fn published_records_are_owner_only() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let paths = ServicePaths::in_dir(directory.path());
        let record = ActiveConfigRecord {
            generation: 42,
            directory: std::path::absolute("configuration").expect("absolute path"),
        };
        write(&paths, &record).expect("record is published");
        assert_eq!(read(&paths).expect("record is readable"), Some(record));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&paths.active_config)
                    .expect("record metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
