use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::sam::{I2pPrivateDestination, SamValueError};
use super::{generate_session_id, I2pSessionIdError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pDestinationKeyPath(PathBuf);

impl I2pDestinationKeyPath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, I2pDestinationKeyPathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(I2pDestinationKeyPathError::Empty);
        }
        if path.file_name().is_none() {
            return Err(I2pDestinationKeyPathError::MissingFileName);
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pDestinationKeyPathError {
    Empty,
    MissingFileName,
}

impl fmt::Display for I2pDestinationKeyPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("I2P destination key path is empty"),
            Self::MissingFileName => {
                formatter.write_str("I2P destination key path has no file name")
            }
        }
    }
}

impl std::error::Error for I2pDestinationKeyPathError {}

#[derive(Debug)]
pub enum I2pDestinationStorageError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Invalid {
        path: PathBuf,
        source: SamValueError,
    },
    CreateDirectory {
        path: PathBuf,
        source: io::Error,
    },
    SessionId(I2pSessionIdError),
    CreateStaging {
        path: PathBuf,
        source: io::Error,
    },
    WriteStaging {
        path: PathBuf,
        source: io::Error,
    },
    SyncDirectory {
        path: PathBuf,
        source: io::Error,
    },
    Publish {
        staging: PathBuf,
        destination: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for I2pDestinationStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "could not read I2P destination {}: {source}",
                    path.display()
                )
            }
            Self::Invalid { path, source } => write!(
                formatter,
                "I2P destination {} is invalid: {source}",
                path.display()
            ),
            Self::CreateDirectory { path, source } => write!(
                formatter,
                "could not create I2P destination directory {}: {source}",
                path.display()
            ),
            Self::SessionId(source) => write!(formatter, "{source}"),
            Self::CreateStaging { path, source } => write!(
                formatter,
                "could not create temporary I2P destination {}: {source}",
                path.display()
            ),
            Self::WriteStaging { path, source } => write!(
                formatter,
                "could not persist temporary I2P destination {}: {source}",
                path.display()
            ),
            Self::SyncDirectory { path, source } => write!(
                formatter,
                "could not durably persist I2P destination directory {}: {source}",
                path.display()
            ),
            Self::Publish {
                staging,
                destination,
                source,
            } => write!(
                formatter,
                "could not publish I2P destination {} as {}: {source}",
                staging.display(),
                destination.display()
            ),
        }
    }
}

impl std::error::Error for I2pDestinationStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. }
            | Self::CreateDirectory { source, .. }
            | Self::CreateStaging { source, .. }
            | Self::WriteStaging { source, .. }
            | Self::SyncDirectory { source, .. }
            | Self::Publish { source, .. } => Some(source),
            Self::Invalid { source, .. } => Some(source),
            Self::SessionId(source) => Some(source),
        }
    }
}

pub fn load_destination(
    key_path: &I2pDestinationKeyPath,
) -> Result<Option<I2pPrivateDestination>, I2pDestinationStorageError> {
    let value = match fs::read_to_string(key_path.as_path()) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(I2pDestinationStorageError::Read {
                path: key_path.0.clone(),
                source,
            })
        }
    };
    I2pPrivateDestination::new(value)
        .map(Some)
        .map_err(|source| I2pDestinationStorageError::Invalid {
            path: key_path.0.clone(),
            source,
        })
}

pub fn persist_destination(
    key_path: &I2pDestinationKeyPath,
    generated: I2pPrivateDestination,
) -> Result<I2pPrivateDestination, I2pDestinationStorageError> {
    let parent = key_path
        .as_path()
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| I2pDestinationStorageError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let staging = staging_path(key_path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file =
        options
            .open(&staging)
            .map_err(|source| I2pDestinationStorageError::CreateStaging {
                path: staging.clone(),
                source,
            })?;
    if let Err(source) = write_and_sync(&mut file, generated.as_str().as_bytes()) {
        let _ = fs::remove_file(&staging);
        return Err(I2pDestinationStorageError::WriteStaging {
            path: staging,
            source,
        });
    }
    drop(file);
    match fs::hard_link(&staging, key_path.as_path()) {
        Ok(()) => {
            let _ = fs::remove_file(&staging);
            sync_directory(parent)?;
            Ok(generated)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&staging);
            sync_directory(parent)?;
            load_destination(key_path)?.ok_or_else(|| I2pDestinationStorageError::Read {
                path: key_path.0.clone(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "destination disappeared during creation",
                ),
            })
        }
        Err(source) => {
            let _ = fs::remove_file(&staging);
            Err(I2pDestinationStorageError::Publish {
                staging,
                destination: key_path.0.clone(),
                source,
            })
        }
    }
}

fn staging_path(key_path: &I2pDestinationKeyPath) -> Result<PathBuf, I2pDestinationStorageError> {
    let session = generate_session_id().map_err(I2pDestinationStorageError::SessionId)?;
    let file_name = key_path
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("destination");
    Ok(key_path
        .as_path()
        .with_file_name(format!(".{file_name}.{}.tmp", session.as_str())))
}

fn write_and_sync(file: &mut File, value: &[u8]) -> io::Result<()> {
    file.write_all(value)?;
    file.sync_all()
}

fn sync_directory(path: &Path) -> Result<(), I2pDestinationStorageError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| I2pDestinationStorageError::SyncDirectory {
                path: path.to_path_buf(),
                source,
            })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
