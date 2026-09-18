#[cfg(feature = "shared-instance-rpc")]
use alloc::string::String;
use core::fmt::{self, Write};

use crate::engine::RouteSnapshot;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::NextHop;
use crate::units::InstantMillis;

#[cfg(feature = "shared-instance-rpc")]
mod blackhole_table;
#[cfg(feature = "shared-instance-rpc")]
mod interface_stats;
mod message_pack;
mod path_table;
#[cfg(feature = "shared-instance-rpc")]
mod rate_table;
mod remote_request;
#[cfg(feature = "shared-instance-rpc")]
mod status_report;
pub mod wire_names;

#[cfg(feature = "shared-instance-rpc")]
pub use blackhole_table::{RnsBlackholeDecodeError, RnsBlackholeTable};
#[cfg(feature = "shared-instance-rpc")]
pub use interface_stats::{
    RnsInterfaceAccessCode, RnsInterfaceStats, RnsInterfaceStatsEntry, RnsTransportStatus,
};
#[cfg(feature = "shared-instance-rpc")]
pub(crate) use message_pack::MessagePackEncoder;
pub use path_table::{write_route_snapshots, RnsPathTableWriteError};
#[cfg(feature = "shared-instance-rpc")]
pub use path_table::{RnsPathTable, RnsPathTableDecodeError, RnsPathTableEntry, RnsPathTableField};
#[cfg(feature = "shared-instance-rpc")]
pub use rate_table::{
    RnsAnnounceRateEntry, RnsAnnounceRateField, RnsAnnounceRateTable,
    RnsAnnounceRateTableDecodeError,
};
pub use remote_request::{
    decode_remote_path_request, RnsRemotePathRequest, RnsRemotePathTableRequest,
    RnsRemoteRateTableRequest, RnsRemoteRequestDecodeError,
};
#[cfg(feature = "shared-instance-rpc")]
pub use remote_request::{decode_remote_status_request, RnsRemoteStatusRequest};
#[cfg(feature = "shared-instance-rpc")]
pub use status_report::{
    RnsInterfaceMode, RnsInterfaceStatsDecodeError, RnsInterfaceStatsReport,
    RnsInterfaceStatusReport, RnsOptionalField, RnsRemoteInterfaceStatsReport, RnsStatsFieldPath,
    RnsStatsFieldScope,
};

#[cfg(feature = "shared-instance-rpc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsManagementEncodeError;

#[cfg(feature = "shared-instance-rpc")]
impl From<crate::message_pack::MessagePackEncodeError> for RnsManagementEncodeError {
    fn from(_: crate::message_pack::MessagePackEncodeError) -> Self {
        Self
    }
}

#[cfg(feature = "shared-instance-rpc")]
impl core::fmt::Display for RnsManagementEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("RNS management value exceeds MessagePack limits")
    }
}

#[cfg(all(feature = "shared-instance-rpc", feature = "std"))]
impl std::error::Error for RnsManagementEncodeError {}

pub(crate) fn next_hop_bytes(entry: &RouteSnapshot) -> [u8; 16] {
    match entry.via {
        NextHop::Via(transport) => *transport.as_bytes(),
        NextHop::Direct => *entry.destination.as_bytes(),
    }
}

#[cfg(feature = "shared-instance-rpc")]
pub(crate) fn interface_name(id: InterfaceId) -> String {
    let mut name = String::new();
    let _ = write_interface_name(&mut name, id);
    name
}

pub(crate) fn write_interface_name(output: &mut impl Write, id: InterfaceId) -> fmt::Result {
    match id.kind() {
        Some(InterfaceKind::LocalServer) => output.write_str("Shared Instance[")?,
        Some(InterfaceKind::LocalClient) => output.write_str("LocalInterface[")?,
        Some(kind) => write!(output, "{kind:?}[")?,
        None => output.write_str("Interface[")?,
    }
    for byte in id.as_bytes().iter().take(4) {
        write!(output, "{byte:02x}")?;
    }
    output.write_char(']')
}

pub(super) fn rns_timestamp(timestamp: InstantMillis) -> f64 {
    core::time::Duration::from_millis(timestamp.0).as_secs_f64()
}

#[cfg(all(test, feature = "shared-instance-rpc"))]
mod tests;
