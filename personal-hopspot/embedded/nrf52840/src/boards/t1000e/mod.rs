mod gnss;
mod hardware;
mod identity;
mod radio;

use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use gnss::{
    control as control_gnss, drive as drive_gnss, snapshot as gnss_snapshot, T1000eGnss as Gnss,
};
pub(crate) use hardware::{
    T1000eBoard as Board, T1000eHardware as Hardware, T1000eLoraInterface as LoraInterface,
};
pub(crate) use identity::bootstrap_node_identity;

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    personal_hopspot_core::T1000E_JOURNAL_LAYOUT;
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(
        personal_hopspot_core::T1000E_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    );
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (T1000-E)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T1000E-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t1ke-usb");
/// Antenna-referred TX ceiling (LR1110 / board PA path).
pub(crate) const MAX_TX_POWER_DBM: i8 = 22;
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x18Personal Hopspot T1000-E\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T1000-E";
