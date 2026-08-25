use core::sync::atomic::{AtomicU8, Ordering};

#[cfg(feature = "board-t1000e")]
use embassy_embedded_hal::adapter::BlockingAsync;
#[cfg(feature = "board-t1000e")]
use embassy_nrf::nvmc::Nvmc;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
#[cfg(not(feature = "board-t1000e"))]
use nrf_softdevice::{Flash, Softdevice};
use personal_hopspot_core::PersistenceState;
use personal_rns::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistencePolicy, FixedRouteSnapshotKeys, SharedNorFlash,
};
use static_cell::StaticCell;

use crate::boards::selected as board;
use crate::storage::Nrf52840Storage as Storage;

const FLASH_CAPACITY: usize = 1024 * 1024;
const PENDING: usize = 8;

#[cfg(not(feature = "board-t1000e"))]
type FlashDriver = Flash;
#[cfg(feature = "board-t1000e")]
type FlashDriver = BlockingAsync<Nvmc<'static>>;

pub(crate) type BoardFlash = SharedNorFlash<'static, CriticalSectionRawMutex, FlashDriver>;
pub(crate) type BoardPersistence = EmbeddedFlashPersistence<
    BoardFlash,
    FixedRouteSnapshotKeys<{ Storage::TRACKED_DESTINATIONS }>,
    fn(EmbeddedPersistenceDiagnostic),
    PENDING,
>;

const _: () = assert!(
    Storage::MAX_COMPACTED_FLASH_JOURNAL_BYTES <= board::JOURNAL_LAYOUT.arenas[0].len() as usize
);
const _: () = assert!(
    Storage::MAX_COMPACTED_FLASH_JOURNAL_BYTES <= board::JOURNAL_LAYOUT.arenas[1].len() as usize
);
const _: () = assert!(
    personal_hopspot_core::NRF52840_MIN_ARENA_BYTES
        <= board::JOURNAL_LAYOUT.arenas[0].len() as usize
);
const _: () = assert!(
    personal_hopspot_core::NRF52840_MIN_ARENA_BYTES
        <= board::JOURNAL_LAYOUT.arenas[1].len() as usize
);

static PERSISTENCE_STATE: AtomicU8 = AtomicU8::new(PersistenceState::Durable.encode());

#[cfg(not(feature = "board-t1000e"))]
pub(crate) fn take_flash(sd: &'static Softdevice) -> BoardFlash {
    static FLASH_STORAGE: StaticCell<Mutex<CriticalSectionRawMutex, FlashDriver>> =
        StaticCell::new();
    let flash = Flash::take(sd);
    SharedNorFlash::new(FLASH_STORAGE.init(Mutex::new(flash)), FLASH_CAPACITY)
}

#[cfg(feature = "board-t1000e")]
pub(crate) fn take_flash(nvmc: Nvmc<'static>) -> BoardFlash {
    static FLASH_STORAGE: StaticCell<Mutex<CriticalSectionRawMutex, FlashDriver>> =
        StaticCell::new();
    let flash = BlockingAsync::new(nvmc);
    SharedNorFlash::new(FLASH_STORAGE.init(Mutex::new(flash)), FLASH_CAPACITY)
}

pub(crate) fn new(flash: BoardFlash) -> BoardPersistence {
    EmbeddedFlashPersistence::new(
        flash,
        board::JOURNAL_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            Storage::MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        FixedRouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114"
))]
pub(crate) fn persistence_state() -> PersistenceState {
    PersistenceState::decode(PERSISTENCE_STATE.load(Ordering::Acquire))
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    if let Some(state) = PersistenceState::from_embedded_diagnostic(&diagnostic) {
        PERSISTENCE_STATE.store(state.encode(), Ordering::Release);
    }
}
