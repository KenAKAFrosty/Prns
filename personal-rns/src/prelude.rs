pub use prns_runtime::runtime::{
    Diagnostic, Manual, Message, PreConfiguredDestination, PrnsEvent, PrnsNodeApi, PrnsNodeRecipe,
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

#[cfg(feature = "tokio-host")]
pub use prns_runtime_tokio::runtime::{
    ephemeral_ble_identity, fill_os_entropy, generate_identity_secret,
    load_or_create_identity_secret, try_generate_identity_secret, IdentitySecretFileError,
    OsEntropyError,
};

#[cfg(feature = "tokio-host")]
pub use prns_runtime_tokio::runtime::{
    AttachIntent, Attachable, AttachedInterface, AttachedSupervisor, Fleet, PrnsNode,
    PrnsNodeHandle,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_runtime_embassy::runtime::{Fleet, PrnsNode, PrnsNodeHandle};

#[cfg(all(feature = "embassy-host", feature = "tokio-host"))]
pub use prns_runtime_embassy::runtime::{
    PrnsNode as EmbassyPrnsNode, PrnsNodeHandle as EmbassyPrnsNodeHandle,
};

#[cfg(all(feature = "tcp", feature = "tokio-host"))]
pub use prns_interfaces_tokio::tcp;

#[cfg(all(feature = "tcp", feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::tcp;

#[cfg(all(feature = "wifi-auto", feature = "tokio-host"))]
pub use prns_interfaces_tokio::wifi_auto;

#[cfg(all(
    feature = "wifi-auto",
    feature = "embassy-host",
    not(feature = "tokio-host")
))]
pub use prns_interfaces_embassy::wifi_auto;

#[cfg(all(feature = "wifi-direct", feature = "tokio-host"))]
pub use prns_interfaces_tokio::wifi_direct;

#[cfg(all(feature = "wifi-aware", feature = "tokio-host"))]
pub use prns_interfaces_tokio::wifi_aware;

#[cfg(feature = "usb")]
pub mod usb_auto {
    #[cfg(feature = "embassy-host")]
    pub use prns_interfaces_embassy::usb_auto::*;
    #[cfg(feature = "tokio-host")]
    pub use prns_interfaces_tokio::usb_auto::*;
}

#[cfg(all(feature = "usb", feature = "tokio-host"))]
pub use usb_auto::AutoUsb;

#[cfg(all(
    feature = "tokio-host",
    any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto")
))]
pub use prns_interfaces_tokio::interface_menu::DefaultAutoInterfaces;

#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::interface_menu;

#[cfg(all(feature = "interface-discovery", feature = "tokio-host"))]
pub use prns_interfaces_tokio::interface_discovery::{
    DiscoveredConnectionFailure, DiscoveryIngressOutcome, RunningTokioInterfaceDiscoveryPublisher,
    TokioDiscoveryEvent, TokioDiscoveryIngress, TokioDiscoveryPublicationEvent,
    TokioDiscoveryPublicationFramingFailure, TokioDiscoveryPublicationPreparationFailure,
    TokioDiscoveryPublisherConstructionError, TokioInterfaceDiscovery,
    TokioInterfaceDiscoveryPublisher, DISCOVERY_PUBLICATION_JOB_INTERVAL,
};

#[cfg(all(feature = "config", feature = "tokio-host"))]
pub use prns_interfaces_tokio::from_plan::{
    self, attach_plan, attach_plan_with_context, config, FromPlan, PlanAttachments, PlanFailure,
    PlanOutcome, PlanRuntimeContext,
};

#[cfg(all(feature = "bluetooth-auto", feature = "tokio-host"))]
pub use prns_interfaces_tokio::bluetooth_auto;

#[cfg(all(feature = "bluetooth-auto", feature = "tokio-host"))]
pub use prns_interfaces_tokio::bluetooth_auto::{AttachedBle, AutoBle};

#[cfg(all(
    feature = "bluetooth-auto",
    feature = "embassy-host",
    not(feature = "tokio-host")
))]
pub use prns_interfaces_embassy::bluetooth_auto;

#[cfg(all(feature = "udp", feature = "tokio-host"))]
pub use prns_interfaces_tokio::udp;

#[cfg(all(feature = "serial", feature = "tokio-host"))]
pub use prns_interfaces_tokio::serial;

#[cfg(all(feature = "kiss", feature = "tokio-host"))]
pub use prns_interfaces_tokio::kiss;

#[cfg(all(feature = "ax25", feature = "tokio-host"))]
pub use prns_interfaces_tokio::ax25_kiss;

#[cfg(all(feature = "rnode", feature = "tokio-host"))]
pub use prns_interfaces_tokio::rnode;

#[cfg(all(feature = "pipe", feature = "tokio-host"))]
pub use prns_interfaces_tokio::pipe;

#[cfg(all(feature = "backbone", feature = "tokio-host"))]
pub use prns_interfaces_tokio::backbone;

#[cfg(all(feature = "websocket", feature = "tokio-host"))]
pub use prns_interfaces_tokio::websocket;

#[cfg(all(feature = "i2p", feature = "tokio-host"))]
pub use prns_interfaces_tokio::i2p;

#[cfg(all(feature = "weave", feature = "tokio-host"))]
pub use prns_interfaces_tokio::weave;

#[cfg(all(feature = "shared-instance", feature = "tokio-host"))]
pub use prns_interfaces_tokio::shared_instance;

#[cfg(feature = "shared-instance")]
pub use prns_runtime::runtime::rns_remote_management;

#[cfg(all(feature = "lora", feature = "embassy-host"))]
pub use prns_interfaces_embassy::{lora, radios};

#[cfg(all(feature = "esp-now", feature = "embassy-host"))]
pub use prns_interfaces_embassy::esp_now;
