use std::{
    fs::{self, File},
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::filesystem::{private_directory, private_file, remove_directory, write_synced};

use crate::{
    Activation, Board, Budgets, CandidateId, Fallback, LaunchBudget, PackageError, Slot, Status,
    VerifiedPackage,
};

const JOURNAL_LIMIT: u64 = 16384;
const MANIFEST_LIMIT: u64 = 4096;
const SIGNATURE_LIMIT: u64 = 4096;
const SLOT_METADATA_HEADROOM: u64 = 32768;

pub struct SpaceBudget {
    pub available_bytes: u64,
    pub reserve_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkpoint {
    PackageFileSynced,
    SlotPublished,
    JournalFileSynced,
    JournalRenamed,
    JournalDirectorySynced,
    ExecutablePublished,
}

pub trait ObserveWrites {
    fn reached(&mut self, checkpoint: Checkpoint) -> std::io::Result<()>;
}

pub struct UnobservedWrites;

impl ObserveWrites for UnobservedWrites {
    fn reached(&mut self, _: Checkpoint) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("appliance I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("installation is locked: {0}")]
    Locked(std::fs::TryLockError),
    #[error("invalid activation metadata: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error("no valid activation journal remains")]
    CorruptJournal,
    #[error("revision exhausted")]
    RevisionExhausted,
    #[error("candidate cannot be staged during a trial")]
    TrialInProgress,
    #[error("confirmation does not name the current trial")]
    StaleConfirmation,
    #[error("no confirmed application is available")]
    NoApplication,
    #[error("metadata or artifact exceeds its read budget")]
    ReadBudget,
    #[error("signature document is not UTF-8 text")]
    SignatureEncoding,
    #[error("application package: {0}")]
    Package(#[from] PackageError),
    #[error("selected slot differs from its activation record")]
    SlotMismatch,
    #[error("a write failed; reopen the installation before continuing")]
    ReopenRequired,
    #[error("insufficient storage or RAM after reserving recovery headroom")]
    LowSpace,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    activation: Activation,
    sha256: String,
}

pub struct Appliance<'key, O = UnobservedWrites> {
    root: PathBuf,
    key: &'key str,
    board: Board,
    budgets: Budgets,
    activation: Activation,
    observer: O,
    _lock: File,
    writes: WriteAdmission,
}

enum WriteAdmission {
    Ready,
    ReopenRequired,
}

impl<'key, O: ObserveWrites> Appliance<'key, O> {
    pub fn open(
        root: &Path,
        key: &'key str,
        board: Board,
        budgets: Budgets,
        observer: O,
    ) -> Result<Self, Error> {
        private_directory(root)?;
        let lock = private_file(&root.join("activation.lock"))?;
        lock.try_lock().map_err(Error::Locked)?;
        private_directory(&root.join("slots"))?;
        let activation = load_journal(root)?;
        Ok(Self {
            root: root.to_owned(),
            key,
            board,
            budgets,
            activation,
            observer,
            _lock: lock,
            writes: WriteAdmission::Ready,
        })
    }

    pub fn activation(&self) -> &Activation {
        &self.activation
    }

    pub fn verify_directory(&self, directory: &Path) -> Result<VerifiedPackage, Error> {
        let manifest = bounded_read(&directory.join("manifest.json"), MANIFEST_LIMIT)?;
        let signature = String::from_utf8(bounded_read(
            &directory.join("manifest.minisig"),
            SIGNATURE_LIMIT,
        )?)
        .map_err(|_| Error::SignatureEncoding)?;
        let compressed = bounded_read(&directory.join("app.gz"), self.budgets.compressed_bytes)?;
        Ok(VerifiedPackage::verify(
            manifest,
            signature,
            compressed,
            self.key,
            &self.board,
            &self.budgets,
        )?)
    }

    pub fn stage(
        &mut self,
        package: VerifiedPackage,
        launches: LaunchBudget,
        space: SpaceBudget,
    ) -> Result<CandidateId, Error> {
        self.admit_write()?;
        let package = VerifiedPackage::verify(
            package.manifest,
            package.signature,
            package.compressed,
            self.key,
            &self.board,
            &self.budgets,
        )?;
        let (slot, fallback) = match &self.activation.status {
            Status::Uninstalled => (Slot::A, Fallback::NoneInstalled),
            Status::Confirmed { slot, candidate } => (
                slot.other(),
                Fallback::Confirmed {
                    slot: slot.clone(),
                    candidate: candidate.clone(),
                },
            ),
            Status::Trial { .. } => return Err(Error::TrialInProgress),
        };
        let revision = self.next_revision()?;
        let candidate = CandidateId {
            revision: NonZeroU64::new(revision).ok_or(Error::RevisionExhausted)?,
            executable_sha256: package
                .executable_digest()
                .to_owned()
                .try_into()
                .map_err(|_| PackageError::Domain)?,
        };
        let slots = self.root.join("slots");
        let staging = slots.join("staging");
        let destination = slots.join(slot.directory());
        let mut reclaimable = 0_u64;
        for directory in [&destination, &staging] {
            for name in ["manifest.json", "manifest.minisig", "app.gz"] {
                match fs::metadata(directory.join(name)) {
                    Ok(metadata) => reclaimable = reclaimable.saturating_add(metadata.len()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let needed = (package.compressed.len() as u64)
            .saturating_add(SLOT_METADATA_HEADROOM)
            .saturating_add(space.reserve_bytes);
        if needed > space.available_bytes.saturating_add(reclaimable) {
            return Err(Error::LowSpace);
        }
        self.writes = WriteAdmission::ReopenRequired;
        remove_directory(&staging)?;
        remove_directory(&destination)?;
        File::open(&slots)?.sync_all()?;
        private_directory(&staging)?;
        for (name, bytes) in [
            ("manifest.json", package.manifest.as_slice()),
            ("manifest.minisig", package.signature.as_bytes()),
            ("app.gz", package.compressed.as_slice()),
        ] {
            write_synced(&staging.join(name), bytes)?;
            self.observer.reached(Checkpoint::PackageFileSynced)?;
        }
        File::open(&staging)?.sync_all()?;
        fs::rename(&staging, destination)?;
        File::open(&slots)?.sync_all()?;
        self.observer.reached(Checkpoint::SlotPublished)?;
        self.commit(Activation {
            revision,
            status: Status::Trial {
                slot,
                candidate: candidate.clone(),
                fallback,
                launches_remaining: launches.get(),
            },
        })?;
        Ok(candidate)
    }

    pub fn confirm(&mut self, candidate: &CandidateId) -> Result<(), Error> {
        self.admit_write()?;
        let slot = match &self.activation.status {
            Status::Trial {
                slot,
                candidate: current,
                ..
            } if current == candidate => slot.clone(),
            _ => return Err(Error::StaleConfirmation),
        };
        self.load_slot(&slot, candidate)?;
        self.commit(Activation {
            revision: self.next_revision()?,
            status: Status::Confirmed {
                slot,
                candidate: candidate.clone(),
            },
        })
    }

    pub fn rollback(&mut self) -> Result<(), Error> {
        self.admit_write()?;
        let fallback = match &self.activation.status {
            Status::Trial { fallback, .. } => fallback.clone(),
            Status::Uninstalled | Status::Confirmed { .. } => return Err(Error::NoApplication),
        };
        let status = match fallback {
            Fallback::NoneInstalled => Status::Uninstalled,
            Fallback::Confirmed { slot, candidate } => {
                self.load_slot(&slot, &candidate)?;
                Status::Confirmed { slot, candidate }
            }
        };
        self.commit(Activation {
            revision: self.next_revision()?,
            status,
        })
    }

    pub fn prepare_launch(
        &mut self,
        ram_directory: &Path,
        space: SpaceBudget,
    ) -> Result<(PathBuf, CandidateId), Error> {
        self.admit_write()?;
        let (slot, candidate) = match self.activation.status.clone() {
            Status::Uninstalled => return Err(Error::NoApplication),
            Status::Confirmed { slot, candidate } => (slot, candidate),
            Status::Trial {
                launches_remaining: 0,
                ..
            } => {
                self.rollback()?;
                return self.prepare_launch(ram_directory, space);
            }
            Status::Trial {
                slot,
                candidate,
                fallback,
                launches_remaining,
            } => {
                if self.load_slot(&slot, &candidate).is_err() {
                    self.rollback()?;
                    return self.prepare_launch(ram_directory, space);
                }
                self.commit(Activation {
                    revision: self.next_revision()?,
                    status: Status::Trial {
                        slot: slot.clone(),
                        candidate: candidate.clone(),
                        fallback,
                        launches_remaining: launches_remaining - 1,
                    },
                })?;
                (slot, candidate)
            }
        };
        let package = self.load_slot(&slot, &candidate)?;
        if package
            .executable_bytes()
            .saturating_add(space.reserve_bytes)
            > space.available_bytes
        {
            return Err(Error::LowSpace);
        }
        private_directory(ram_directory)?;
        let temporary = ram_directory.join("app.new");
        let destination = ram_directory.join("app");
        let mut file = private_file(&temporary)?;
        file.set_len(0)?;
        package.expand(&mut file)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
        fs::rename(&temporary, &destination)?;
        File::open(ram_directory)?.sync_all()?;
        self.observer.reached(Checkpoint::ExecutablePublished)?;
        Ok((destination, candidate))
    }

    fn load_slot(&self, slot: &Slot, candidate: &CandidateId) -> Result<VerifiedPackage, Error> {
        let package = self.verify_directory(&self.root.join("slots").join(slot.directory()))?;
        if package.executable_digest() != candidate.executable_sha256.as_str() {
            return Err(Error::SlotMismatch);
        }
        Ok(package)
    }

    fn next_revision(&self) -> Result<u64, Error> {
        self.activation
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }

    fn admit_write(&self) -> Result<(), Error> {
        match self.writes {
            WriteAdmission::Ready => Ok(()),
            WriteAdmission::ReopenRequired => Err(Error::ReopenRequired),
        }
    }

    fn commit(&mut self, activation: Activation) -> Result<(), Error> {
        self.writes = WriteAdmission::ReopenRequired;
        let bytes = serde_json::to_vec(&activation)?;
        let journal = Journal {
            sha256: prns_flash_manifest::sha256_hex(&bytes),
            activation,
        };
        let temporary = self.root.join("journal.new");
        write_synced(&temporary, &serde_json::to_vec(&journal)?)?;
        self.observer.reached(Checkpoint::JournalFileSynced)?;
        let bank = journal.activation.revision % 2;
        fs::rename(temporary, self.root.join(format!("journal-{bank}.json")))?;
        // Publication may precede a failed directory sync. Refuse further writes
        // through this object; reopening reconciles the authoritative journals.
        self.activation = journal.activation;
        self.observer.reached(Checkpoint::JournalRenamed)?;
        File::open(&self.root)?.sync_all()?;
        self.observer.reached(Checkpoint::JournalDirectorySynced)?;
        self.writes = WriteAdmission::Ready;
        Ok(())
    }
}

fn load_journal(root: &Path) -> Result<Activation, Error> {
    let mut valid = Vec::new();
    let mut present = 0;
    for bank in 0..2 {
        let path = root.join(format!("journal-{bank}.json"));
        let bytes = match bounded_read(&path, JOURNAL_LIMIT) {
            Ok(bytes) => {
                present += 1;
                bytes
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let journal: Journal = match serde_json::from_slice(&bytes) {
            Ok(journal) => journal,
            Err(_) => continue,
        };
        let checksum = prns_flash_manifest::sha256_hex(&serde_json::to_vec(&journal.activation)?);
        if checksum == journal.sha256 && journal.activation.is_consistent() {
            valid.push(journal.activation);
        }
    }
    valid.sort_by_key(Activation::revision);
    match valid.pop() {
        Some(activation) => Ok(activation),
        None if present == 0
            && !root.join("slots/a").exists()
            && !root.join("slots/b").exists() =>
        {
            Ok(Activation::empty())
        }
        None => Err(Error::CorruptJournal),
    }
}

fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    match crate::filesystem::bounded_read(path, limit) {
        Ok(bytes) => Ok(bytes),
        Err(crate::filesystem::ReadError::Io(error)) => Err(error.into()),
        Err(crate::filesystem::ReadError::Budget) => Err(Error::ReadBudget),
    }
}

#[cfg(all(test, unix))]
mod tests;
