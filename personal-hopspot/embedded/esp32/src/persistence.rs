use portable_atomic::{AtomicBool, Ordering};

#[cfg(target_arch = "xtensa")]
use allocator_api2::vec::Vec;
#[cfg(target_arch = "xtensa")]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(target_arch = "riscv32")]
use personal_rns::persistence::FlashArenaRange;
use personal_rns::persistence::FlashJournalLayout;
#[cfg(target_arch = "riscv32")]
use personal_rns::runtime::FixedRouteSnapshotKeys;
use personal_rns::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistencePolicy,
};
#[cfg(target_arch = "xtensa")]
use personal_rns::runtime::{RouteSnapshotKeyError, RouteSnapshotKeys, SharedNorFlash};
#[cfg(target_arch = "xtensa")]
use personal_rns::wire::DestinationHash;

use crate::flash::EspRomFlash;
#[cfg(target_arch = "xtensa")]
use crate::storage::{EngineStorageType, PsramAlloc};

#[cfg(target_arch = "riscv32")]
pub const C6_FLASH_CAPACITY: usize = 4 * 1024 * 1024;
#[cfg(target_arch = "xtensa")]
pub const S3_ARENA_BYTES: usize = 191 * 4096;
#[cfg(target_arch = "riscv32")]
pub const C6_ARENA_BYTES: usize = 15 * 4096;
#[cfg(target_arch = "riscv32")]
pub const C6_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0x3E0000, 0x3E1000],
    [
        FlashArenaRange::new(0x3E2000, 0x3F1000),
        FlashArenaRange::new(0x3F1000, 0x400000),
    ],
);

#[cfg(target_arch = "xtensa")]
const S3_PENDING: usize = 64;
#[cfg(target_arch = "riscv32")]
const C6_PENDING: usize = 32;

#[cfg(target_arch = "xtensa")]
pub type S3SharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, EspRomFlash>;
#[cfg(target_arch = "xtensa")]
pub struct S3RouteSnapshotKeys {
    keys: Vec<DestinationHash, PsramAlloc>,
}

#[cfg(target_arch = "xtensa")]
impl S3RouteSnapshotKeys {
    fn new() -> Self {
        Self {
            keys: Vec::with_capacity_in(EngineStorageType::TRACKED_DESTINATIONS, PsramAlloc),
        }
    }
}

#[cfg(target_arch = "xtensa")]
impl RouteSnapshotKeys for S3RouteSnapshotKeys {
    fn clear(&mut self) {
        self.keys.clear();
    }

    fn push(&mut self, destination: DestinationHash) -> Result<(), RouteSnapshotKeyError> {
        if self.keys.len() == self.keys.capacity() {
            return Err(RouteSnapshotKeyError::Capacity);
        }
        self.keys.push(destination);
        Ok(())
    }

    fn get(&self, index: usize) -> Option<DestinationHash> {
        self.keys.get(index).copied()
    }
}

#[cfg(target_arch = "xtensa")]
pub type S3Persistence = EmbeddedFlashPersistence<
    S3SharedFlash,
    S3RouteSnapshotKeys,
    fn(EmbeddedPersistenceDiagnostic),
    S3_PENDING,
>;
#[cfg(target_arch = "riscv32")]
pub type C6Persistence = EmbeddedFlashPersistence<
    EspRomFlash,
    FixedRouteSnapshotKeys<{ crate::storage::C6Storage::TRACKED_DESTINATIONS }>,
    fn(EmbeddedPersistenceDiagnostic),
    C6_PENDING,
>;

static STATE_NOT_SAVED: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "xtensa")]
pub fn s3(flash: S3SharedFlash, layout: FlashJournalLayout) -> S3Persistence {
    EmbeddedFlashPersistence::new(
        flash,
        layout,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            EngineStorageType::MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        S3RouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

#[cfg(target_arch = "riscv32")]
pub fn c6() -> C6Persistence {
    EmbeddedFlashPersistence::new(
        EspRomFlash::new(C6_FLASH_CAPACITY),
        C6_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            crate::storage::C6Storage::MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        FixedRouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

#[cfg(target_arch = "xtensa")]
pub fn state_not_saved() -> bool {
    STATE_NOT_SAVED.load(Ordering::Acquire)
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    match diagnostic {
        EmbeddedPersistenceDiagnostic::Restored(report) => {
            STATE_NOT_SAVED.store(false, Ordering::Release);
            log::info!(
                "state restored routes={} refused={} ratchets={} warning={:?}",
                report.route_seeded_count,
                report.route_refused_count,
                report.ratchet_seeded_count,
                report.warning
            );
        }
        EmbeddedPersistenceDiagnostic::BatchPersisted {
            records,
            at,
            state_not_saved,
        } => {
            STATE_NOT_SAVED.store(state_not_saved, Ordering::Release);
            log::info!("state persisted records={records} at={}", at.0);
        }
        EmbeddedPersistenceDiagnostic::CompactionStarted {
            at,
            next_allowed_at,
        } => {
            log::info!(
                "state compaction started at={} next_allowed_at={}",
                at.0,
                next_allowed_at.0
            );
        }
        EmbeddedPersistenceDiagnostic::CompactionCompleted {
            records,
            at,
            state_not_saved,
        } => {
            STATE_NOT_SAVED.store(state_not_saved, Ordering::Release);
            log::info!("state compaction completed records={records} at={}", at.0);
        }
        EmbeddedPersistenceDiagnostic::DurabilityDeferred { target, until } => {
            STATE_NOT_SAVED.store(true, Ordering::Release);
            log::warn!(
                "state durability deferred target={target:?} until={}",
                until.0
            );
        }
        EmbeddedPersistenceDiagnostic::WriteFailed { failure, retry_at } => {
            STATE_NOT_SAVED.store(true, Ordering::Release);
            log::error!(
                "state persistence failed {failure:?}; retry_at={}",
                retry_at.0
            );
        }
    }
}
