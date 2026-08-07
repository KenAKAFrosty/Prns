use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use nrf_softdevice::Flash;
use personal_hopspot_core::PersistenceState;
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

static PERSISTENCE_STATE: AtomicU8 = AtomicU8::new(PersistenceState::Durable.encode());

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
