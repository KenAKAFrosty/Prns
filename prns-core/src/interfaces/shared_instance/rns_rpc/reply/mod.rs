use alloc::string::String;
use alloc::vec::Vec;

use crate::engine::RouteSnapshot;
use crate::interfaces::rns_management::{
    interface_name, next_hop_bytes, MessagePackEncoder, RnsAnnounceRateTable, RnsBlackholeTable,
    RnsInterfaceStats, RnsManagementEncodeError, RnsPathTable,
};
use crate::routing::BlackholedIdentity;

use super::wire_names::reply_value;
use super::{RnsInteger, RpcDialect};

mod legacy;
mod outcomes;
mod pickle;
mod scalar;

pub use legacy::LegacyRpcReplyPlan;
pub use outcomes::RpcOperationOutcome;
pub use scalar::{RnsRpcScalarReply, RnsRpcScalarReplyDecodeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsRpcReplyEncodeError;

impl core::fmt::Display for RnsRpcReplyEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("RNS RPC reply exceeds wire encoding limits")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsRpcReplyEncodeError {}

impl From<RnsManagementEncodeError> for RnsRpcReplyEncodeError {
    fn from(_: RnsManagementEncodeError) -> Self {
        Self
    }
}

impl From<crate::message_pack::MessagePackEncodeError> for RnsRpcReplyEncodeError {
    fn from(_: crate::message_pack::MessagePackEncodeError) -> Self {
        Self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsRpcReply(RnsRpcReplyKind);

#[derive(Debug, Clone, PartialEq)]
enum RnsRpcReplyKind {
    None,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    NextHop(Option<[u8; 16]>),
    NextHopInterfaceName(String),
    PathTable(RnsPathTable),
    AnnounceRateTable(RnsAnnounceRateTable),
    InterfaceStats(RnsInterfaceStats),
    BlackholeTable(RnsBlackholeTable),
}

impl RnsRpcReply {
    pub const fn none() -> Self {
        Self(RnsRpcReplyKind::None)
    }

    pub const fn boolean(value: bool) -> Self {
        Self(RnsRpcReplyKind::Boolean(value))
    }

    pub const fn integer(value: i64) -> Self {
        Self(RnsRpcReplyKind::Integer(value))
    }

    pub const fn float(value: f64) -> Self {
        Self(RnsRpcReplyKind::Float(value))
    }

    pub fn next_hop(route: Option<RouteSnapshot>) -> Self {
        Self(RnsRpcReplyKind::NextHop(route.as_ref().map(next_hop_bytes)))
    }

    pub fn next_hop_interface_name(route: Option<RouteSnapshot>) -> Self {
        let name = route.map_or_else(
            || String::from(reply_value::NO_INTERFACE),
            |route| interface_name(route.interface),
        );
        Self(RnsRpcReplyKind::NextHopInterfaceName(name))
    }

    pub fn path_table(mut entries: Vec<RouteSnapshot>, maximum_hops: Option<&RnsInteger>) -> Self {
        entries.retain(|entry| within_hop_limit(entry.hops, maximum_hops));
        Self(RnsRpcReplyKind::PathTable(RnsPathTable::new(entries)))
    }

    pub fn announce_rate_table(table: RnsAnnounceRateTable) -> Self {
        Self(RnsRpcReplyKind::AnnounceRateTable(table))
    }

    pub fn interface_stats(stats: RnsInterfaceStats) -> Self {
        Self(RnsRpcReplyKind::InterfaceStats(stats))
    }

    fn blackhole_table<Reason: AsRef<str>>(
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Self {
        Self(RnsRpcReplyKind::BlackholeTable(
            RnsBlackholeTable::from_entries(entries),
        ))
    }

    pub fn empty_blackhole_table() -> Self {
        Self(RnsRpcReplyKind::BlackholeTable(RnsBlackholeTable::empty()))
    }

    pub fn encode(&self, dialect: RpcDialect) -> Result<Vec<u8>, RnsRpcReplyEncodeError> {
        match dialect {
            RpcDialect::Pickle => Ok(pickle::encode(&self.0)),
            RpcDialect::Msgpack => self.encode_message_pack(),
        }
    }

    fn encode_message_pack(&self) -> Result<Vec<u8>, RnsRpcReplyEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        match &self.0 {
            RnsRpcReplyKind::None => encoder.nil(),
            RnsRpcReplyKind::Boolean(value) => encoder.boolean(*value),
            RnsRpcReplyKind::Integer(value) => encoder.signed(*value),
            RnsRpcReplyKind::Float(value) => encoder.float(*value),
            RnsRpcReplyKind::NextHop(Some(value)) => encoder.binary(value)?,
            RnsRpcReplyKind::NextHop(None) => encoder.nil(),
            RnsRpcReplyKind::NextHopInterfaceName(value) => encoder.string(value)?,
            RnsRpcReplyKind::PathTable(table) => table.encode_into(&mut encoder)?,
            RnsRpcReplyKind::AnnounceRateTable(table) => table.encode_into(&mut encoder)?,
            RnsRpcReplyKind::InterfaceStats(stats) => stats.encode_into(&mut encoder)?,
            RnsRpcReplyKind::BlackholeTable(table) => table.encode_into(&mut encoder)?,
        }
        Ok(encoder.finish())
    }
}

fn within_hop_limit(hops: u8, maximum: Option<&RnsInteger>) -> bool {
    maximum.is_none_or(|maximum| {
        maximum
            .nonnegative_value()
            .is_some_and(|maximum| u64::from(hops) <= maximum)
    })
}

#[cfg(test)]
mod tests;
