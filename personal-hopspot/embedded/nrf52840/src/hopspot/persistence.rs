use core::sync::atomic::{AtomicBool, Ordering};

use nrf_softdevice::Flash;
use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};
use personal_rns::runtime::{
    EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic, EmbeddedPersistenceFailure,
    EmbeddedPersistencePolicy,
};

pub const ARENA_BYTES: usize = 20 * 4096;
pub const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0xC0000, 0xC1000],
    [
        FlashArenaRange::new(0xC2000, 0xD6000),
        FlashArenaRange::new(0xD6000, 0xEA000),
    ],
);

const PENDING: usize = 8;

pub type TechoPersistence =
    EmbeddedFlashPersistence<Flash, fn(EmbeddedPersistenceDiagnostic), PENDING>;

static STATE_NOT_SAVED: AtomicBool = AtomicBool::new(false);

pub fn new(flash: Flash) -> TechoPersistence {
    EmbeddedFlashPersistence::new(
        flash,
        LAYOUT,
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
