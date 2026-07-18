use std::vec::Vec;

use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    DestinationDataOperation, PacketHashArgument, RnsRpcRequest, RpcDialect, RpcRequest, RpcVerb,
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

use super::reply::{
    encode_msgpack, reply_bool, reply_empty_map, reply_int, reply_interface_stats, reply_next_hop,
    reply_next_hop_if_name, reply_none, reply_path_table, reply_rate_table,
};

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
        RpcRequest::Pickle(_) => {
            reply_for_pickle(request.verb(), request.legacy_destination_hash(), query).await
        }
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
    packet_hash: &PacketHashArgument,
) -> Option<PacketPhyStats> {
    let bytes: [u8; PACKET_HASH_LEN] = packet_hash.as_bytes().try_into().ok()?;
    query.packet_phy(PacketHash::new(bytes))
}

async fn reply_for_pickle(
    verb: RpcVerb,
    destination_hash: Option<DestinationHash>,
    query: &impl NodeIntrospection,
) -> std::io::Result<Vec<u8>> {
    let dialect = RpcDialect::Pickle;
    match verb {
        RpcVerb::GetInterfaceStats => reply_interface_stats(dialect, query.interface_inventory()),
        RpcVerb::GetRateTable => reply_rate_table(dialect, Vec::new()),
        RpcVerb::GetBlackholedIdentities => reply_empty_map(dialect),
        RpcVerb::CheckIdentityBlackholed => reply_bool(dialect, false),
        RpcVerb::GetPathTable => reply_path_table(dialect, query.routes().await, None),
        RpcVerb::GetNextHopInterfaceName => {
            reply_next_hop_if_name(dialect, legacy_route(destination_hash, query).await)
        }
        RpcVerb::GetNextHop => reply_next_hop(dialect, legacy_route(destination_hash, query).await),
        RpcVerb::GetFirstHopTimeout => reply_int(dialect, DEFAULT_PER_HOP_TIMEOUT_SECS),
        RpcVerb::GetLinkCount => reply_int(dialect, i64::from(query.link_count().await)),
        RpcVerb::DropAnnounceQueues => reply_none(dialect),
        RpcVerb::DropAllVia => reply_int(dialect, 0),
        RpcVerb::DropPath
        | RpcVerb::BlackholeIdentity
        | RpcVerb::UnblackholeIdentity
        | RpcVerb::UpdateDestinationData
        | RpcVerb::RetainIdentity => reply_bool(dialect, false),
        RpcVerb::GetPacketRssi
        | RpcVerb::GetPacketSnr
        | RpcVerb::GetPacketQuality
        | RpcVerb::Unknown => reply_none(dialect),
    }
}

async fn legacy_route(
    destination_hash: Option<DestinationHash>,
    query: &impl NodeIntrospection,
) -> Option<RouteSnapshot> {
    match destination_hash {
        Some(destination) => query.route(destination).await,
        None => None,
    }
}
