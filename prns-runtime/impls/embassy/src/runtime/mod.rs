mod embedded_persistence;
mod interface_store;
mod node_facade;
mod remote_control_authorization_exchange;
mod remote_control_controller_grants;
mod remote_control_pairing_authorizations;
mod remote_control_pairing_persistence;
mod remote_control_target_accesses;
mod request_runner;
mod shared_flash;

pub use prns_runtime::runtime::*;

pub use embedded_persistence::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistenceFailure, EmbeddedPersistencePolicy, EmbeddedPersistenceRestoreReport,
    EmbeddedPersistenceTarget, FixedRouteSnapshotKeys, RouteSnapshotKeyError, RouteSnapshotKeys,
};
pub(crate) use embedded_persistence::{ManifoldPersistence, NoManifoldPersistence};
#[cfg(test)]
pub(crate) use embedded_persistence::{
    RemoteControlAuthorizationSnapshot, RemoteControlAuthorizationSnapshotKind,
    StoreRemoteControlAuthorizationSnapshotOutcome,
};
pub use interface_store::{minimum_interface_store_capacity, EmbassyInterfaceStore};
pub(crate) use interface_store::{InterfaceInspectionStore, NoInterfaceInspectionStore};
pub use node_facade::Fleet as EmbassyFleet;
pub use node_facade::{
    minimum_manifold_notification_capacity, CompletionPool, Fleet, InboundDeliveryError,
    InterfaceLane, LaneClaimError, ManifoldLaneSet, ManifoldWiring, OutboundFrame, PrnsNode,
    PrnsNodeHandle, RemoteControlHandle, RemoteControlTargetHandle, RequestResponseData,
    RequestRoutingCapacity, StaticManifoldLane, SupervisorLane,
};
pub use remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionFailure;
pub use remote_control_pairing_persistence::{
    EmbeddedRemoteControlControllerPairingFinalization,
    EmbeddedRemoteControlPairingPersistenceFailure,
    EmbeddedRemoteControlPairingPersistenceOperation,
};
pub use shared_flash::SharedNorFlash;
