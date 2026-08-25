mod display;
mod hardware;
mod identity;
mod profile;

use personal_rns::interfaces::InterfaceId;

pub(crate) use super::button::{drive as drive_button, EVENTS as INPUT_EVENTS};
pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::St7789Display as DisplayDriver;
pub(crate) use hardware::{
    T114Battery as Battery, T114Board as Board, T114Display as Display, T114Hardware as Hardware,
    T114LoraInterface as LoraInterface,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};
pub(crate) use profile::{new as new_profile_store, Store as ProfileStore};

pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Heltec T114)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T114-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t114-usb");
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] =
    personal_hopspot_core::NRF52840_RADIO_PROFILE_PAGES;
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot T114\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T114";

pub(crate) async fn maintain() {}

const _: () = {
    const PAGE_BYTES: u32 = embassy_nrf::nvmc::PAGE_SIZE as u32;
    assert!(identity::BLE_IDENTITY_FLASH_OFFSET + PAGE_BYTES == RADIO_PROFILE_PAGES[0]);
    assert!(RADIO_PROFILE_PAGES[0] + PAGE_BYTES == RADIO_PROFILE_PAGES[1]);
    assert!(RADIO_PROFILE_PAGES[1] + PAGE_BYTES == identity::NODE_IDENTITY_FLASH_OFFSET);
};
