use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

#[zbus::proxy(
    interface = "fi.w1.wpa_supplicant1",
    default_service = "fi.w1.wpa_supplicant1",
    default_path = "/fi/w1/wpa_supplicant1"
)]
pub trait Supplicant {
    fn get_interface(&self, ifname: &str) -> zbus::Result<OwnedObjectPath>;
    fn create_interface(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "fi.w1.wpa_supplicant1.Interface",
    default_service = "fi.w1.wpa_supplicant1"
)]
pub trait SupplicantInterface {
    #[zbus(property)]
    fn ifname(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "fi.w1.wpa_supplicant1.Peer",
    default_service = "fi.w1.wpa_supplicant1"
)]
pub trait Peer {
    #[zbus(property)]
    fn device_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn device_address(&self) -> zbus::Result<Vec<u8>>;
}

#[zbus::proxy(
    interface = "fi.w1.wpa_supplicant1.Interface.P2PDevice",
    default_service = "fi.w1.wpa_supplicant1"
)]
pub trait P2PDevice {
    fn find(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn stop_find(&self) -> zbus::Result<()>;
    fn listen(&self, timeout: i32) -> zbus::Result<()>;
    fn extended_listen(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn connect(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<String>;
    fn cancel(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
    fn add_service(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn flush_service(&self) -> zbus::Result<()>;
    fn service_discovery_request(&self, args: HashMap<&str, Value<'_>>) -> zbus::Result<u64>;
    fn service_discovery_cancel_request(&self, args: u64) -> zbus::Result<()>;
    fn service_discovery_external(&self, arg: i32) -> zbus::Result<()>;

    #[zbus(property, name = "P2PDeviceConfig")]
    fn p2p_device_config(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    #[zbus(property, name = "P2PDeviceConfig")]
    fn set_p2p_device_config(&self, config: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
}

pub const P2P_DEVICE_INTERFACE: &str = "fi.w1.wpa_supplicant1.Interface.P2PDevice";
pub const SUPPLICANT_SERVICE: &str = "fi.w1.wpa_supplicant1";

pub type GroupProperties = HashMap<String, OwnedValue>;
