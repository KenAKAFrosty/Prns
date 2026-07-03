//! The one `use` for a Personal Reticulum app: the runtime vocabulary plus every enabled
//! interface family under its lane-neutral name — the declared runtime lane picks the
//! implementation, and tokio takes the bare name when a build enables both lanes.

pub use prns_runtime::runtime::{
    Diagnostic, Manual, Message, PreConfiguredDestination, PrnsApi, PrnsEvent, PrnsRecipe,
    RuntimeHealth, SendError,
};

pub use prns_runtime::engine::{CommandId, EngineCommand, PacketReceiptDelivered, RatchetPolicy};
pub use prns_runtime::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
pub use prns_runtime::interfaces::InterfaceStatus;
pub use prns_runtime::routes;
pub use prns_runtime::routing::links::resources::ResourceStrategy;
pub use prns_runtime::routing::ProofStrategy;
pub use prns_runtime::storage::Nrf52840;
pub use prns_runtime::wire::{DestinationHash, TransportId};

#[cfg(feature = "alloc")]
pub use prns_runtime::storage::GrowableHeap;

#[cfg(feature = "external-alloc")]
pub use prns_runtime::storage::{Esp32C6, Esp32S3};

#[cfg(feature = "std")]
pub use prns_runtime::runtime::{
    ephemeral_ble_identity, generate_identity_secret, load_or_create_identity_secret,
    IdentitySecretFileError,
};

#[cfg(feature = "tokio-host")]
pub use prns_runtime::runtime::{
    AttachIntent, Attachable, AttachedInterface, AttachedSupervisor, Fleet, Prns, TokioPrnsHandle,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_runtime::runtime::{Fleet, Prns};

#[cfg(feature = "embassy-host")]
pub use prns_runtime::runtime::EmbassyPrnsHandle;

#[cfg(all(feature = "tcp", feature = "tokio-host"))]
pub use prns_interfaces_tokio::tcp;

#[cfg(all(feature = "tcp", feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::tcp;

#[cfg(all(feature = "wifi", feature = "tokio-host"))]
pub use prns_interfaces_tokio::wifi;

#[cfg(all(
    feature = "wifi",
    feature = "embassy-host",
    not(feature = "tokio-host")
))]
pub use prns_interfaces_embassy::wifi;

#[cfg(all(feature = "wifi-direct", feature = "tokio-host"))]
pub use prns_interfaces_tokio::wifi_direct;

#[cfg(all(feature = "usb", feature = "tokio-host"))]
pub use prns_interfaces_tokio::usb;

#[cfg(all(feature = "usb", feature = "tokio-host"))]
pub use prns_interfaces_tokio::usb_host::{self, AutoUsb};

#[cfg(all(
    feature = "tokio-host",
    any(feature = "wifi", feature = "usb", feature = "ble")
))]
pub use prns_interfaces_tokio::auto::Auto;

#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::interface_menu;

#[cfg(all(feature = "config", feature = "tokio-host"))]
pub use prns_interfaces_tokio::from_plan::{self, attach_plan, config, FromPlan, PlanOutcome};

#[cfg(all(feature = "pipe", feature = "tokio-host"))]
pub use prns_interfaces_tokio::pipe_host;

#[cfg(all(feature = "usb", feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::usb;

#[cfg(all(feature = "ble", feature = "tokio-host"))]
pub use prns_interfaces_tokio::ble;

#[cfg(all(feature = "ble", feature = "tokio-host"))]
pub use prns_interfaces_tokio::ble_host;

#[cfg(all(feature = "ble", feature = "tokio-host"))]
pub use prns_interfaces_tokio::ble_host::{AttachedBle, AutoBle};

#[cfg(all(feature = "ble", feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::ble;

#[cfg(all(feature = "udp", feature = "tokio-host"))]
pub use prns_interfaces_tokio::udp;

#[cfg(all(feature = "serial", feature = "tokio-host"))]
pub use prns_interfaces_tokio::serial;

#[cfg(all(feature = "serial", feature = "tokio-host"))]
pub use prns_interfaces_tokio::serial_host;

#[cfg(all(feature = "kiss", feature = "tokio-host"))]
pub use prns_interfaces_tokio::kiss;

#[cfg(all(feature = "ax25", feature = "tokio-host"))]
pub use prns_interfaces_tokio::ax25;

#[cfg(all(feature = "rnode", feature = "tokio-host"))]
pub use prns_interfaces_tokio::rnode;

#[cfg(all(feature = "pipe", feature = "tokio-host"))]
pub use prns_interfaces_tokio::pipe;

#[cfg(all(feature = "backbone", feature = "tokio-host"))]
pub use prns_interfaces_tokio::backbone;

#[cfg(all(feature = "websocket", feature = "tokio-host"))]
pub use prns_interfaces_tokio::websocket;

#[cfg(all(feature = "shared-instance", feature = "tokio-host"))]
pub use prns_interfaces_tokio::shared_instance;

#[cfg(all(feature = "lora", feature = "embassy-host"))]
pub use prns_interfaces_embassy::lora;

#[cfg(all(feature = "ble-trouble", feature = "embassy-host"))]
pub use prns_interfaces_embassy::ble_trouble;

#[cfg(all(feature = "esp-now", feature = "embassy-host"))]
pub use prns_interfaces_embassy::esp_now;

#[cfg(all(feature = "usb-device", feature = "embassy-host"))]
pub use prns_interfaces_embassy::usb_device;
