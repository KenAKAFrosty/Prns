use portable_atomic::{AtomicBool, Ordering};

use personal_rns::persistence::{FlashArenaRange, FlashJournalLayout};
use personal_rns::runtime::{
    EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic, EmbeddedPersistenceFailure,
    EmbeddedPersistencePolicy,
};

use crate::flash::EspRomFlash;

#[cfg(target_arch = "xtensa")]
pub const S3_FLASH_CAPACITY: usize = 8 * 1024 * 1024;
#[cfg(target_arch = "riscv32")]
pub const C6_FLASH_CAPACITY: usize = 4 * 1024 * 1024;
#[cfg(target_arch = "xtensa")]
pub const S3_ARENA_BYTES: usize = 191 * 4096;
#[cfg(target_arch = "riscv32")]
pub const C6_ARENA_BYTES: usize = 15 * 4096;
#[cfg(target_arch = "xtensa")]
pub const S3_LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0x680000, 0x681000],
    [
        FlashArenaRange::new(0x682000, 0x741000),
        FlashArenaRange::new(0x741000, 0x800000),
    ],
);
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
pub type S3Persistence = EmbeddedFlashPersistence<
    EspRomFlash<S3_FLASH_CAPACITY>,
    fn(EmbeddedPersistenceDiagnostic),
    S3_PENDING,
>;
#[cfg(target_arch = "riscv32")]
pub type C6Persistence = EmbeddedFlashPersistence<
    EspRomFlash<C6_FLASH_CAPACITY>,
    fn(EmbeddedPersistenceDiagnostic),
    C6_PENDING,
>;

static STATE_NOT_SAVED: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "xtensa")]
pub fn s3() -> S3Persistence {
    EmbeddedFlashPersistence::new(
        EspRomFlash::new(),
        S3_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

#[cfg(target_arch = "riscv32")]
pub fn c6() -> C6Persistence {
    EmbeddedFlashPersistence::new(
        EspRomFlash::new(),
        C6_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(),
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
            log::info!(
                "state restored routes={} refused={} ratchets={} warning={:?}",
                report.route_seeded_count,
                report.route_refused_count,
                report.ratchet_seeded_count,
                report.warning
            );
        }
        EmbeddedPersistenceDiagnostic::BatchPersisted { records, at } => {
            STATE_NOT_SAVED.store(false, Ordering::Release);
            log::info!("state persisted records={records} at={}", at.0);
        }
        EmbeddedPersistenceDiagnostic::WriteFailed { failure, retry_at } => {
            if failure == EmbeddedPersistenceFailure::Flash {
                STATE_NOT_SAVED.store(true, Ordering::Release);
            }
            log::error!(
                "state persistence failed {failure:?}; retry_at={}",
                retry_at.0
            );
        }
    }
}
