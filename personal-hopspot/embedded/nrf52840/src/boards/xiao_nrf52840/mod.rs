mod hardware;
mod identity;

use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use personal_hopspot_memory::{MemoryProfile, RegionRole, XIAO_NRF52840};
use personal_rns::interfaces::InterfaceId;

use crate::memory::NrfFirmwareMemory;
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use hardware::{
    XiaoBoard as Board, XiaoHardware as Hardware, XiaoLoraInterface as LoraInterface,
};
pub(crate) use identity::{bootstrap_ble_identity, bootstrap_node_identity};

pub(crate) const MEMORY_PROFILE: &MemoryProfile = &XIAO_NRF52840;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = MEMORY.two_flash_pages(RegionRole::RadioProfile);
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at_with_stale_application_page_recovery(
        MEMORY.flash_offset(RegionRole::RemoteControlIdentity),
    );
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Seeed XIAO nRF52840)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-XIAO-NRF52840-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"xiao-nrf");
// msgpack: fixarray(2), str8 of NODE_ANNOUNCE_APP_DATA.len(), then nil.
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot XIAO\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot XIAO";

const _: () = {
    // The str8 length byte must agree with the name, or peers decode a truncated announce.
    assert!(NODE_ANNOUNCE_APP_DATA.len() == 0x15);
    assert!(ANNOUNCE_APP_DATA.len() == NODE_ANNOUNCE_APP_DATA.len() + 4);
};

const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

pub(crate) static BUTTON_PRESSES: Channel<CriticalSectionRawMutex, (), 4> = Channel::new();

pub(crate) async fn maintain() {}

/// Optional user button on D0 (P0.02), active-low with pull-up; idle when nothing is fitted.
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
