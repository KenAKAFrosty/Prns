use std::{
    fs::{self, File},
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    filesystem::{self, private_directory, private_file, remove_directory, write_synced},
    Board, RadioPlan, RadioProfile, RadioProfileError, SpaceBudget, UnobservedWrites,
};

mod recovery;
mod types;
pub use types::*;

const JOURNAL_LIMIT: u64 = 32768;
const METADATA_HEADROOM: u64 = 32768;

#[derive(Debug, thiserror::Error)]
pub enum RadioTransactionError {
    #[error("radio transaction I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("radio transaction is locked: {0}")]
    Locked(std::fs::TryLockError),
    #[error("invalid radio transaction metadata: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error(transparent)]
    Profile(#[from] RadioProfileError),
    #[error("invalid boot identity")]
    BootIdentity,
    #[error("invalid SHA-256 digest")]
    Digest,
    #[error("recovery window must be 1..=3600 seconds")]
    RecoveryWindow,
    #[error("trial boot allowance must be 1..=3")]
    TrialBoots,
    #[error("invalid Unix permission bits")]
    FileMode,
    #[error("radio metadata or file exceeds its budget")]
    ReadBudget,
    #[error("no valid radio journal or recovery guard remains")]
    CorruptJournal,
    #[error("radio recovery backup is absent or corrupt")]
    BackupIntegrity,
    #[error("radio transaction revision exhausted")]
    RevisionExhausted,
    #[error("a radio transaction is already pending recovery or confirmation")]
    InProgress,
    #[error("operation does not name the current radio transaction")]
    StaleCandidate,
    #[error("operation is invalid for this radio phase")]
    Phase,
    #[error("radio recovery lease expired or belongs to another boot")]
    LeaseExpired,
    #[error("vendor radio files changed after preparation")]
    ConfigurationChanged,
    #[error("pending UCI changes must be resolved before radio activation")]
    PendingConfiguration,
    #[error("independent persistent recovery service is unavailable")]
    RecoveryProtection,
    #[error("vendor radio operation failed: {0:?}")]
    Vendor(RadioVendorOperation),
    #[error("insufficient storage after recovery reserve")]
    LowSpace,
    #[error("a write failed; reopen the radio transaction before continuing")]
    ReopenRequired,
}

#[derive(Debug)]
pub enum RadioVendorOperation {
    Inspection,
    Staging,
    Reload,
    Reboot,
    Readiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioCheckpoint {
    BackupFileSynced(RadioFile),
    CandidateFileSynced(RadioFile),
    GuardFileSynced,
    GuardDirectorySynced,
    GuardRetired,
    GuardRenamed,
    GuardPublished,
    JournalFileSynced,
    JournalRenamed,
    JournalDirectorySynced,
    MutationIntentSynced,
    VendorFileReplaced(RadioFile),
    RadioReloaded,
}

pub trait ObserveRadioWrites {
    fn reached(&mut self, checkpoint: RadioCheckpoint) -> std::io::Result<()>;
}

impl ObserveRadioWrites for UnobservedWrites {
    fn reached(&mut self, _: RadioCheckpoint) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    digest: RadioDigest,
    mode: FileMode,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Guard {
    candidate: RadioCandidateId,
    board: Board,
    boot_adapter_sha256: String,
    profile: RadioProfile,
    window: RecoveryWindow,
    initial_lease: RadioLease,
    original: [Descriptor; 4],
    configured: [Descriptor; 2],
}

impl Guard {
    fn plan(&self) -> Result<RadioPlan, RadioTransactionError> {
        let expected = match self.board {
            Board::ThinkNodeG4 => super::G4_BOOT_ADAPTER_SHA256,
            Board::HeltecHtHd01V2 => super::HELTEC_BOOT_ADAPTER_SHA256,
        };
        if self.boot_adapter_sha256 != expected
            || self.candidate.profile_sha256 != RadioDigest::of(&serde_json::to_vec(&self.profile)?)
            || !(1..=types::MAX_TRIAL_BOOTS).contains(&self.initial_lease.trial_boots_remaining)
            || self.initial_lease.expires_at_uptime_seconds == 0
        {
            return Err(RadioTransactionError::BackupIntegrity);
        }
        Ok(self
            .profile
            .qualified_plan(&self.board, self.boot_adapter_sha256.clone()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed<T> {
    value: T,
    sha256: RadioDigest,
}

enum GuardPresence {
    Absent,
    Present(Box<Guard>),
}
enum WriteAdmission {
    Ready,
    ReopenRequired,
}

pub struct RadioTransaction<O = UnobservedWrites> {
    root: PathBuf,
    status: RadioTransactionStatus,
    guard: GuardPresence,
    observer: O,
    writes: WriteAdmission,
    _lock: File,
}

impl<O: ObserveRadioWrites> RadioTransaction<O> {
    pub fn open(root: &Path, observer: O) -> Result<Self, RadioTransactionError> {
        private_directory(root)?;
        let lock = private_file(&root.join("radio.lock"))?;
        lock.try_lock().map_err(RadioTransactionError::Locked)?;
        let guard = match read_sealed::<Guard>(&root.join("active/guard.json")) {
            Ok(guard) => {
                guard.plan()?;
                GuardPresence::Present(Box::new(guard))
            }
            Err(RadioTransactionError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                GuardPresence::Absent
            }
            Err(_) => return Err(RadioTransactionError::BackupIntegrity),
        };
        let mut journals = Vec::new();
        let mut present = 0;
        for bank in 0..2 {
            match read_sealed::<RadioTransactionStatus>(&root.join(format!("journal-{bank}.json")))
            {
                Ok(status) => {
                    present += 1;
                    if consistent(&status) {
                        journals.push(status);
                    }
                }
                Err(RadioTransactionError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => present += 1,
            }
        }
        journals.sort_by_key(|status| status.revision);
        let mut status = match (journals.pop(), &guard) {
            (Some(status), GuardPresence::Present(guard)) => match &status.state {
                RadioTransactionState::Active { candidate, .. }
                    if candidate == &guard.candidate =>
                {
                    status
                }
                _ if status.revision < guard.candidate.revision.get() => RadioTransactionStatus {
                    revision: guard.candidate.revision.get(),
                    state: RadioTransactionState::Active {
                        candidate: guard.candidate.clone(),
                        phase: RadioPhase::Prepared {
                            lease: guard.initial_lease.clone(),
                        },
                    },
                },
                _ => return Err(RadioTransactionError::BackupIntegrity),
            },
            (Some(status), GuardPresence::Absent) => match &status.state {
                RadioTransactionState::Empty => status,
                RadioTransactionState::Active { phase, .. } if phase.is_terminal() => status,
                RadioTransactionState::Active { .. } => {
                    return Err(RadioTransactionError::BackupIntegrity)
                }
            },
            (None, GuardPresence::Present(guard)) => RadioTransactionStatus {
                revision: guard.candidate.revision.get(),
                state: RadioTransactionState::Active {
                    candidate: guard.candidate.clone(),
                    phase: RadioPhase::Restoring,
                },
            },
            (None, GuardPresence::Absent) if present == 0 && !root.join("active").exists() => {
                RadioTransactionStatus {
                    revision: 0,
                    state: RadioTransactionState::Empty,
                }
            }
            (None, GuardPresence::Absent) => return Err(RadioTransactionError::CorruptJournal),
        };
        let intent = root.join("active/mutation-intent.json");
        match read_sealed::<RadioCandidateId>(&intent) {
            Ok(candidate) => {
                if !matches!(&guard, GuardPresence::Present(guard) if guard.candidate == candidate)
                {
                    return Err(RadioTransactionError::BackupIntegrity);
                }
                if let RadioTransactionState::Active {
                    phase: RadioPhase::Prepared { .. },
                    ..
                } = &status.state
                {
                    status.state = RadioTransactionState::Active {
                        candidate,
                        phase: RadioPhase::Restoring,
                    };
                }
            }
            Err(RadioTransactionError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                let GuardPresence::Present(guard) = &guard else {
                    return Err(RadioTransactionError::BackupIntegrity);
                };
                if !matches!(&status.state, RadioTransactionState::Active { phase, .. } if phase.is_terminal())
                {
                    status.state = RadioTransactionState::Active {
                        candidate: guard.candidate.clone(),
                        phase: RadioPhase::Restoring,
                    };
                }
            }
        }
        Ok(Self {
            root: root.to_owned(),
            status,
            guard,
            observer,
            writes: WriteAdmission::Ready,
            _lock: lock,
        })
    }

    pub fn status(&self) -> &RadioTransactionStatus {
        &self.status
    }

    pub fn prepare(
        &mut self,
        platform: &mut impl RadioPlatform,
        profile: RadioProfile,
        window: RecoveryWindow,
        boots: TrialBoots,
        space: SpaceBudget,
    ) -> Result<RadioCandidateId, RadioTransactionError> {
        self.admit()?;
        match &self.status.state {
            RadioTransactionState::Empty => {}
            RadioTransactionState::Active { phase, .. } if phase.is_terminal() => {
                // Both banks must be terminal before the previous guard can be retired.
                self.commit(self.status.state.clone())?;
                self.commit(self.status.state.clone())?;
            }
            RadioTransactionState::Active { .. } => return Err(RadioTransactionError::InProgress),
        }
        let prepared = platform.prepare(&profile)?;
        if prepared.plan.profile() != &profile {
            return Err(RadioTransactionError::ConfigurationChanged);
        }
        platform.validate(&prepared.plan)?;
        let now = platform.time()?;
        let revision = self.next_revision()?;
        let candidate = RadioCandidateId {
            revision: NonZeroU64::new(revision).ok_or(RadioTransactionError::RevisionExhausted)?,
            profile_sha256: RadioDigest::of(&serde_json::to_vec(&profile)?),
        };
        let lease = RadioLease {
            boot: now.boot,
            expires_at_uptime_seconds: now
                .uptime_seconds
                .checked_add(u64::from(window.seconds()))
                .ok_or(RadioTransactionError::LeaseExpired)?,
            trial_boots_remaining: boots.get(),
        };
        let originals = RadioFile::ALL
            .iter()
            .map(|file| prepared.original.image(file))
            .collect::<Vec<_>>();
        let configured = [&prepared.candidate_wireless, &prepared.candidate_mesh11sd];
        let bytes = originals
            .iter()
            .map(|image| image.bytes.len())
            .chain(configured.iter().map(|bytes| bytes.len()))
            .try_fold(0_u64, |total, bytes| {
                if bytes as u64 > MAX_FILE_BYTES {
                    return Err(RadioTransactionError::ReadBudget);
                }
                total
                    .checked_add(bytes as u64)
                    .ok_or(RadioTransactionError::LowSpace)
            })?;
        if bytes
            .saturating_add(METADATA_HEADROOM)
            .saturating_add(space.reserve_bytes)
            > space.available_bytes
        {
            return Err(RadioTransactionError::LowSpace);
        }
        let guard = Guard {
            candidate: candidate.clone(),
            board: prepared.plan.board().clone(),
            boot_adapter_sha256: prepared.plan.boot_adapter_sha256().to_owned(),
            profile,
            window,
            initial_lease: lease.clone(),
            original: std::array::from_fn(|index| Descriptor {
                digest: RadioDigest::of(&originals[index].bytes),
                mode: originals[index].mode.clone(),
            }),
            configured: std::array::from_fn(|index| Descriptor {
                digest: RadioDigest::of(configured[index]),
                mode: originals[index].mode.clone(),
            }),
        };
        self.writes = WriteAdmission::ReopenRequired;
        let staging = self.root.join("staging");
        remove_directory(&staging)?;
        private_directory(&staging)?;
        for (file, image) in RadioFile::ALL.iter().zip(originals) {
            write_synced(
                &staging.join(format!("original-{}", file.name())),
                &image.bytes,
            )?;
            self.observer
                .reached(RadioCheckpoint::BackupFileSynced(file.clone()))?;
        }
        for (file, bytes) in RadioFile::CANDIDATE.iter().zip(configured) {
            write_synced(&staging.join(format!("candidate-{}", file.name())), bytes)?;
            self.observer
                .reached(RadioCheckpoint::CandidateFileSynced(file.clone()))?;
        }
        write_sealed(&staging.join("guard.json"), &guard)?;
        self.observer.reached(RadioCheckpoint::GuardFileSynced)?;
        File::open(&staging)?.sync_all()?;
        self.observer
            .reached(RadioCheckpoint::GuardDirectorySynced)?;
        remove_directory(&self.root.join("active"))?;
        File::open(&self.root)?.sync_all()?;
        self.observer.reached(RadioCheckpoint::GuardRetired)?;
        fs::rename(staging, self.root.join("active"))?;
        self.observer.reached(RadioCheckpoint::GuardRenamed)?;
        File::open(&self.root)?.sync_all()?;
        self.observer.reached(RadioCheckpoint::GuardPublished)?;
        self.guard = GuardPresence::Present(Box::new(guard));
        self.commit(RadioTransactionState::Active {
            candidate: candidate.clone(),
            phase: RadioPhase::Prepared { lease },
        })?;
        Ok(candidate)
    }

    pub fn apply(
        &mut self,
        platform: &mut impl RadioPlatform,
        candidate: &RadioCandidateId,
    ) -> Result<(), RadioTransactionError> {
        self.admit()?;
        let lease = match self.phase(candidate)? {
            RadioPhase::Prepared { lease } => lease.clone(),
            _ => return Err(RadioTransactionError::Phase),
        };
        check_lease(&lease, &platform.time()?)?;
        let plan = self.guard()?.plan()?;
        platform.validate(&plan)?;
        platform.require_recovery_service()?;
        for file in RadioFile::ALL {
            let original = self.image(&file, ImageSource::Original)?;
            if platform.read(&file)? != original {
                return Err(RadioTransactionError::ConfigurationChanged);
            }
        }
        // Validate every rollback and candidate artifact before the first host mutation.
        let images = RadioFile::CANDIDATE
            .iter()
            .map(|file| self.image(file, ImageSource::Candidate))
            .collect::<Result<Vec<_>, _>>()?;
        self.writes = WriteAdmission::ReopenRequired;
        write_sealed(&self.root.join("active/mutation-intent.json"), candidate)?;
        File::open(self.root.join("active"))?.sync_all()?;
        self.observer
            .reached(RadioCheckpoint::MutationIntentSynced)?;
        self.set_phase(
            candidate,
            RadioPhase::Applying {
                lease: lease.clone(),
            },
        )?;
        self.writes = WriteAdmission::ReopenRequired;
        for (file, image) in RadioFile::CANDIDATE.iter().zip(images) {
            platform.replace_synced(file, &image)?;
            self.observer
                .reached(RadioCheckpoint::VendorFileReplaced(file.clone()))?;
        }
        platform.reload(&plan.profile().binding.radio)?;
        self.observer.reached(RadioCheckpoint::RadioReloaded)?;
        self.set_phase(candidate, RadioPhase::Trial { lease })
    }

    pub fn confirm(
        &mut self,
        platform: &mut impl RadioPlatform,
        candidate: &RadioCandidateId,
    ) -> Result<(), RadioTransactionError> {
        self.admit()?;
        let lease = match self.phase(candidate)? {
            RadioPhase::Trial { lease } => lease.clone(),
            _ => return Err(RadioTransactionError::Phase),
        };
        check_lease(&lease, &platform.time()?)?;
        let plan = self.guard()?.plan()?;
        platform.validate(&plan)?;
        for file in RadioFile::CANDIDATE {
            if platform.read(&file)? != self.image(&file, ImageSource::Candidate)? {
                return Err(RadioTransactionError::ConfigurationChanged);
            }
        }
        self.set_phase(candidate, RadioPhase::Confirmed)
    }

    pub fn rollback(&mut self, candidate: &RadioCandidateId) -> Result<(), RadioTransactionError> {
        self.admit()?;
        let phase = match self.phase(candidate)? {
            RadioPhase::Prepared { .. } => RadioPhase::CanceledBeforeApply,
            RadioPhase::Applying { .. } | RadioPhase::Trial { .. } => RadioPhase::Restoring,
            _ => return Err(RadioTransactionError::Phase),
        };
        self.set_phase(candidate, phase)
    }

    fn guard(&self) -> Result<&Guard, RadioTransactionError> {
        match &self.guard {
            GuardPresence::Present(guard) => Ok(guard),
            GuardPresence::Absent => Err(RadioTransactionError::BackupIntegrity),
        }
    }
    fn phase(&self, candidate: &RadioCandidateId) -> Result<&RadioPhase, RadioTransactionError> {
        match &self.status.state {
            RadioTransactionState::Active {
                candidate: current,
                phase,
            } if current == candidate => Ok(phase),
            _ => Err(RadioTransactionError::StaleCandidate),
        }
    }
    fn image(
        &self,
        file: &RadioFile,
        source: ImageSource,
    ) -> Result<RadioFileImage, RadioTransactionError> {
        let index = RadioFile::ALL
            .iter()
            .position(|kind| kind == file)
            .ok_or(RadioTransactionError::BackupIntegrity)?;
        let (prefix, descriptor) = match source {
            ImageSource::Original => ("original", &self.guard()?.original[index]),
            ImageSource::Candidate => (
                "candidate",
                self.guard()?
                    .configured
                    .get(index)
                    .ok_or(RadioTransactionError::BackupIntegrity)?,
            ),
        };
        let bytes = read_bounded(
            &self.root.join(format!("active/{prefix}-{}", file.name())),
            MAX_FILE_BYTES,
        )?;
        if RadioDigest::of(&bytes) != descriptor.digest {
            return Err(RadioTransactionError::BackupIntegrity);
        }
        Ok(RadioFileImage {
            bytes,
            mode: descriptor.mode.clone(),
        })
    }
    fn admit(&self) -> Result<(), RadioTransactionError> {
        match self.writes {
            WriteAdmission::Ready => Ok(()),
            WriteAdmission::ReopenRequired => Err(RadioTransactionError::ReopenRequired),
        }
    }
    fn next_revision(&self) -> Result<u64, RadioTransactionError> {
        self.status
            .revision
            .checked_add(1)
            .ok_or(RadioTransactionError::RevisionExhausted)
    }
    fn set_phase(
        &mut self,
        candidate: &RadioCandidateId,
        phase: RadioPhase,
    ) -> Result<(), RadioTransactionError> {
        self.commit(RadioTransactionState::Active {
            candidate: candidate.clone(),
            phase,
        })
    }
    fn commit(&mut self, state: RadioTransactionState) -> Result<(), RadioTransactionError> {
        let status = RadioTransactionStatus {
            revision: self.next_revision()?,
            state,
        };
        self.writes = WriteAdmission::ReopenRequired;
        let temporary = self.root.join("journal.new");
        write_sealed(&temporary, &status)?;
        self.observer.reached(RadioCheckpoint::JournalFileSynced)?;
        fs::rename(
            temporary,
            self.root
                .join(format!("journal-{}.json", status.revision % 2)),
        )?;
        self.status = status;
        self.observer.reached(RadioCheckpoint::JournalRenamed)?;
        File::open(&self.root)?.sync_all()?;
        self.observer
            .reached(RadioCheckpoint::JournalDirectorySynced)?;
        self.writes = WriteAdmission::Ready;
        Ok(())
    }
}

enum ImageSource {
    Original,
    Candidate,
}

fn consistent(status: &RadioTransactionStatus) -> bool {
    match &status.state {
        RadioTransactionState::Empty => status.revision == 0,
        RadioTransactionState::Active { candidate, phase } => {
            candidate.revision.get() <= status.revision
                && match phase {
                    RadioPhase::Prepared { lease }
                    | RadioPhase::Applying { lease }
                    | RadioPhase::Trial { lease } => {
                        lease.trial_boots_remaining <= types::MAX_TRIAL_BOOTS
                            && lease.expires_at_uptime_seconds > 0
                    }
                    RadioPhase::CheckingRestoration { lease } => {
                        lease.trial_boots_remaining == 0 && lease.expires_at_uptime_seconds > 0
                    }
                    RadioPhase::Confirmed
                    | RadioPhase::CanceledBeforeApply
                    | RadioPhase::Restoring
                    | RadioPhase::RebootRequired { .. }
                    | RadioPhase::RebootUnavailable { .. }
                    | RadioPhase::Restored
                    | RadioPhase::RestorationUnavailable => true,
                }
        }
    }
}

fn check_lease(lease: &RadioLease, now: &BootTime) -> Result<(), RadioTransactionError> {
    if lease.boot != now.boot || now.uptime_seconds >= lease.expires_at_uptime_seconds {
        return Err(RadioTransactionError::LeaseExpired);
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, RadioTransactionError> {
    match filesystem::bounded_read(path, limit) {
        Ok(bytes) => Ok(bytes),
        Err(filesystem::ReadError::Io(error)) => Err(error.into()),
        Err(filesystem::ReadError::Budget) => Err(RadioTransactionError::ReadBudget),
    }
}

fn read_sealed<T: serde::de::DeserializeOwned + Serialize>(
    path: &Path,
) -> Result<T, RadioTransactionError> {
    let sealed: Sealed<T> = serde_json::from_slice(&read_bounded(path, JOURNAL_LIMIT)?)?;
    if RadioDigest::of(&serde_json::to_vec(&sealed.value)?) != sealed.sha256 {
        return Err(RadioTransactionError::CorruptJournal);
    }
    Ok(sealed.value)
}

fn write_sealed<T: Serialize>(path: &Path, value: &T) -> Result<(), RadioTransactionError> {
    let sealed = Sealed {
        sha256: RadioDigest::of(&serde_json::to_vec(value)?),
        value,
    };
    write_synced(path, &serde_json::to_vec(&sealed)?)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
