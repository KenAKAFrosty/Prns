mod gnss;
mod hardware;
mod identity;

use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use gnss::{
    control as control_gnss, drive as drive_gnss, snapshot as gnss_snapshot, SolarNodeGnss as Gnss,
};
pub(crate) use hardware::{
    SolarNodeBoard as Board, SolarNodeHardware as Hardware,
    SolarNodeLoraInterface as LoraInterface,
};
pub(crate) use identity::bootstrap_node_identity;

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    personal_hopspot_core::SOLAR_NODE_JOURNAL_LAYOUT;
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(
        personal_hopspot_core::SOLAR_NODE_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    );
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (SenseCAP Solar Node)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-SOLARNODE-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"scsn-usb");
// msgpack: fixarray(2), str8 of NODE_ANNOUNCE_APP_DATA.len(), then nil.
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1bPersonal Hopspot Solar Node\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot Solar Node";

const _: () = {
    // The str8 length byte must agree with the name, or peers decode a truncated announce.
    assert!(NODE_ANNOUNCE_APP_DATA.len() == 0x1b);
    assert!(ANNOUNCE_APP_DATA.len() == NODE_ANNOUNCE_APP_DATA.len() + 4);
};
