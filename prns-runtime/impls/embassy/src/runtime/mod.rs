mod embedded_persistence;
mod interface_store;
mod node_facade;
mod remote_control_access;
mod request_runner;
mod shared_flash;

pub use prns_runtime::runtime::*;

pub use embedded_persistence::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistenceFailure, EmbeddedPersistencePolicy, EmbeddedPersistenceRestoreReport,
    EmbeddedPersistenceTarget, FixedRouteSnapshotKeys, RouteSnapshotKeyError, RouteSnapshotKeys,
};
pub(crate) use embedded_persistence::{ManifoldPersistence, NoManifoldPersistence};
pub use interface_store::{minimum_interface_store_capacity, EmbassyInterfaceStore};
pub(crate) use interface_store::{InterfaceInspectionStore, NoInterfaceInspectionStore};
pub use node_facade::Fleet as EmbassyFleet;
pub use node_facade::{
    minimum_manifold_notification_capacity, CompletionPool, Fleet, InboundDeliveryError,
    InterfaceLane, LaneClaimError, ManifoldLaneSet, ManifoldWiring, OutboundFrame, PrnsNode,
    PrnsNodeHandle, RemoteControlHandle, RequestResponseData, RequestRoutingCapacity,
    StaticManifoldLane, SupervisorLane,
};
pub use shared_flash::SharedNorFlash;
