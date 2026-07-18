use std::vec::Vec;

use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    DestinationDataOperation, PacketHashArgument, RnsRpcReply, RnsRpcRequest, RpcRequest, RpcVerb,
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

use super::projections::{announce_rate_table, interface_stats};

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
    let reply = match request {
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
    };
    reply
        .encode(request.dialect())
        .map_err(std::io::Error::other)
}

async fn reply_for_msgpack<B>(
    request: &RnsRpcRequest,
    query: &impl NodeIntrospection,
    control: &impl RoutingControl,
    retention: &impl DestinationIdentityRetentionControl,
    blackholes: &B,
    blackhole_source: IdentityHash,
) -> RnsRpcReply
where
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    match request {
        RnsRpcRequest::InterfaceStats => {
            RnsRpcReply::interface_stats(interface_stats(query.interface_inventory()))
        }

        RnsRpcRequest::PathTable { max_hops } => {
            RnsRpcReply::path_table(query.routes().await, max_hops.as_ref())
        }

        RnsRpcRequest::RateTable => {
            RnsRpcReply::announce_rate_table(announce_rate_table(query.announce_rates().await))
        }

        RnsRpcRequest::NextHopInterface { destination_hash } => {
            RnsRpcReply::next_hop_interface_name(query.route(*destination_hash).await)
        }

        RnsRpcRequest::NextHop { destination_hash } => {
            RnsRpcReply::next_hop(query.route(*destination_hash).await)
        }

        RnsRpcRequest::FirstHopTimeout { .. } => RnsRpcReply::integer(DEFAULT_PER_HOP_TIMEOUT_SECS),

        RnsRpcRequest::LinkCount => RnsRpcReply::integer(i64::from(query.link_count().await)),

        RnsRpcRequest::PacketRssi { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.rssi) {
                Some(rssi) => RnsRpcReply::integer(i64::from(rssi.get())),
                None => RnsRpcReply::none(),
            }
        }

        RnsRpcRequest::PacketSnr { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.snr) {
                Some(snr) => RnsRpcReply::float(f64::from(snr.quarters()) / 4.0),
                None => RnsRpcReply::none(),
            }
        }

        RnsRpcRequest::PacketQuality { packet_hash } => {
            match packet_phy(query, packet_hash).and_then(|stats| stats.quality) {
                Some(quality) => RnsRpcReply::float(f64::from(quality.tenths_percent()) / 10.0),
                None => RnsRpcReply::none(),
            }
        }

        RnsRpcRequest::BlackholedIdentities => match blackholes.blackholed_identities().await {
            Ok(entries) => RnsRpcReply::blackholed_identities(entries),
            Err(IdentityBlackholeSourceError::NodeStopped | IdentityBlackholeSourceError::Busy) => {
                RnsRpcReply::empty_blackhole_table()
            }
        },

        RnsRpcRequest::DropPath { destination_hash } => {
            let dropped = match control.drop_route(*destination_hash).await {
                Ok(DropRouteOutcome::Dropped) => true,
                Ok(DropRouteOutcome::NotFound)
                | Err(RoutingControlError::NodeStopped | RoutingControlError::Busy) => false,
            };
            RnsRpcReply::boolean(dropped)
        }

        RnsRpcRequest::DropAllVia { transport_id } => {
            let dropped = match control.drop_routes_via(*transport_id).await {
                Ok(outcome) => outcome.dropped_routes,
                Err(RoutingControlError::NodeStopped | RoutingControlError::Busy) => 0,
            };
            RnsRpcReply::integer(i64::from(dropped))
        }

        RnsRpcRequest::DropAnnounceQueues => {
            let _ = control.clear_announce_queues().await;
            RnsRpcReply::none()
        }

        RnsRpcRequest::IsBlackholed { identity_hash } => {
            let blackholed = blackholes
                .is_blackholed(*identity_hash)
                .await
                .is_ok_and(|blackholed| blackholed);
            RnsRpcReply::boolean(blackholed)
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
                Ok(BlackholeIdentityOutcome::Added) => RnsRpcReply::boolean(true),
                Ok(BlackholeIdentityOutcome::AlreadyPresent) => RnsRpcReply::none(),
                Err(
                    IdentityBlackholeControlError::NodeStopped
                    | IdentityBlackholeControlError::Busy
                    | IdentityBlackholeControlError::CapacityExhausted
                    | IdentityBlackholeControlError::ReasonTooLong
                    | IdentityBlackholeControlError::DurabilityFailed,
                ) => RnsRpcReply::boolean(false),
            }
        }

        RnsRpcRequest::UnblackholeIdentity { identity_hash } => {
            match blackholes.unblackhole_identity(*identity_hash).await {
                Ok(UnblackholeIdentityOutcome::Removed) => RnsRpcReply::boolean(true),
                Ok(UnblackholeIdentityOutcome::NotFound) => RnsRpcReply::none(),
                Err(
                    IdentityBlackholeControlError::NodeStopped
                    | IdentityBlackholeControlError::Busy
                    | IdentityBlackholeControlError::CapacityExhausted
                    | IdentityBlackholeControlError::ReasonTooLong
                    | IdentityBlackholeControlError::DurabilityFailed,
                ) => RnsRpcReply::boolean(false),
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
            RnsRpcReply::boolean(succeeded)
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
            RnsRpcReply::boolean(retained)
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
) -> RnsRpcReply {
    match verb {
        RpcVerb::GetInterfaceStats => {
            RnsRpcReply::interface_stats(interface_stats(query.interface_inventory()))
        }
        RpcVerb::GetRateTable => RnsRpcReply::announce_rate_table(announce_rate_table(Vec::new())),
        RpcVerb::GetBlackholedIdentities => RnsRpcReply::empty_blackhole_table(),
        RpcVerb::CheckIdentityBlackholed => RnsRpcReply::boolean(false),
        RpcVerb::GetPathTable => RnsRpcReply::path_table(query.routes().await, None),
        RpcVerb::GetNextHopInterfaceName => {
            RnsRpcReply::next_hop_interface_name(legacy_route(destination_hash, query).await)
        }
        RpcVerb::GetNextHop => RnsRpcReply::next_hop(legacy_route(destination_hash, query).await),
        RpcVerb::GetFirstHopTimeout => RnsRpcReply::integer(DEFAULT_PER_HOP_TIMEOUT_SECS),
        RpcVerb::GetLinkCount => RnsRpcReply::integer(i64::from(query.link_count().await)),
        RpcVerb::DropAnnounceQueues => RnsRpcReply::none(),
        RpcVerb::DropAllVia => RnsRpcReply::integer(0),
        RpcVerb::DropPath
        | RpcVerb::BlackholeIdentity
        | RpcVerb::UnblackholeIdentity
        | RpcVerb::UpdateDestinationData
        | RpcVerb::RetainIdentity => RnsRpcReply::boolean(false),
        RpcVerb::GetPacketRssi
        | RpcVerb::GetPacketSnr
        | RpcVerb::GetPacketQuality
        | RpcVerb::Unknown => RnsRpcReply::none(),
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
