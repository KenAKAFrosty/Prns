mod display;
mod hardware;
mod identity;

use personal_hopspot_memory::{MemoryProfile, RegionRole, T114};
use personal_rns::interfaces::InterfaceId;

pub(crate) use super::button::{drive as drive_button, EVENTS as INPUT_EVENTS};
use crate::memory::NrfFirmwareMemory;
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::St7789Display as DisplayDriver;
pub(crate) use hardware::{
    T114Battery as Battery, T114Board as Board, T114DisplayBringup as Display,
    T114Hardware as Hardware, T114LoraInterface as LoraInterface,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};

pub(crate) const MEMORY_PROFILE: &MemoryProfile = &T114;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Heltec T114)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T114-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t114-usb");
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = MEMORY.two_flash_pages(RegionRole::RadioProfile);
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(MEMORY.flash_offset(RegionRole::RemoteControlIdentity));
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot T114\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T114";

pub(crate) async fn maintain() {}
