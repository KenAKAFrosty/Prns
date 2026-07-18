use std::vec::Vec;

use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
};
use prns_core::interfaces::PacketPhyStats;
use prns_core::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use prns_core::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::wire::DestinationHash;
use prns_runtime::node_introspection::{NodeIntrospection, RouteSnapshot};
use prns_runtime::runtime::{
    DestinationIdentityRetentionControl, DestinationIdentityRetentionControlError,
    DropRouteOutcome, IdentityBlackholeControl, IdentityBlackholeControlError,
    IdentityBlackholeSource, IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};
use rmpv::Value;

use crate::shared_instance::blackhole_compat::table_value as blackhole_table_value;

use super::protocol::{contains, position_of, RpcDialect, RpcRequest};
use super::reply::{
    encode_msgpack, reply_bool, reply_empty_map, reply_int, reply_interface_stats, reply_next_hop,
    reply_next_hop_if_name, reply_none, reply_path_table, reply_rate_table,
};
use super::request::{self, DestinationDataOperation, RnsRpcRequest};

const DEFAULT_PER_HOP_TIMEOUT_SECS: i64 = 6;

pub(super) async fn reply_for_decoded<B>(
    request: &RpcRequest<'_>,
    query: &impl NodeIntrospection,
    control: &impl RoutingControl,
    retention: &impl DestinationIdentityRetentionControl,
    blackholes: &B,
    blackhole_source: IdentityHash,
) -> std::io::Result<Vec<u8>>
where
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    match request {
        RpcRequest::Pickle(request) => reply_for_pickle(request, query).await,
        RpcRequest::Msgpack(request) => {
            reply_for_msgpack(
                request,
                query,
                control,
                retention,
                blackholes,
                blackhole_source,
            )
            .await
        }
    }
}

async fn reply_for_msgpack<B>(
    request: &RnsRpcRequest,
    query: &impl NodeIntrospection,
    control: &impl RoutingControl,
    retention: &impl DestinationIdentityRetentionControl,
    blackholes: &B,
    blackhole_source: IdentityHash,
) -> std::io::Result<Vec<u8>>
where
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    let dialect = RpcDialect::Msgpack;
    match request {
        RnsRpcRequest::InterfaceStats => {
            reply_interface_stats(dialect, query.interface_inventory())
        }

        RnsRpcRequest::PathTable { max_hops } => {
            reply_path_table(dialect, query.routes().await, max_hops.as_ref())
        }

        RnsRpcRequest::RateTable => reply_rate_table(dialect, query.announce_rates().await),

        RnsRpcRequest::NextHopInterface { destination_hash } => {
            reply_next_hop_if_name(dialect, query.route(*destination_hash).await)
        }

        RnsRpcRequest::NextHop { destination_hash } => {
            reply_next_hop(dialect, query.route(*destination_hash).await)
        }

        RnsRpcRequest::FirstHopTimeout { .. } => reply_int(dialect, DEFAULT_PER_HOP_TIMEOUT_SECS),

        RnsRpcRequest::LinkCount => reply_int(dialect, i64::from(query.link_count().await)),

        RnsRpcRequest::PacketRssi { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.rssi) {
                Some(rssi) => reply_int(dialect, i64::from(rssi.get())),
                None => reply_none(dialect),
            }
        }

        RnsRpcRequest::PacketSnr { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.snr) {
                Some(snr) => encode_msgpack(Value::F64(f64::from(snr.quarters()) / 4.0)),
                None => reply_none(dialect),
            }
        }

        RnsRpcRequest::PacketQuality { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.quality) {
                Some(quality) => {
                    encode_msgpack(Value::F64(f64::from(quality.tenths_percent()) / 10.0))
                }
                None => reply_none(dialect),
            }
        }

        RnsRpcRequest::BlackholedIdentities => match blackholes.blackholed_identities().await {
            Ok(entries) => encode_msgpack(blackhole_table_value(entries)),
            Err(IdentityBlackholeSourceError::NodeStopped | IdentityBlackholeSourceError::Busy) => {
                reply_empty_map(dialect)
            }
        },

        RnsRpcRequest::DropPath { destination_hash } => {
            let dropped = match control.drop_route(*destination_hash).await {
                Ok(DropRouteOutcome::Dropped) => true,
                Ok(DropRouteOutcome::NotFound)
                | Err(RoutingControlError::NodeStopped | RoutingControlError::Busy) => false,
            };
            reply_bool(dialect, dropped)
        }

        RnsRpcRequest::DropAllVia { transport_id } => {
            let dropped = match control.drop_routes_via(*transport_id).await {
                Ok(outcome) => outcome.dropped_routes,
                Err(RoutingControlError::NodeStopped | RoutingControlError::Busy) => 0,
            };
            reply_int(dialect, i64::from(dropped))
        }

        RnsRpcRequest::DropAnnounceQueues => {
            let _ = control.clear_announce_queues().await;
            reply_none(dialect)
        }

        RnsRpcRequest::IsBlackholed { identity_hash } => {
            let blackholed = blackholes
                .is_blackholed(*identity_hash)
                .await
                .is_ok_and(|blackholed| blackholed);
            reply_bool(dialect, blackholed)
        }

        RnsRpcRequest::BlackholeIdentity {
            identity_hash,
            until,
            reason,
        } => {
            let expiry = until.as_ref().map_or(BlackholeExpiry::Indefinite, |until| {
                until.blackhole_expiry()
            });
            match blackholes
                .blackhole_identity(BlackholedIdentity {
                    identity: *identity_hash,
                    source: blackhole_source,
                    expiry,
                    reason: reason.as_deref(),
                })
                .await
            {
                Ok(BlackholeIdentityOutcome::Added) => reply_bool(dialect, true),
                Ok(BlackholeIdentityOutcome::AlreadyPresent) => reply_none(dialect),
                Err(
                    IdentityBlackholeControlError::NodeStopped
                    | IdentityBlackholeControlError::Busy
                    | IdentityBlackholeControlError::CapacityExhausted
                    | IdentityBlackholeControlError::ReasonTooLong
                    | IdentityBlackholeControlError::DurabilityFailed,
                ) => reply_bool(dialect, false),
            }
        }

        RnsRpcRequest::UnblackholeIdentity { identity_hash } => {
            match blackholes.unblackhole_identity(*identity_hash).await {
                Ok(UnblackholeIdentityOutcome::Removed) => reply_bool(dialect, true),
                Ok(UnblackholeIdentityOutcome::NotFound) => reply_none(dialect),
                Err(
                    IdentityBlackholeControlError::NodeStopped
                    | IdentityBlackholeControlError::Busy
                    | IdentityBlackholeControlError::CapacityExhausted
                    | IdentityBlackholeControlError::ReasonTooLong
                    | IdentityBlackholeControlError::DurabilityFailed,
                ) => reply_bool(dialect, false),
            }
        }

        RnsRpcRequest::DestinationData {
            operation,
            destination_hash,
        } => {
            let succeeded = match operation {
                DestinationDataOperation::Used => {
                    match retention.mark_destination_used(*destination_hash).await {
                        Ok(
                            MarkDestinationUsedOutcome::Recorded
                            | MarkDestinationUsedOutcome::Refreshed,
                        ) => true,
                        Ok(
                            MarkDestinationUsedOutcome::Retained
                            | MarkDestinationUsedOutcome::NotFound,
                        )
                        | Err(
                            DestinationIdentityRetentionControlError::NodeStopped
                            | DestinationIdentityRetentionControlError::Busy,
                        ) => false,
                    }
                }
                DestinationDataOperation::Retain => {
                    match retention.retain_destination(*destination_hash).await {
                        Ok(
                            RetainDestinationOutcome::Retained
                            | RetainDestinationOutcome::AlreadyRetained,
                        ) => true,
                        Ok(RetainDestinationOutcome::NotFound)
                        | Err(
                            DestinationIdentityRetentionControlError::NodeStopped
                            | DestinationIdentityRetentionControlError::Busy,
                        ) => false,
                    }
                }
                DestinationDataOperation::Unretain => {
                    match retention.release_destination(*destination_hash).await {
                        Ok(
                            ReleaseDestinationOutcome::Released
                            | ReleaseDestinationOutcome::UseRecorded
                            | ReleaseDestinationOutcome::UseRefreshed,
                        ) => true,
                        Ok(ReleaseDestinationOutcome::NotFound)
                        | Err(
                            DestinationIdentityRetentionControlError::NodeStopped
                            | DestinationIdentityRetentionControlError::Busy,
                        ) => false,
                    }
                }
            };
            reply_bool(dialect, succeeded)
        }

        RnsRpcRequest::RetainIdentity { identity_hash } => {
            let retained = match retention.retain_identity(*identity_hash).await {
                Ok(outcome) => {
                    outcome.newly_retained_destination_count != 0
                        || outcome.already_retained_destination_count != 0
                }
                Err(
                    DestinationIdentityRetentionControlError::NodeStopped
                    | DestinationIdentityRetentionControlError::Busy,
                ) => false,
            };
            reply_bool(dialect, retained)
        }
    }
}

fn packet_phy(
    query: &impl NodeIntrospection,
    packet_hash: &request::PacketHashArgument,
) -> Option<PacketPhyStats> {
    let bytes: [u8; PACKET_HASH_LEN] = packet_hash.as_bytes().try_into().ok()?;
    query.packet_phy(PacketHash::new(bytes))
}

async fn reply_for_pickle(
    request: &[u8],
    query: &impl NodeIntrospection,
) -> std::io::Result<Vec<u8>> {
    let dialect = RpcDialect::Pickle;
    if contains(request, b"interface_stats") {
        reply_interface_stats(dialect, query.interface_inventory())
    } else if contains(request, b"rate_table") {
        reply_rate_table(dialect, Vec::new())
    } else if contains(request, b"blackholed_identities") {
        reply_empty_map(dialect)
    } else if contains(request, b"is_blackholed") {
        reply_bool(dialect, false)
    } else if contains(request, b"path_table") {
        reply_path_table(dialect, query.routes().await, None)
    } else if contains(request, b"next_hop_if_name") {
        reply_next_hop_if_name(dialect, legacy_route_arg(request, query).await)
    } else if contains(request, b"next_hop") {
        reply_next_hop(dialect, legacy_route_arg(request, query).await)
    } else if contains(request, b"first_hop_timeout") {
        reply_int(dialect, DEFAULT_PER_HOP_TIMEOUT_SECS)
    } else if contains(request, b"link_count") {
        reply_int(dialect, i64::from(query.link_count().await))
    } else if contains(request, b"drop") && contains(request, b"announce_queues") {
        reply_none(dialect)
    } else if contains(request, b"drop") && contains(request, b"all_via") {
        reply_int(dialect, 0)
    } else if (contains(request, b"drop") && contains(request, b"path"))
        || contains(request, b"blackhole_identity")
        || contains(request, b"unblackhole_identity")
        || contains(request, b"destination_data")
        || contains(request, b"identity_data")
    {
        reply_bool(dialect, false)
    } else {
        reply_none(dialect)
    }
}

async fn legacy_route_arg(request: &[u8], query: &impl NodeIntrospection) -> Option<RouteSnapshot> {
    match legacy_destination_hash_arg(request) {
        Some(destination) => query.route(destination).await,
        None => None,
    }
}

fn legacy_destination_hash_arg(request: &[u8]) -> Option<DestinationHash> {
    let key_end = position_of(request, b"destination_hash")? + b"destination_hash".len();
    let tail = &request[key_end..];
    let value_start = tail
        .windows(2)
        .position(|window| matches!(window, [0x43, 0x10]))?
        + 2;
    let bytes: [u8; 16] = tail.get(value_start..value_start + 16)?.try_into().ok()?;
    Some(DestinationHash::new(bytes))
}
