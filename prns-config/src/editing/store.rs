use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use crate::configobj::{ConfigDocument, ConfigError};

use super::document::EditedConfig;

#[derive(Debug, Clone)]
pub struct ConfigFile {
    path: PathBuf,
    original: Option<Vec<u8>>,
    document: ConfigDocument,
}

impl ConfigFile {
    pub fn load(path: impl Into<PathBuf>, fallback: &str) -> Result<Self, ConfigFileError> {
        let path = path.into();
        let (original, source) = match fs::read(&path) {
            Ok(bytes) => {
                let source = String::from_utf8(bytes.clone()).map_err(ConfigFileError::Encoding)?;
                (Some(bytes), source)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (None, fallback.to_string()),
            Err(source) => {
                return Err(ConfigFileError::Io {
                    operation: ConfigFileOperation::Read,
                    source,
                })
            }
        };
        let document = ConfigDocument::parse(&source).map_err(ConfigFileError::Syntax)?;
        Ok(Self {
            path,
            original,
            document,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn document(&self) -> &ConfigDocument {
        &self.document
    }

    pub fn is_materialized(&self) -> bool {
        self.original.is_some()
    }

    pub fn write(&self, edited: &EditedConfig) -> Result<ConfigWriteReceipt, ConfigFileError> {
        if edited.original() != self.document.source() {
            return Err(ConfigFileError::CandidateSourceMismatch);
        }
        let parent = self.path.parent().ok_or(ConfigFileError::MissingParent)?;
        fs::create_dir_all(parent).map_err(|source| ConfigFileError::Io {
            operation: ConfigFileOperation::CreateDirectory,
            source,
        })?;
        let current = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(ConfigFileError::Io {
                    operation: ConfigFileOperation::ReadBeforeWrite,
                    source,
                })
            }
        };
        if current != self.original {
            return Err(ConfigFileError::ConcurrentModification);
        }
        let permissions = current
            .as_ref()
            .and_then(|_| fs::metadata(&self.path).ok())
            .map(|metadata| metadata.permissions());
        let backup = match current {
            Some(bytes) => {
                let backup = backup_path(&self.path);
                atomic_write(&backup, &bytes, permissions.clone())?;
                Some(backup)
            }
            None => None,
        };
        atomic_write(&self.path, edited.candidate().as_bytes(), permissions)?;
        Ok(ConfigWriteReceipt {
            path: self.path.clone(),
            backup,
            created: self.original.is_none(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWriteReceipt {
    path: PathBuf,
    backup: Option<PathBuf>,
    created: bool,
}

impl ConfigWriteReceipt {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn backup(&self) -> Option<&Path> {
        self.backup.as_deref()
    }

    pub const fn created(&self) -> bool {
        self.created
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFileOperation {
    Read,
    CreateDirectory,
    ReadBeforeWrite,
    CreateTemporary,
    SetPermissions,
    WriteTemporary,
    Persist,
    SyncDirectory,
}

impl fmt::Display for ConfigFileOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read configuration",
            Self::CreateDirectory => "create configuration directory",
            Self::ReadBeforeWrite => "re-read configuration before writing",
            Self::CreateTemporary => "create temporary configuration",
            Self::SetPermissions => "set configuration permissions",
            Self::WriteTemporary => "write temporary configuration",
            Self::Persist => "replace configuration",
            Self::SyncDirectory => "sync configuration directory",
        })
    }
}

#[derive(Debug)]
pub enum ConfigFileError {
    Io {
        operation: ConfigFileOperation,
        source: io::Error,
    },
    Encoding(std::string::FromUtf8Error),
    Syntax(ConfigError),
    MissingParent,
    CandidateSourceMismatch,
    ConcurrentModification,
}

impl fmt::Display for ConfigFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "could not {operation}: {source}"),
            Self::Encoding(_) => formatter.write_str("configuration is not valid UTF-8"),
            Self::Syntax(error) => error.fmt(formatter),
            Self::MissingParent => {
                formatter.write_str("configuration path has no parent directory")
            }
            Self::CandidateSourceMismatch => {
                formatter.write_str("edited configuration does not belong to the loaded source")
            }
            Self::ConcurrentModification => formatter
                .write_str("configuration changed after it was loaded; no file was overwritten"),
        }
    }
}

impl std::error::Error for ConfigFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Encoding(source) => Some(source),
            Self::Syntax(source) => Some(source),
            Self::MissingParent | Self::CandidateSourceMismatch | Self::ConcurrentModification => {
                None
            }
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{file_name}.prns-backup"))
}

fn atomic_write(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> Result<(), ConfigFileError> {
    let parent = path.parent().ok_or(ConfigFileError::MissingParent)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| ConfigFileError::Io {
        operation: ConfigFileOperation::CreateTemporary,
        source,
    })?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|source| ConfigFileError::Io {
                operation: ConfigFileOperation::SetPermissions,
                source,
            })?;
    } else {
        protect_new_file(temporary.as_file())?;
    }
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| ConfigFileError::Io {
            operation: ConfigFileOperation::WriteTemporary,
            source,
        })?;
    temporary
        .persist(path)
        .map_err(|error| ConfigFileError::Io {
            operation: ConfigFileOperation::Persist,
            source: error.error,
        })?;
    sync_parent(parent)
}

#[cfg(unix)]
fn protect_new_file(file: &fs::File) -> Result<(), ConfigFileError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| ConfigFileError::Io {
            operation: ConfigFileOperation::SetPermissions,
            source,
        })
}

#[cfg(not(unix))]
fn protect_new_file(_file: &fs::File) -> Result<(), ConfigFileError> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), ConfigFileError> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| ConfigFileError::Io {
            operation: ConfigFileOperation::SyncDirectory,
            source,
        })
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), ConfigFileError> {
    Ok(())
}
