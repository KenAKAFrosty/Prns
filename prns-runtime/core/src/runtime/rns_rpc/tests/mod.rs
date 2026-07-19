use std::string::String;
use std::vec::Vec;

use prns_core::engine::RouteSnapshot;
use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use prns_core::interfaces::shared_instance::rns_rpc::RpcRequest;
use prns_core::interfaces::{
    ConnectionState, InterfaceId, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
};
use prns_core::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use prns_core::routing::types::NextHop;
use prns_core::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::wire::{DestinationHash, TransportId};
use rmpv::Value;

use super::reply as reply_for_decoded;
use crate::runtime::node_introspection::{AnnounceRateSnapshot, NodeIntrospection};
use crate::runtime::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DropRouteOutcome, DropRoutesViaOutcome,
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};

mod management;
mod queries;
mod routes;
mod support;

use support::*;

type InterfaceIfacSnapshot = crate::runtime::node_introspection::InterfaceIfacSnapshot<String>;
type InterfaceInventoryEntry = crate::runtime::node_introspection::InterfaceInventoryEntry<String>;

fn encode_msgpack(value: Value) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes)
}
