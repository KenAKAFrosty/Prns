mod display;
mod hardware;
mod identity;
mod input;
mod raster;
mod ssd1681;

use embassy_futures::join::join;
#[cfg(feature = "softdevice-s140-v6")]
use personal_hopspot_memory::T_ECHO_S140_V6;
#[cfg(not(feature = "softdevice-s140-v6"))]
use personal_hopspot_memory::T_ECHO_S140_V7;
use personal_hopspot_memory::{MemoryProfile, RegionRole};
use personal_rns::interfaces::InterfaceId;

use crate::memory::NrfFirmwareMemory;
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::retained_policy;
pub(crate) use hardware::{
    TechoBoard as Board, TechoControls as Controls, TechoDisplayHardware as DisplayHardware,
    TechoEarlyHardware as EarlyHardware, TechoFaceHardware as FaceHardware, TechoRadio as Radio,
    TechoRuntimeHardware as RuntimeHardware, TechoUsbHardware as UsbHardware,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};
pub(crate) use input::{EVENTS as INPUT_EVENTS, EVENT_CAPACITY as INPUT_EVENT_CAPACITY};

pub(crate) type BatteryGauge = personal_hopspot_core::BatteryGauge;

pub(crate) const fn battery_gauge() -> BatteryGauge {
    BatteryGauge::lipo()
}

pub(crate) async fn drive_controls(controls: Controls) -> ! {
    let Controls { button, frontlight } = controls;
    let _ = join(
        input::drive_button(button),
        input::drive_frontlight(frontlight),
    )
    .await;
    core::future::pending().await
}

#[cfg(feature = "softdevice-s140-v6")]
pub(crate) const MEMORY_PROFILE: &MemoryProfile = &T_ECHO_S140_V6;
#[cfg(not(feature = "softdevice-s140-v6"))]
pub(crate) const MEMORY_PROFILE: &MemoryProfile = &T_ECHO_S140_V7;

const MEMORY: NrfFirmwareMemory = NrfFirmwareMemory::new(MEMORY_PROFILE);

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    MEMORY.journal_layout();
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = MEMORY.two_flash_pages(RegionRole::RadioProfile);
pub(crate) const NODE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::NodeIdentity);
pub(crate) const BLE_IDENTITY_FLASH_OFFSET: u32 = MEMORY.flash_offset(RegionRole::BleIdentity);
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(MEMORY.flash_offset(RegionRole::RemoteControlIdentity));
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (T-Echo)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-TECHO-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"techousb");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x17Personal Hopspot T-Echo\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T-Echo";
