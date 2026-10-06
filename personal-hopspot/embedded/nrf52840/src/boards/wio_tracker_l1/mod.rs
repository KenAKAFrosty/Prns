mod display;
mod gnss;
mod hardware;
mod identity;
mod input;

use personal_hopspot_memory::{MemoryProfile, RegionRole, WIO_TRACKER_L1};
use personal_rns::interfaces::InterfaceId;

use crate::memory::NrfFirmwareMemory;
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::OledDisplay as DisplayDriver;
pub(crate) use gnss::{
    control as control_gnss, drive as drive_gnss, snapshot as gnss_snapshot, WioGnss as Gnss,
};
pub(crate) use hardware::{
    WioBattery as Battery, WioBoard as Board, WioDisplayBringup as Display,
    WioHardware as Hardware, WioLoraInterface as LoraInterface,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};
pub(crate) use input::{drive as drive_button, WioInputs as ButtonInput, EVENTS as INPUT_EVENTS};

pub(crate) const MEMORY_PROFILE: &MemoryProfile = &WIO_TRACKER_L1;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
#[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Wio Tracker L1)";
#[cfg(feature = "wio-tracker-l1-pro-1w")]
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Wio Tracker L1 Pro 1W)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-WIO-L1-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"wiol1usb");
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = MEMORY.two_flash_pages(RegionRole::RadioProfile);
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(MEMORY.flash_offset(RegionRole::RemoteControlIdentity));
#[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1fPersonal Hopspot Wio Tracker L1\xc0";
#[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot Wio Tracker L1";
#[cfg(feature = "wio-tracker-l1-pro-1w")]
pub(crate) const ANNOUNCE_APP_DATA: &[u8] =
    b"\x92\xc4\x26Personal Hopspot Wio Tracker L1 Pro 1W\xc0";
#[cfg(feature = "wio-tracker-l1-pro-1w")]
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot Wio Tracker L1 Pro 1W";
