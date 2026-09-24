mod discovery_group_store;
mod embedded_persistence;
mod entropy;
mod interface_store;
mod node_facade;
mod node_name_store;
mod remote_control_authorization_exchange;
mod remote_control_controller_grants;
mod remote_control_pairing_authorizations;
mod remote_control_pairing_persistence;
mod remote_control_target_accesses;
mod request_runner;
mod shared_flash;

pub use prns_runtime::runtime::*;

pub use discovery_group_store::{
    restored_discovery_group_configuration, restored_discovery_group_configuration_now,
    restored_discovery_groups, restored_discovery_groups_now, store_discovery_group_configuration,
    DiscoveryGroupConfigurationStoreExchange, GlobalDiscoveryGroupStore,
};
pub use embedded_persistence::{
    DiscoveryGroupConfigurationChange, EmbeddedCompactionPolicy, EmbeddedFlashPersistence,
    EmbeddedPersistenceDiagnostic, EmbeddedPersistenceFailure, EmbeddedPersistencePolicy,
    EmbeddedPersistenceRestoreReport, EmbeddedPersistenceTarget, FixedRouteSnapshotKeys,
    RouteSnapshotKeyError, RouteSnapshotKeys,
};
pub(crate) use embedded_persistence::{ManifoldPersistence, NoManifoldPersistence};
#[cfg(test)]
pub(crate) use embedded_persistence::{
    RemoteControlAuthorizationSnapshot, RemoteControlAuthorizationSnapshotKind,
    StoreRemoteControlAuthorizationSnapshotOutcome,
};
pub use entropy::{EntropyHandle, SharedRuntimeEntropy};
pub use interface_store::{minimum_interface_store_capacity, EmbassyInterfaceStore};
pub(crate) use interface_store::{InterfaceInspectionStore, NoInterfaceInspectionStore};
pub use node_facade::Fleet as EmbassyFleet;
pub use node_facade::ResourceResponse;
pub(crate) use node_facade::ResourceResponsePayload;
pub use node_facade::{
    minimum_manifold_notification_capacity, CompletionPool, Fleet, InboundDeliveryError,
    InterfaceLane, LaneClaimError, ManifoldLaneSet, ManifoldWiring, OutboundFrame, PrnsNode,
    PrnsNodeHandle, RemoteControlHandle, RemoteControlTargetHandle, RequestResponseData,
    RequestRoutingCapacity, StaticManifoldLane, SupervisorLane,
};
pub use node_name_store::{
    restored_node_name, restored_node_name_now, store_node_name, NodeNameStoreExchange,
};
pub use remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionFailure;
pub use remote_control_pairing_persistence::{
    EmbeddedRemoteControlControllerPairingFinalization,
    EmbeddedRemoteControlPairingPersistenceFailure,
    EmbeddedRemoteControlPairingPersistenceOperation,
    EmbeddedRemoteControlTargetPairingFinalization,
};
pub use shared_flash::SharedNorFlash;
