//! Filesystem owner for the portable shared announce scheduler.
//!
//! A dedicated locked directory holds a checksummed atomic snapshot. This is
//! scheduling metadata, not delivery custody. Pending in-memory requests can
//! be lost on restart; a reserved attempt can never become an unspent budget.

#[cfg(feature = "owner")]
mod client;
#[cfg(feature = "owner")]
pub use client::IdentityAnnounceClient as Client;
#[cfg(feature = "owner")]
pub use client::{IdentityAnnounceClient, Job, Reply, Request, Submit};

/// Persisted unsolicited announcement policy. None means manual-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityAnnounceSettings"))]
pub struct Settings {
    pub interval_millis: Option<u64>,
}
impl Settings {
    pub fn validate(self) -> std::result::Result<(), prns_identities_core::ApiError> {
        use prns_identities_runtime::announce::{COOLDOWN_MS, MAX_INTERVAL_MS};
        if self
            .interval_millis
            .is_some_and(|value| !(COOLDOWN_MS..=MAX_INTERVAL_MS).contains(&value))
        {
            return Err(prns_identities_core::ApiError::InvalidInput);
        }
        Ok(())
    }
}

use crate::{files, Error as StoreError};
use prns_identities_core::{LocalIdentityId, ServiceId};
pub use prns_identities_runtime::announce::{
    AnnounceKey, Configuration, Error, ManualRequest, Reservation,
};
use prns_identities_runtime::announce::{Attempt, Entry, Runtime, Scheduler, Snapshot, Storage};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"PNIANN01";
const MAX_SLOTS: usize = 256;
type Result<T> = std::result::Result<T, Error<StoreError>>;

/// One instance per Reticulum host, shared across its local identities and
/// services. Never clone this owner or create independent stores for personas.
pub struct FileAnnounceScheduler<const N: usize> {
    storage: FileStorage<N>,
    snapshot: Snapshot<N>,
    scratch: Snapshot<N>,
    runtime: Runtime<N>,
}

impl<const N: usize> FileAnnounceScheduler<N> {
    pub fn create_new(path: impl AsRef<Path>, now_ms: u64, wall_ms: Option<u64>) -> Result<Self> {
        let storage = FileStorage::create_new(path.as_ref()).map_err(Error::Storage)?;
        Self::from_storage(storage, now_ms, wall_ms)
    }

    /// Existing corrupt/missing state is an error, never an empty scheduler.
    pub fn open(path: impl AsRef<Path>, now_ms: u64, wall_ms: Option<u64>) -> Result<Self> {
        let storage = FileStorage::open(path.as_ref()).map_err(Error::Storage)?;
        Self::from_storage(storage, now_ms, wall_ms)
    }

    fn from_storage(storage: FileStorage<N>, now_ms: u64, wall_ms: Option<u64>) -> Result<Self> {
        let mut owner = Self {
            storage,
            snapshot: Snapshot::empty(),
            scratch: Snapshot::empty(),
            runtime: Runtime::empty(),
        };
        owner.reload(now_ms, wall_ms)?;
        Ok(owner)
    }

    pub fn root(&self) -> &Path {
        &self.storage.root
    }
    pub fn snapshot(&self) -> &Snapshot<N> {
        &self.snapshot
    }

    /// Also recovers an owner fenced after an uncertain storage error. It
    /// starts the same fresh cooldown as a process restart.
    pub fn reload(&mut self, now_ms: u64, wall_ms: Option<u64>) -> Result<()> {
        self.controller().reload(now_ms, wall_ms)
    }

    pub fn configure(
        &mut self,
        key: AnnounceKey,
        channels: u64,
        interval_ms: Option<u64>,
        enabled: bool,
        now_ms: u64,
        wall_ms: Option<u64>,
    ) -> Result<()> {
        self.controller().configure(
            key,
            Configuration {
                channels,
                interval_ms,
                enabled,
            },
            now_ms,
            wall_ms,
        )
    }

    pub fn request_manual(
        &mut self,
        key: AnnounceKey,
        now_ms: u64,
        wall_ms: Option<u64>,
    ) -> Result<ManualRequest> {
        self.controller().request_manual(key, now_ms, wall_ms)
    }

    pub fn next_ready_at(&mut self, now_ms: u64) -> Result<Option<u64>> {
        self.controller().next_ready_at(now_ms)
    }

    pub fn reserve_next(
        &mut self,
        now_ms: u64,
        wall_ms: Option<u64>,
        eligible_interface: bool,
        jitter: u16,
    ) -> Result<Option<Reservation>> {
        self.controller()
            .reserve_next(now_ms, wall_ms, eligible_interface, jitter)
    }

    pub fn reserve_next_matching(
        &mut self,
        now_ms: u64,
        wall_ms: Option<u64>,
        eligible_interface: bool,
        jitter: u16,
        available: impl FnMut(AnnounceKey) -> bool,
    ) -> Result<Option<Reservation>> {
        self.controller().reserve_next_matching(
            now_ms,
            wall_ms,
            eligible_interface,
            jitter,
            available,
        )
    }

    pub fn complete(
        &mut self,
        reservation: Reservation,
        requested_mask: u64,
        failed_mask: u64,
    ) -> Result<()> {
        self.controller()
            .complete(reservation, requested_mask, failed_mask)
    }

    fn controller(&mut self) -> Scheduler<'_, N, FileStorage<N>> {
        Scheduler {
            storage: &mut self.storage,
            snapshot: &mut self.snapshot,
            scratch: &mut self.scratch,
            runtime: &mut self.runtime,
        }
    }
}

struct FileStorage<const N: usize> {
    root: PathBuf,
    lock: File,
}

impl<const N: usize> Drop for FileStorage<N> {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

impl<const N: usize> FileStorage<N> {
    fn create_new(path: &Path) -> crate::Result<Self> {
        Self::check_capacity()?;
        let parent = path
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let root = fs::canonicalize(parent)?.join(path.file_name().ok_or(StoreError::UnsafePath)?);
        files::new_directory(&root)?;
        let lock = files::new_file(&root.join("owner.lock"))?;
        lock.sync_all()?;
        lock.try_lock().map_err(crate::lock_error)?;
        files::sync_directory(&root)?;
        let storage = Self { root, lock };
        storage.write(&Snapshot::empty())?;
        Ok(storage)
    }

    fn open(path: &Path) -> crate::Result<Self> {
        Self::check_capacity()?;
        files::check_directory(path)?;
        let root = fs::canonicalize(path)?;
        let lock_path = root.join("owner.lock");
        if !files::check_file(&lock_path)? {
            return Err(StoreError::MissingCatalog);
        }
        let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
        lock.try_lock().map_err(crate::lock_error)?;
        Ok(Self { root, lock })
    }

    fn check_capacity() -> crate::Result<()> {
        if N == 0 || N > MAX_SLOTS {
            Err(StoreError::Bounds)
        } else {
            Ok(())
        }
    }

    fn maximum_body() -> usize {
        32 + N * 128
    }

    fn write(&self, snapshot: &Snapshot<N>) -> crate::Result<()> {
        snapshot.validate().map_err(|_| StoreError::Corrupt)?;
        let bytes = crate::wrap(MAGIC, &encode(snapshot))?;
        files::replace(
            &self.root.join("state.bin"),
            &self.root.join("state.next"),
            &bytes,
            "announces",
        )
    }
}

impl<const N: usize> Storage<N> for FileStorage<N> {
    type Error = StoreError;
    fn load(&mut self, into: &mut Snapshot<N>) -> crate::Result<()> {
        let bytes = files::read_bounded(
            &self.root.join("state.bin"),
            Self::maximum_body() + crate::ENVELOPE_OVERHEAD,
        )?
        .ok_or(StoreError::MissingCatalog)?;
        let body = crate::unwrap(&bytes, MAGIC, Self::maximum_body())?;
        decode(body, into)
    }

    fn compare_and_store(
        &mut self,
        expected_revision: u64,
        next: &Snapshot<N>,
    ) -> crate::Result<()> {
        let mut current = Snapshot::empty();
        self.load(&mut current)?;
        if current.revision != expected_revision {
            return Err(StoreError::RevisionMismatch {
                expected: expected_revision,
                found: current.revision,
            });
        }
        if expected_revision.checked_add(1) != Some(next.revision) {
            return Err(StoreError::Corrupt);
        }
        self.write(next)
    }
}

fn encode<const N: usize>(snapshot: &Snapshot<N>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32 + N * 128);
    bytes.extend_from_slice(&(N as u32).to_le_bytes());
    bytes.extend_from_slice(&snapshot.revision.to_le_bytes());
    bytes.extend_from_slice(&snapshot.last_sequence.to_le_bytes());
    bytes.extend_from_slice(&(snapshot.cursor as u32).to_le_bytes());
    for slot in snapshot.entries {
        bytes.push(u8::from(slot.is_some()));
        let Some(entry) = slot else { continue };
        bytes.extend_from_slice(&entry.key.identity.incarnation);
        bytes.extend_from_slice(&entry.key.identity.sequence.to_le_bytes());
        bytes.extend_from_slice(&entry.key.service.0.to_le_bytes());
        bytes.extend_from_slice(&entry.configuration.channels.to_le_bytes());
        put_optional(&mut bytes, entry.configuration.interval_ms);
        bytes.push(u8::from(entry.configuration.enabled));
        bytes.push(u8::from(entry.last_attempt.is_some()));
        if let Some(attempt) = entry.last_attempt {
            bytes.extend_from_slice(&attempt.sequence.to_le_bytes());
            put_optional(&mut bytes, attempt.wall_ms);
            bytes.extend_from_slice(&attempt.channels.to_le_bytes());
            bytes.extend_from_slice(&attempt.requested_mask.to_le_bytes());
            bytes.extend_from_slice(&attempt.failed_mask.to_le_bytes());
            bytes.push(u8::from(attempt.completed));
        }
    }
    bytes
}

fn put_optional(bytes: &mut Vec<u8>, value: Option<u64>) {
    bytes.push(u8::from(value.is_some()));
    if let Some(value) = value {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn decode<const N: usize>(bytes: &[u8], into: &mut Snapshot<N>) -> crate::Result<()> {
    let mut reader = Reader(bytes);
    if reader.u32()? as usize != N {
        return Err(StoreError::Bounds);
    }
    *into = Snapshot::empty();
    into.revision = reader.u64()?;
    into.last_sequence = reader.u64()?;
    into.cursor = reader.u32()? as usize;
    for slot in &mut into.entries {
        if !reader.boolean()? {
            continue;
        }
        let key = AnnounceKey {
            identity: LocalIdentityId {
                incarnation: reader.take()?,
                sequence: reader.u64()?,
            },
            service: ServiceId(reader.u32()?),
        };
        let configuration = Configuration {
            channels: reader.u64()?,
            interval_ms: reader.optional()?,
            enabled: reader.boolean()?,
        };
        let last_attempt = if reader.boolean()? {
            Some(Attempt {
                sequence: reader.u64()?,
                wall_ms: reader.optional()?,
                channels: reader.u64()?,
                requested_mask: reader.u64()?,
                failed_mask: reader.u64()?,
                completed: reader.boolean()?,
            })
        } else {
            None
        };
        *slot = Some(Entry {
            key,
            configuration,
            last_attempt,
        });
    }
    if !reader.0.is_empty() {
        return Err(StoreError::Corrupt);
    }
    into.validate().map_err(|_| StoreError::Corrupt)
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take<const L: usize>(&mut self) -> crate::Result<[u8; L]> {
        let (value, remaining) = self.0.split_at_checked(L).ok_or(StoreError::Corrupt)?;
        self.0 = remaining;
        value.try_into().map_err(|_| StoreError::Corrupt)
    }
    fn boolean(&mut self) -> crate::Result<bool> {
        match self.take::<1>()?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StoreError::Corrupt),
        }
    }
    fn u64(&mut self) -> crate::Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> crate::Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn optional(&mut self) -> crate::Result<Option<u64>> {
        if self.boolean()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }
}
