use std::string::String;
use std::vec::Vec;

use tokio::sync::oneshot;

use crate::identity::{IdentityHash, PublicIdentityMaterial};
use crate::wire::DestinationHash;

pub use crate::engine::RouteSnapshot;

type CoreInterfaceIfacSnapshot =
    prns_runtime::runtime::node_introspection::InterfaceIfacSnapshot<String>;
type CoreInterfaceInventoryEntry =
    prns_runtime::runtime::node_introspection::InterfaceInventoryEntry<String>;

pub(crate) use prns_runtime::runtime::node_introspection::HeapAnnounceRateHistory as AnnounceRateHistory;
pub use prns_runtime::runtime::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot, FrameAccountingCoverage,
    InterfaceTimingSnapshot, NodeIntrospection,
};
pub type InterfaceIfacSnapshot = CoreInterfaceIfacSnapshot;
pub type InterfaceInventoryEntry = CoreInterfaceInventoryEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationIdentityQuery {
    Destination(DestinationHash),
    Identity(IdentityHash),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationIdentitySnapshot {
    pub destination: DestinationHash,
    pub identity: IdentityHash,
    pub public: PublicIdentityMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInspectionSnapshot {
    /// The same engine clock used by the returned route timestamps.
    pub engine_time_millis: u64,
    pub link_count: u32,
    pub routes: Vec<RouteSnapshot>,
    pub destination_identities: Vec<DestinationIdentitySnapshot>,
}

pub enum NodeIntrospectionRequest {
    RemoteControlControllerPairingStatus {
        reply: oneshot::Sender<
            Result<
                crate::runtime::RemoteControlControllerPairingStatus,
                crate::runtime::RemoteControlPairingInspectionError,
            >,
        >,
    },
    RemoteControlControllerPairing {
        reply: oneshot::Sender<
            Result<
                crate::runtime::RemoteControlControllerPairingSnapshot,
                crate::runtime::RemoteControlPairingInspectionError,
            >,
        >,
    },
    LinkCount {
        reply: oneshot::Sender<u32>,
    },
    AnnounceRates {
        reply: oneshot::Sender<Vec<AnnounceRateSnapshot>>,
    },
    Routes {
        reply: oneshot::Sender<Vec<RouteSnapshot>>,
    },
    Route {
        destination: DestinationHash,
        reply: oneshot::Sender<Option<RouteSnapshot>>,
    },
    DestinationIdentityHash {
        destination: DestinationHash,
        reply: oneshot::Sender<Option<IdentityHash>>,
    },
    DestinationIdentity {
        query: DestinationIdentityQuery,
        reply: oneshot::Sender<Option<DestinationIdentitySnapshot>>,
    },
    DestinationIdentities {
        reply: oneshot::Sender<Vec<DestinationIdentitySnapshot>>,
    },
    EngineSnapshot {
        reply: oneshot::Sender<EngineInspectionSnapshot>,
    },
}
