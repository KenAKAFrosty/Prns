use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use nrf_softdevice::Flash;
use personal_hopspot_core::PersistenceState;
use personal_rns::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistencePolicy, FixedRouteSnapshotKeys, SharedNorFlash,
};

// The T1000-E shares the T-Echo's on-chip flash layout (byte-identical nRF52840 +
// S140 + memory.x), so the durable arena/journal sizing aliases the T-Echo
// constants. `personal_hopspot_core::flash_layout` defines `T1000E_*` aliases of
// these same values; once they are re-exported from the core crate root the board
// should reference those instead, so the intent ("T1000-E layout") reads at the
// call site.
pub const ARENA_BYTES: usize = personal_hopspot_core::T_ECHO_MIN_ARENA_BYTES;

const PENDING: usize = 8;

pub type T1000eSharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, Flash>;
pub type T1000ePersistence = EmbeddedFlashPersistence<
    T1000eSharedFlash,
    FixedRouteSnapshotKeys<{ super::storage::T1000eStorage::TRACKED_DESTINATIONS }>,
    fn(EmbeddedPersistenceDiagnostic),
    PENDING,
>;

static PERSISTENCE_STATE: AtomicU8 = AtomicU8::new(PersistenceState::Durable.encode());

pub fn new(flash: T1000eSharedFlash) -> T1000ePersistence {
    EmbeddedFlashPersistence::new(
        flash,
        personal_hopspot_core::T_ECHO_JOURNAL_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            super::storage::T1000eStorage::MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        FixedRouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

pub fn persistence_state() -> PersistenceState {
    PersistenceState::decode(PERSISTENCE_STATE.load(Ordering::Acquire))
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    match diagnostic {
        EmbeddedPersistenceDiagnostic::Restored(_) => {
            PERSISTENCE_STATE.store(PersistenceState::Durable.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::BatchPersisted {
            state_not_saved, ..
        }
        | EmbeddedPersistenceDiagnostic::CompactionCompleted {
            state_not_saved, ..
        } => {
            let state = if state_not_saved {
                PersistenceState::Deferred
            } else {
                PersistenceState::Durable
            };
            PERSISTENCE_STATE.store(state.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::CompactionStarted { .. } => {}
        EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. } => {
            PERSISTENCE_STATE.store(PersistenceState::Deferred.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::WriteFailed { .. } => {
            PERSISTENCE_STATE.store(PersistenceState::Failed.encode(), Ordering::Release);
        }
    }
}