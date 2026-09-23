mod hardware;
mod identity;

use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use personal_hopspot_memory::{MemoryProfile, RegionRole, RAK4631};
use personal_rns::interfaces::InterfaceId;

use crate::memory::NrfFirmwareMemory;
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use hardware::{
    Rak4631Board as Board, Rak4631Hardware as Hardware, Rak4631LoraInterface as LoraInterface,
};
pub(crate) use identity::{bootstrap_ble_identity, bootstrap_node_identity};

pub(crate) const MEMORY_PROFILE: &MemoryProfile = &RAK4631;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(MEMORY.flash_offset(RegionRole::RemoteControlIdentity));
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (RAK WisBlock 4631)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-RAK4631-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"rak-4631");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x18Personal Hopspot RAK4631\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot RAK4631";

const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

pub(crate) static BUTTON_PRESSES: Channel<CriticalSectionRawMutex, (), 4> = Channel::new();

pub(crate) async fn maintain() {}

/// Optional WisBlock IO5 / Meshtastic PIN_BUTTON1: P0.09, active-low with pull-up.
pub(crate) async fn drive_button(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(BUTTON_DEBOUNCE).await;
        if !button.is_low() {
            continue;
        }
        BUTTON_PRESSES.send(()).await;
        button.wait_for_rising_edge().await;
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}
