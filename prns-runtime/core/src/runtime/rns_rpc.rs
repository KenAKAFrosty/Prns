use alloc::vec::Vec;

use prns_core::identity::IdentityHash;
use prns_core::interfaces::shared_instance::rns_rpc::{
    DestinationDataOperation, LegacyRpcReplyPlan, RnsRpcReply, RnsRpcReplyEncodeError,
    RnsRpcRequest, RpcOperationOutcome, RpcRequest, RpcVerb,
};
use prns_core::routing::{BlackholeExpiry, BlackholedIdentity};
use prns_core::wire::DestinationHash;

use super::node_introspection::NodeIntrospection;
use super::rns_management::{announce_rate_table, interface_stats};
use super::{
    DestinationIdentityRetentionControl, DestinationIdentityRetentionControlError,
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};

#[cfg(test)]
mod tests;

pub async fn reply<B>(
    request: &RpcRequest<'_>,
    query: &impl NodeIntrospection,
    control: &impl RoutingControl,
    retention: &impl DestinationIdentityRetentionControl,
    blackholes: &B,
    blackhole_source: IdentityHash,
) -> Result<Vec<u8>, RnsRpcReplyEncodeError>
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
    reply.encode(request.dialect())
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

        RnsRpcRequest::FirstHopTimeout { .. } => RnsRpcReply::first_hop_timeout(),

        RnsRpcRequest::LinkCount => RnsRpcReply::integer(i64::from(query.link_count().await)),

        RnsRpcRequest::PacketRssi { packet_hash } => RnsRpcReply::packet_rssi(
            packet_hash
                .packet_hash()
                .and_then(|packet_hash| query.packet_phy(packet_hash)),
        ),

        RnsRpcRequest::PacketSnr { packet_hash } => RnsRpcReply::packet_snr(
            packet_hash
                .packet_hash()
                .and_then(|packet_hash| query.packet_phy(packet_hash)),
        ),

        RnsRpcRequest::PacketQuality { packet_hash } => RnsRpcReply::packet_quality(
            packet_hash
                .packet_hash()
                .and_then(|packet_hash| query.packet_phy(packet_hash)),
        ),

        RnsRpcRequest::BlackholedIdentities => RnsRpcReply::blackholed_identities(
            blackhole_source_outcome(blackholes.blackholed_identities().await),
        ),

        RnsRpcRequest::DropPath { destination_hash } => RnsRpcReply::drop_path(
            routing_control_outcome(control.drop_route(*destination_hash).await),
        ),

        RnsRpcRequest::DropAllVia { transport_id } => RnsRpcReply::drop_all_via(
            routing_control_outcome(control.drop_routes_via(*transport_id).await),
        ),

        RnsRpcRequest::DropAnnounceQueues => {
            let _ = control.clear_announce_queues().await;
            RnsRpcReply::drop_announce_queues()
        }

        RnsRpcRequest::IsBlackholed { identity_hash } => RnsRpcReply::is_blackholed(
            blackhole_source_outcome(blackholes.is_blackholed(*identity_hash).await),
        ),

        RnsRpcRequest::BlackholeIdentity {
            identity_hash,
            until,
            reason,
        } => {
            let expiry = until.as_ref().map_or(BlackholeExpiry::Indefinite, |until| {
                until.blackhole_expiry()
            });
            RnsRpcReply::blackhole_identity(blackhole_control_outcome(
                blackholes
                    .blackhole_identity(BlackholedIdentity {
                        identity: *identity_hash,
                        source: blackhole_source,
                        expiry,
                        reason: reason.as_deref(),
                    })
                    .await,
            ))
        }

        RnsRpcRequest::UnblackholeIdentity { identity_hash } => RnsRpcReply::unblackhole_identity(
            blackhole_control_outcome(blackholes.unblackhole_identity(*identity_hash).await),
        ),

        RnsRpcRequest::DestinationData {
            operation,
            destination_hash,
        } => match operation {
            DestinationDataOperation::Used => RnsRpcReply::mark_destination_used(
                retention_control_outcome(retention.mark_destination_used(*destination_hash).await),
            ),
            DestinationDataOperation::Retain => RnsRpcReply::retain_destination(
                retention_control_outcome(retention.retain_destination(*destination_hash).await),
            ),
            DestinationDataOperation::Unretain => RnsRpcReply::release_destination(
                retention_control_outcome(retention.release_destination(*destination_hash).await),
            ),
        },

        RnsRpcRequest::RetainIdentity { identity_hash } => RnsRpcReply::retain_identity(
            retention_control_outcome(retention.retain_identity(*identity_hash).await),
        ),
    }
}

async fn reply_for_pickle(
    verb: RpcVerb,
    destination_hash: Option<DestinationHash>,
    query: &impl NodeIntrospection,
) -> RnsRpcReply {
    match LegacyRpcReplyPlan::for_request(verb, destination_hash) {
        LegacyRpcReplyPlan::InterfaceStats => {
            RnsRpcReply::interface_stats(interface_stats(query.interface_inventory()))
        }
        LegacyRpcReplyPlan::PathTable => RnsRpcReply::path_table(query.routes().await, None),
        LegacyRpcReplyPlan::NextHopInterfaceName(destination_hash) => {
            RnsRpcReply::next_hop_interface_name(query.route(destination_hash).await)
        }
        LegacyRpcReplyPlan::NextHop(destination_hash) => {
            RnsRpcReply::next_hop(query.route(destination_hash).await)
        }
        LegacyRpcReplyPlan::LinkCount => RnsRpcReply::integer(i64::from(query.link_count().await)),
        LegacyRpcReplyPlan::Immediate(reply) => reply,
    }
}

fn routing_control_outcome<T>(result: Result<T, RoutingControlError>) -> RpcOperationOutcome<T> {
    match result {
        Ok(outcome) => RpcOperationOutcome::Completed(outcome),
        Err(RoutingControlError::NodeStopped | RoutingControlError::Busy) => {
            RpcOperationOutcome::Unavailable
        }
    }
}

fn retention_control_outcome<T>(
    result: Result<T, DestinationIdentityRetentionControlError>,
) -> RpcOperationOutcome<T> {
    match result {
        Ok(outcome) => RpcOperationOutcome::Completed(outcome),
        Err(
            DestinationIdentityRetentionControlError::NodeStopped
            | DestinationIdentityRetentionControlError::Busy,
        ) => RpcOperationOutcome::Unavailable,
    }
}

fn blackhole_source_outcome<T>(
    result: Result<T, IdentityBlackholeSourceError>,
) -> RpcOperationOutcome<T> {
    match result {
        Ok(outcome) => RpcOperationOutcome::Completed(outcome),
        Err(IdentityBlackholeSourceError::NodeStopped | IdentityBlackholeSourceError::Busy) => {
            RpcOperationOutcome::Unavailable
        }
    }
}

fn blackhole_control_outcome<T>(
    result: Result<T, IdentityBlackholeControlError>,
) -> RpcOperationOutcome<T> {
    match result {
        Ok(outcome) => RpcOperationOutcome::Completed(outcome),
        Err(
            IdentityBlackholeControlError::NodeStopped
            | IdentityBlackholeControlError::Busy
            | IdentityBlackholeControlError::CapacityExhausted
            | IdentityBlackholeControlError::ReasonTooLong
            | IdentityBlackholeControlError::DurabilityFailed,
        ) => RpcOperationOutcome::Unavailable,
    }
}
