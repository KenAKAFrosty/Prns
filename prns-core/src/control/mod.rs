use alloc::string::String;
use alloc::vec::Vec;

use crate::identity::IdentityHash;
use crate::interfaces::{ConnectionState, InterfaceId, InterfaceMode, TransferRates};
use crate::routing::types::{NextHop, RouteResponsiveness};
use crate::units::InstantMillis;
use crate::units::{BitsPerSecond, ByteCount, DurationMillis, HopCount, LinkCount};
use crate::wire::{DestinationHash, TransportId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRequest {
    Query(ControlQuery),
    Command(ControlCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlQuery {
    InterfaceStats,
    PathTable { max_hops: Option<HopCount> },
    RateTable,
    LinkCount,
    NextHop { destination: DestinationHash },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    DropPath { destination: DestinationHash },
    DropAllVia { transport: TransportId },
    DropAnnounceQueues,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlReply {
    InterfaceStats(InterfaceStatsReport),
    PathTable(Vec<PathEntry>),
    RateTable(Vec<RateEntry>),
    LinkCount(LinkCount),
    NextHop(Option<NextHopReport>),
    Dropped(DropOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropOutcome {
    Path { existed: bool },
    AllVia { dropped: u32 },
    AnnounceQueues { cleared: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceStatsReport {
    pub node: NodeInfo,
    pub totals: TrafficTotals,
    pub interfaces: Vec<InterfaceStat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInfo {
    pub transport_id: IdentityHash,
    pub network_id: Option<IdentityHash>,
    pub uptime: DurationMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficTotals {
    pub rx_bytes: ByteCount,
    pub tx_bytes: ByteCount,
    pub rates: TransferRates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceStat {
    pub id: InterfaceId,
    pub name: InterfaceLabel,
    pub kind: InterfaceKind,
    pub mode: InterfaceMode,
    pub connection: ConnectionState,
    pub bitrate: Option<BitsPerSecond>,
    pub rx_bytes: ByteCount,
    pub tx_bytes: ByteCount,
    pub rates: Option<TransferRates>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceLabel {
    pub full: String,
    pub short: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceKind {
    TcpClient,
    TcpServer,
    Udp,
    Serial,
    UsbAuto,
    Local,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathEntry {
    pub destination: DestinationHash,
    pub next_hop: NextHop,
    pub hops: HopCount,
    pub learned_at: InstantMillis,
    pub expires: InstantMillis,
    pub interface: InterfaceId,
    pub responsiveness: RouteResponsiveness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextHopReport {
    pub next_hop: NextHop,
    pub interface: InterfaceId,
    pub hops: HopCount,
    pub first_hop_timeout: DurationMillis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateEntry {
    pub destination: DestinationHash,
    pub last: InstantMillis,
    pub rate_violations: u32,
    pub blocked_until: InstantMillis,
    pub timestamps: Vec<InstantMillis>,
}
