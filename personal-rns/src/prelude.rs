//! The one `use` for a Personal Reticulum app: the runtime vocabulary plus every enabled
//! interface family — tokio families under their bare names, embassy families under
//! `embassy_*`.

pub use prns_runtime::runtime::{
    Diagnostic, Message, PreConfiguredDestination, PrnsApi, PrnsEvent, PrnsRecipe, RuntimeHealth,
    SendError,
};

#[cfg(feature = "tokio-host")]
pub use prns_runtime::runtime::{Fleet, Prns, TokioPrnsHandle};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_runtime::runtime::{Fleet, Prns};

#[cfg(feature = "embassy-host")]
pub use prns_runtime::runtime::EmbassyPrnsHandle;

#[cfg(feature = "tcp")]
pub use prns_interfaces_tokio::tcp;

#[cfg(feature = "udp")]
pub use prns_interfaces_tokio::udp;

#[cfg(feature = "serial")]
pub use prns_interfaces_tokio::serial;

#[cfg(feature = "kiss")]
pub use prns_interfaces_tokio::kiss;

#[cfg(feature = "ax25")]
pub use prns_interfaces_tokio::ax25;

#[cfg(feature = "rnode")]
pub use prns_interfaces_tokio::rnode;

#[cfg(feature = "pipe")]
pub use prns_interfaces_tokio::pipe;

#[cfg(feature = "websocket")]
pub use prns_interfaces_tokio::websocket;

#[cfg(feature = "backbone")]
pub use prns_interfaces_tokio::backbone;

#[cfg(feature = "wifi")]
pub use prns_interfaces_tokio::wifi;

#[cfg(feature = "usb")]
pub use prns_interfaces_tokio::usb;

#[cfg(feature = "shared-instance")]
pub use prns_interfaces_tokio::shared_instance;

#[cfg(feature = "ble")]
pub use prns_interfaces_tokio::ble;

#[cfg(feature = "embassy-tcp")]
pub use prns_interfaces_embassy::tcp as embassy_tcp;

#[cfg(feature = "embassy-wifi")]
pub use prns_interfaces_embassy::wifi as embassy_wifi;

#[cfg(feature = "embassy-lora")]
pub use prns_interfaces_embassy::lora as embassy_lora;

#[cfg(feature = "embassy-esp-now")]
pub use prns_interfaces_embassy::esp_now as embassy_esp_now;

#[cfg(feature = "embassy-ble")]
pub use prns_interfaces_embassy::ble as embassy_ble;

#[cfg(feature = "embassy-usb")]
pub use prns_interfaces_embassy::usb as embassy_usb;

#[cfg(feature = "embassy-usb-device")]
pub use prns_interfaces_embassy::usb_device as embassy_usb_device;
