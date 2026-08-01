use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    BackendInfo, DestinationHash, IdentityHash, InterfaceHealth, InterfaceId, InterfaceKind,
    PersistenceFlushCause,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub interface_id: InterfaceId,
    pub name: Option<String>,
    pub kind: Option<InterfaceKind>,
    pub health: InterfaceHealth,
    pub failure_detail: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub destination: DestinationHash,
    pub hops: u8,
    pub via_identity: Option<IdentityHash>,
    pub interface_id: InterfaceId,
    pub learned_at_millis: u64,
    pub last_relayed_at_millis: u64,
    pub expires_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DestinationIdentitySnapshot {
    pub destination: DestinationHash,
    pub identity: IdentityHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeHealthSnapshot {
    pub running: bool,
    pub uptime_millis: u64,
    pub interface_count: u32,
    pub online_interface_count: u32,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistenceSnapshot {
    pub persistent: bool,
    pub restored: bool,
    pub last_flush_cause: Option<PersistenceFlushCause>,
    pub last_failure_detail: Option<String>,
}

impl PersistenceSnapshot {
    #[must_use]
    pub const fn ephemeral() -> Self {
        Self {
            persistent: false,
            restored: false,
            last_flush_cause: None,
            last_failure_detail: None,
        }
    }

    #[must_use]
    pub const fn persistent() -> Self {
        Self {
            persistent: true,
            restored: false,
            last_flush_cause: None,
            last_failure_detail: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSnapshot {
    pub revision: u64,
    pub backend: BackendInfo,
    pub interfaces: Vec<InterfaceSnapshot>,
    pub routes: Vec<RouteSnapshot>,
    pub active_link_count: u32,
    pub destination_identities: Vec<DestinationIdentitySnapshot>,
    pub runtime: RuntimeHealthSnapshot,
    pub persistence: PersistenceSnapshot,
}
