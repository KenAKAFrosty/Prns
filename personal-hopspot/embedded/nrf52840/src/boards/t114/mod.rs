mod hardware;
mod identity;

use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use hardware::{
    T114Board as Board, T114Hardware as Hardware, T114LoraInterface as LoraInterface,
};
pub(crate) use identity::bootstrap_node_identity;

pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Heltec T114)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-T114-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"t114-usb");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot T114\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T114";
