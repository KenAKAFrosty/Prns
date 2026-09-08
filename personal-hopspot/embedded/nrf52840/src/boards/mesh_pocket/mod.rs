mod battery;
mod display;
mod hardware;
mod identity;
mod raster;
mod ssd1680;

#[cfg(feature = "mesh-pocket-battery-10000")]
use personal_hopspot_memory::MESH_POCKET_10000;
#[cfg(feature = "mesh-pocket-battery-5000")]
use personal_hopspot_memory::MESH_POCKET_5000;
use personal_hopspot_memory::{MemoryProfile, RegionRole};
use personal_rns::interfaces::InterfaceId;

use crate::memory::NrfFirmwareMemory;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use battery::gauge as battery_gauge;
pub(crate) use display::retained_policy;
pub(crate) use hardware::{
    MeshPocketBoard as Board, MeshPocketControls as Controls,
    MeshPocketDisplayHardware as DisplayHardware, MeshPocketEarlyHardware as EarlyHardware,
    MeshPocketFaceHardware as FaceHardware, MeshPocketRadio as Radio,
    MeshPocketRuntimeHardware as RuntimeHardware, MeshPocketUsbHardware as UsbHardware,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};

pub(crate) use super::button::{EVENTS as INPUT_EVENTS, EVENT_CAPACITY as INPUT_EVENT_CAPACITY};

#[cfg(feature = "mesh-pocket-battery-5000")]
pub(crate) const MEMORY_PROFILE: &MemoryProfile = &MESH_POCKET_5000;
#[cfg(feature = "mesh-pocket-battery-10000")]
pub(crate) const MEMORY_PROFILE: &MemoryProfile = &MESH_POCKET_10000;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = MEMORY.two_flash_pages(RegionRole::RadioProfile);
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(MEMORY.flash_offset(RegionRole::RemoteControlIdentity));
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
#[cfg(feature = "mesh-pocket-battery-5000")]
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (MeshPocket 5000)";
#[cfg(feature = "mesh-pocket-battery-10000")]
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (MeshPocket 10000)";
#[cfg(feature = "mesh-pocket-battery-5000")]
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-MSPK5-HOP";
#[cfg(feature = "mesh-pocket-battery-10000")]
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-MSPK10-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"mspk-usb");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1bPersonal Hopspot MeshPocket\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot MeshPocket";

pub(crate) async fn drive_controls(controls: Controls) -> ! {
    super::button::drive(controls.button).await
}
