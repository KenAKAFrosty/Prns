use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_nrf::nvmc::Nvmc;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use personal_hopspot_core::RadioProfileStore;
use personal_rns::runtime::SharedNorFlash;
use static_cell::StaticCell;

type AsyncFlash = BlockingAsync<Nvmc<'static>>;
type SharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, AsyncFlash>;

pub(crate) type Store = RadioProfileStore<SharedFlash>;

pub(crate) fn new(flash: Nvmc<'static>) -> Store {
    static FLASH_STORAGE: StaticCell<Mutex<CriticalSectionRawMutex, AsyncFlash>> =
        StaticCell::new();
    let flash = BlockingAsync::new(flash);
    let shared = SharedNorFlash::new(FLASH_STORAGE.init(Mutex::new(flash)), 1024 * 1024);
    RadioProfileStore::new(shared, super::RADIO_PROFILE_PAGES)
}
