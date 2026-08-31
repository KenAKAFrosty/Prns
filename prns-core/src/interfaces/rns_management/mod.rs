use alloc::format;
use alloc::string::String;

use crate::engine::RouteSnapshot;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::NextHop;
use crate::units::InstantMillis;

mod blackhole_table;
mod interface_stats;
mod message_pack;
mod path_table;
mod rate_table;
mod remote_request;
mod status_report;
pub mod wire_names;

pub use blackhole_table::{RnsBlackholeDecodeError, RnsBlackholeTable};
pub use interface_stats::{
    RnsInterfaceAccessCode, RnsInterfaceStats, RnsInterfaceStatsEntry, RnsTransportStatus,
};
pub(crate) use message_pack::MessagePackEncoder;
pub use path_table::{RnsPathTable, RnsPathTableDecodeError, RnsPathTableEntry, RnsPathTableField};
pub use rate_table::{
    RnsAnnounceRateEntry, RnsAnnounceRateField, RnsAnnounceRateTable,
    RnsAnnounceRateTableDecodeError,
};
pub use remote_request::{
    decode_remote_path_request, decode_remote_status_request, RnsRemotePathRequest,
    RnsRemotePathTableRequest, RnsRemoteRateTableRequest, RnsRemoteRequestDecodeError,
    RnsRemoteStatusRequest,
};
pub use status_report::{
    RnsFleetPeerReport, RnsInterfaceMode, RnsInterfaceStatsDecodeError, RnsInterfaceStatsReport,
    RnsInterfaceStatusReport, RnsOptionalField, RnsRemoteInterfaceStatsReport, RnsStatsFieldPath,
    RnsStatsFieldScope,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsManagementEncodeError;

impl From<crate::message_pack::MessagePackEncodeError> for RnsManagementEncodeError {
    fn from(_: crate::message_pack::MessagePackEncodeError) -> Self {
        Self
    }
}

impl core::fmt::Display for RnsManagementEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("RNS management value exceeds MessagePack limits")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsManagementEncodeError {}

pub(crate) fn next_hop_bytes(entry: &RouteSnapshot) -> [u8; 16] {
    match entry.via {
        NextHop::Via(transport) => *transport.as_bytes(),
        NextHop::Direct => *entry.destination.as_bytes(),
    }
}

pub(crate) fn interface_name(id: InterfaceId) -> String {
    let mut name = match id.kind() {
        Some(InterfaceKind::LocalServer) => String::from("Shared Instance["),
        Some(InterfaceKind::LocalClient) => String::from("LocalInterface["),
        Some(kind) => format!("{kind:?}["),
        None => String::from("Interface["),
    };
    for byte in id.as_bytes().iter().take(4) {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push(']');
    name
}

pub(super) fn rns_timestamp(timestamp: InstantMillis) -> f64 {
    core::time::Duration::from_millis(timestamp.0).as_secs_f64()
}

#[cfg(test)]
mod tests;
