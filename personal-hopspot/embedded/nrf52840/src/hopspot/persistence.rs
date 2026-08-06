use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use nrf_softdevice::Flash;
use personal_rns::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistencePolicy, FixedRouteSnapshotKeys, SharedNorFlash,
};

pub const ARENA_BYTES: usize = personal_hopspot_core::T_ECHO_MIN_ARENA_BYTES;

const PENDING: usize = 8;

pub type TechoSharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, Flash>;
pub type TechoPersistence = EmbeddedFlashPersistence<
    TechoSharedFlash,
    FixedRouteSnapshotKeys<{ crate::storage::TechoStorage::TRACKED_DESTINATIONS }>,
    fn(EmbeddedPersistenceDiagnostic),
    PENDING,
>;

static STATE_NOT_SAVED: AtomicBool = AtomicBool::new(false);

pub fn new(flash: TechoSharedFlash) -> TechoPersistence {
    EmbeddedFlashPersistence::new(
        flash,
        personal_hopspot_core::T_ECHO_JOURNAL_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            crate::storage::TechoStorage::MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        FixedRouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

pub fn state_not_saved() -> bool {
    STATE_NOT_SAVED.load(Ordering::Acquire)
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    match diagnostic {
        EmbeddedPersistenceDiagnostic::Restored(_) => {
            STATE_NOT_SAVED.store(false, Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::BatchPersisted {
            state_not_saved, ..
        }
        | EmbeddedPersistenceDiagnostic::CompactionCompleted {
            state_not_saved, ..
        } => {
            STATE_NOT_SAVED.store(state_not_saved, Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::CompactionStarted { .. } => {}
        EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. }
        | EmbeddedPersistenceDiagnostic::WriteFailed { .. } => {
            STATE_NOT_SAVED.store(true, Ordering::Release);
        }
    }
}
