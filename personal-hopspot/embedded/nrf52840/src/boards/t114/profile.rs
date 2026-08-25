use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use nrf_softdevice::{Flash, Softdevice};
use personal_hopspot_core::RadioProfileStore;
use personal_rns::runtime::SharedNorFlash;
use static_cell::StaticCell;

type SharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, Flash>;

pub(crate) type Store = RadioProfileStore<SharedFlash>;

pub(crate) fn new(sd: &'static Softdevice) -> Store {
    static FLASH_STORAGE: StaticCell<Mutex<CriticalSectionRawMutex, Flash>> = StaticCell::new();
    let flash = Flash::take(sd);
    let shared = SharedNorFlash::new(FLASH_STORAGE.init(Mutex::new(flash)), 1024 * 1024);
    RadioProfileStore::new(shared, super::RADIO_PROFILE_PAGES)
}
