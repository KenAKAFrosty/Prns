mod display;
mod gnss;
mod hardware;
mod identity;
mod input;

use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use display::St7735Display as Display;
pub(crate) use gnss::{
    control as control_gnss, drive as drive_gnss, snapshot as gnss_snapshot, T096Gnss as Gnss,
};
pub(crate) use hardware::{
    external_power_state, T096Board as Board, T096Hardware as Hardware,
    T096LoraInterface as LoraInterface,
};
pub(crate) use identity::bootstrap_node_identity;
pub(crate) use input::{drive_button, EVENTS as INPUT_EVENTS};

pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Heltec T096)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T096-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t096-usb");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot T096\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T096";
