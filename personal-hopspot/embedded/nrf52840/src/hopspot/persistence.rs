use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use nrf_softdevice::Flash;
use personal_rns::runtime::{
    EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic, EmbeddedPersistenceFailure,
    EmbeddedPersistencePolicy, SharedNorFlash,
};

pub const ARENA_BYTES: usize = personal_hopspot_core::T_ECHO_MIN_ARENA_BYTES;

const PENDING: usize = 8;

pub type TechoSharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, Flash>;
pub type TechoPersistence =
    EmbeddedFlashPersistence<TechoSharedFlash, fn(EmbeddedPersistenceDiagnostic), PENDING>;

static STATE_NOT_SAVED: AtomicBool = AtomicBool::new(false);

pub fn new(flash: TechoSharedFlash) -> TechoPersistence {
    EmbeddedFlashPersistence::new(
        flash,
        personal_hopspot_core::T_ECHO_JOURNAL_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

pub fn state_not_saved() -> bool {
    STATE_NOT_SAVED.load(Ordering::Acquire)
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    match diagnostic {
        EmbeddedPersistenceDiagnostic::Restored(_) => {}
        EmbeddedPersistenceDiagnostic::BatchPersisted { .. } => {
            STATE_NOT_SAVED.store(false, Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::WriteFailed { failure, .. } => {
            if failure == EmbeddedPersistenceFailure::Flash {
                STATE_NOT_SAVED.store(true, Ordering::Release);
            }
        }
    }
}
