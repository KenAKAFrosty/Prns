mod display;
mod hardware;
mod identity;
mod input;
mod persistence;
mod storage;

use personal_rns::interfaces::InterfaceId;

pub(crate) use display::{frame_hash, EinkScreen};
pub(crate) use hardware::{
    T1000eBoard as Board, T1000eControls as Controls, T1000eDisplayHardware as DisplayHardware,
    T1000eEarlyHardware as EarlyHardware, T1000eFaceHardware as FaceHardware, T1000eRadio as LoraRadio,
    T1000eRuntimeHardware as RuntimeHardware, T1000eUsbHardware as UsbHardware,
};
pub(crate) use identity::{
    bootstrap_ble_identity, bootstrap_node_identity, startup_notice as identity_startup_notice,
};
pub(crate) use input::{drive_button, drive_frontlight, EVENTS as INPUT_EVENTS};
pub(crate) use persistence::{
    new as new_persistence, persistence_state, T1000ePersistence as Persistence,
};
pub(crate) use storage::T1000eStorage as Storage;

// The T1000-E shares the T-Echo's on-chip flash layout (byte-identical nRF52840 +
// S140 + memory.x), so the radio-profile pages alias the T-Echo's. Once
// `personal_hopspot_core` re-exports the `T1000E_*` constants from its crate root,
// swap this to `personal_hopspot_core::T1000E_RADIO_PROFILE_PAGES`.
pub(crate) const RADIO_PROFILE_PAGES: [u32; 2] = personal_hopspot_core::T_ECHO_RADIO_PROFILE_PAGES;
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (T1000-E)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T1000E-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t1000eub");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x17Personal Hopspot T1000-E\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T1000-E";