#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]
#![doc = "Reticulum"]
#![deny(rustdoc::broken_intra_doc_links)]

pub use prns_runtime::{
    crypto, engine, identity, interfaces, persistence, request_endpoints, rncp, routing, storage,
    units, wire,
};

#[cfg(feature = "rnx")]
pub use prns_runtime::rnx;

#[cfg(feature = "signed-artifact")]
pub use prns_core::message_pack;

#[cfg(feature = "interface-discovery")]
pub use prns_runtime::interface_discovery;
#[cfg(feature = "tokio-host")]
pub use prns_runtime_tokio::node_introspection;

#[cfg(all(feature = "config", feature = "tokio-host"))]
pub mod from_plan;
mod interface_families;
mod lane_guards;
pub mod manifold;
pub mod prelude;
pub mod runtime;

#[cfg(all(feature = "ax25", feature = "tokio-host"))]
pub use interface_families::ax25_kiss;
#[cfg(all(feature = "backbone", feature = "tokio-host"))]
pub use interface_families::backbone;
#[cfg(all(
    feature = "bluetooth-auto",
    any(feature = "tokio-host", feature = "embassy-host")
))]
pub use interface_families::bluetooth_auto;
#[cfg(all(feature = "browser-rendezvous", feature = "tokio-host"))]
pub use interface_families::browser_rendezvous;
#[cfg(all(feature = "esp-now", feature = "embassy-host"))]
pub use interface_families::esp_now;
#[cfg(all(feature = "i2p", feature = "tokio-host"))]
pub use interface_families::i2p;
#[cfg(all(feature = "kiss", feature = "tokio-host"))]
pub use interface_families::kiss;
#[cfg(all(feature = "pipe", feature = "tokio-host"))]
pub use interface_families::pipe;
#[cfg(all(feature = "rnode", feature = "tokio-host"))]
pub use interface_families::rnode;
#[cfg(all(feature = "serial", feature = "tokio-host"))]
pub use interface_families::serial;
#[cfg(all(feature = "shared-instance", feature = "tokio-host"))]
pub use interface_families::shared_instance;
#[cfg(all(feature = "tcp", any(feature = "tokio-host", feature = "embassy-host")))]
pub use interface_families::tcp;
#[cfg(all(feature = "udp", feature = "tokio-host"))]
pub use interface_families::udp;
#[cfg(all(feature = "usb", any(feature = "tokio-host", feature = "embassy-host")))]
pub use interface_families::usb_auto;
#[cfg(all(feature = "weave", feature = "tokio-host"))]
pub use interface_families::weave;
#[cfg(all(feature = "websocket", feature = "tokio-host"))]
pub use interface_families::websocket;
#[cfg(all(
    feature = "wifi-auto",
    any(feature = "tokio-host", feature = "embassy-host")
))]
pub use interface_families::wifi_auto;
#[cfg(all(feature = "wifi-aware", feature = "tokio-host"))]
pub use interface_families::wifi_aware;
#[cfg(all(feature = "wifi-direct", feature = "tokio-host"))]
pub use interface_families::wifi_direct;
#[cfg(all(feature = "lora", feature = "embassy-host"))]
pub use interface_families::{lora, radios};

pub use prns_runtime::engine::{CommandId, PacketReceiptDelivered, PrnsCommand, RatchetPolicy};
pub use prns_runtime::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
pub use prns_runtime::interfaces::InterfaceStatus;
pub use prns_runtime::routing::links::resources::{ResourceMemoryLimits, ResourceStrategy};
pub use prns_runtime::routing::ProofStrategy;
pub use prns_runtime::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, PreConfiguredDestination, PrnsEvent,
    PrnsNodeApi, PrnsNodeRecipe, RuntimeHealth, SendError,
};
pub use prns_runtime::wire::{DestinationHash, TransportId};

#[cfg(feature = "alloc")]
pub use prns_runtime::storage::GrowableHeap;

#[cfg(feature = "external-alloc")]
pub use prns_runtime::storage::Esp32S3;

#[cfg(feature = "tokio-host")]
pub use prns_runtime_tokio::runtime::{
    fill_os_entropy, generate_identity_secret, load_or_create_ble_identity,
    load_or_create_browser_rendezvous_id, load_or_create_browser_selection_seed,
    load_or_create_identity_secret, try_generate_identity_secret, AttachIntent, Attachable,
    AttachedInterface, AttachedSupervisor, Fleet, IdentitySecretFileError, LocalIdentityFileError,
    OsEntropyError, PrnsNode, PrnsNodeHandle,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_runtime_embassy::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistenceFailure, EmbeddedPersistencePolicy, EmbeddedPersistenceRestoreReport,
    EmbeddedPersistenceTarget, FixedRouteSnapshotKeys, Fleet, PrnsNode, PrnsNodeHandle,
    RouteSnapshotKeyError, RouteSnapshotKeys, SharedNorFlash,
};

#[cfg(all(feature = "embassy-host", feature = "tokio-host"))]
pub use prns_runtime_embassy::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistenceFailure, EmbeddedPersistencePolicy, EmbeddedPersistenceRestoreReport,
    EmbeddedPersistenceTarget, FixedRouteSnapshotKeys, PrnsNode as EmbassyPrnsNode,
    PrnsNodeHandle as EmbassyPrnsNodeHandle, RouteSnapshotKeyError, RouteSnapshotKeys,
    SharedNorFlash,
};

#[cfg(all(
    feature = "tokio-host",
    any(feature = "wifi-auto", feature = "usb", feature = "bluetooth-auto")
))]
pub use prns_interfaces_tokio::interface_menu::DefaultAutoInterfaces;

#[cfg(all(
    feature = "usb",
    feature = "tokio-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use usb_auto::AutoUsb;

#[cfg(all(feature = "bluetooth-auto", feature = "tokio-host"))]
pub use bluetooth_auto::{AttachedBle, AttachedBluetoothLe, AutoBle, AutoBluetoothLe};

#[cfg(all(feature = "interface-discovery", feature = "tokio-host"))]
pub use prns_interfaces_tokio::interface_discovery::{
    DiscoveredConnectionFailure, DiscoveryIngressOutcome, RunningTokioInterfaceDiscoveryPublisher,
    TokioDiscoveryEvent, TokioDiscoveryIngress, TokioDiscoveryPublicationEvent,
    TokioDiscoveryPublicationFramingFailure, TokioDiscoveryPublicationPreparationFailure,
    TokioDiscoveryPublisherConstructionError, TokioInterfaceDiscovery,
    TokioInterfaceDiscoveryPublisher, DISCOVERY_PUBLICATION_JOB_INTERVAL,
};

#[cfg(all(feature = "config", feature = "tokio-host"))]
pub use prns_interfaces_tokio::from_plan::config;
#[cfg(all(feature = "config", feature = "tokio-host"))]
pub use prns_interfaces_tokio::from_plan::{
    attach_plan, attach_plan_with_context, FromPlan, PlanAttachments, PlanFailure, PlanOutcome,
    PlanRuntimeContext,
};

#[cfg(feature = "shared-instance")]
pub use prns_runtime::runtime::rns_remote_management;
