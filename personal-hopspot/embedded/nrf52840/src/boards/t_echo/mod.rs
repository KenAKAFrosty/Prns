mod display;
mod hardware;
mod identity;
mod input;
mod persistence;
mod ssd1681;

use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::{frame_hash, EinkScreen};
pub(crate) use hardware::{
    TechoBoard as Board, TechoControls as Controls, TechoDisplayHardware as DisplayHardware,
    TechoEarlyHardware as EarlyHardware, TechoFaceHardware as FaceHardware, TechoRadio as Radio,
    TechoRuntimeHardware as RuntimeHardware, TechoUsbHardware as UsbHardware,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};
pub(crate) use input::{drive_button, drive_frontlight, EVENTS as INPUT_EVENTS};
pub(crate) use persistence::{
    new as new_persistence, persistence_state, TechoPersistence as Persistence,
};

pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] =
    personal_hopspot_core::NRF52840_RADIO_PROFILE_PAGES;
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (T-Echo)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-TECHO-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"techousb");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x17Personal Hopspot T-Echo\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T-Echo";
