mod document;
mod file;
mod manual_configuration;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::interface_discovery::{DiscoveryCatalog, DiscoveryRecord};

use self::document::{ArchiveDocument, ArchiveDocumentRef, ArchivedRecord};
use self::file::replace_archive_file;

pub use self::file::{
    ArchiveFileOperation, ArchiveRecordError, DiscoveryArchiveError, HexDecodeError,
};

pub const DISCOVERED_INTERFACES_FILE: &str = "discovered_interfaces.json";
const FORMAT_VERSION: u32 = 1;
const MAX_ARCHIVE_BYTES: usize = 8 * 1024 * 1024;
const CONFIGURATION_NOTE: &str =
    "Copy a configuration_entry beneath [interfaces] in the Reticulum config, then fill any blank hardware-specific fields.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryArchiveFileState {
    Missing,
    Present,
}

pub struct LoadedDiscoveryArchive {
    pub archive: DiscoveryArchive,
    pub catalog: DiscoveryCatalog,
    pub file_state: DiscoveryArchiveFileState,
}

pub struct DiscoveryArchive {
    path: PathBuf,
    interfaces: BTreeMap<String, ArchivedRecord>,
}

pub struct DiscoveryArchiveRecord {
    id: String,
    record: ArchivedRecord,
}

impl From<&DiscoveryRecord> for DiscoveryArchiveRecord {
    fn from(record: &DiscoveryRecord) -> Self {
        Self {
            id: file::encode_hex(record.id().as_bytes()),
            record: ArchivedRecord::from_record(record),
        }
    }
}

impl DiscoveryArchive {
    pub fn load(path: PathBuf) -> Result<LoadedDiscoveryArchive, DiscoveryArchiveError> {
        let (mut document, file_state) = match fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > MAX_ARCHIVE_BYTES {
                    return Err(DiscoveryArchiveError::TooLarge {
                        path,
                        bytes: bytes.len(),
                        maximum: MAX_ARCHIVE_BYTES,
                    });
                }
                let document =
                    serde_json::from_slice::<ArchiveDocument>(&bytes).map_err(|source| {
                        DiscoveryArchiveError::Decode {
                            path: path.clone(),
                            source,
                        }
                    })?;
                (document, DiscoveryArchiveFileState::Present)
            }
            Err(source) if source.kind() == ErrorKind::NotFound => {
                (ArchiveDocument::empty(), DiscoveryArchiveFileState::Missing)
            }
            Err(source) => {
                return Err(DiscoveryArchiveError::read(path, source));
            }
        };
        if document.format_version != FORMAT_VERSION {
            return Err(DiscoveryArchiveError::UnsupportedFormat {
                path,
                found: document.format_version,
                supported: FORMAT_VERSION,
            });
        }

        let mut catalog = DiscoveryCatalog::new();
        for (id, record) in &mut document.interfaces {
            let seed =
                record
                    .to_seed(id)
                    .map_err(|source| DiscoveryArchiveError::InvalidRecord {
                        path: path.clone(),
                        id: id.clone(),
                        source,
                    })?;
            record.refresh_manual_configuration(&seed.interface);
            catalog
                .restore(seed)
                .map_err(|source| DiscoveryArchiveError::InvalidRecord {
                    path: path.clone(),
                    id: id.clone(),
                    source: ArchiveRecordError::Catalog(source),
                })?;
        }

        Ok(LoadedDiscoveryArchive {
            archive: Self {
                path,
                interfaces: document.interfaces,
            },
            catalog,
            file_state,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.interfaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.interfaces.is_empty()
    }

    pub fn record(
        &mut self,
        record: impl Into<DiscoveryArchiveRecord>,
    ) -> Result<(), DiscoveryArchiveError> {
        let DiscoveryArchiveRecord {
            id,
            record: mut updated,
        } = record.into();
        if let Some(previous) = self.interfaces.get(&id) {
            if !updated.merge_history_from(previous) {
                return Ok(());
            }
        }
        let previous = self.interfaces.insert(id.clone(), updated);
        if let Err(error) = self.persist() {
            match previous {
                Some(previous) => {
                    self.interfaces.insert(id, previous);
                }
                None => {
                    self.interfaces.remove(&id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn persist(&self) -> Result<(), DiscoveryArchiveError> {
        let document = ArchiveDocumentRef {
            format_version: FORMAT_VERSION,
            configuration_note: CONFIGURATION_NOTE,
            interfaces: &self.interfaces,
        };
        let mut bytes = serde_json::to_vec_pretty(&document).map_err(|source| {
            DiscoveryArchiveError::Encode {
                path: self.path.clone(),
                source,
            }
        })?;
        bytes.push(b'\n');
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err(DiscoveryArchiveError::TooLarge {
                path: self.path.clone(),
                bytes: bytes.len(),
                maximum: MAX_ARCHIVE_BYTES,
            });
        }
        replace_archive_file(&self.path, &bytes)
    }
}
