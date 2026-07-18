use std::string::String;
use std::vec::Vec;

use prns_core::crypto::{hmac_sha256, hmac_sha256_verify};
use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome, IDENTITY_SECRET_KEY_LEN,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    RnsRpcRequest, RpcAuthenticationControlMessage, RpcAuthenticationKey, RpcChallengeNonce,
    RpcDigest, RpcRequest, RpcVerb, AUTHENTICATION_FRAME_MAX_LENGTH, LEGACY_MD5_MESSAGE_LENGTH,
};
use prns_core::interfaces::{
    ConnectionState, InterfaceId, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
};
use prns_core::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use prns_core::routing::types::NextHop;
use prns_core::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::wire::{DestinationHash, TransportId};
use prns_runtime::node_introspection::{
    AnnounceRateSnapshot, InterfaceInventoryEntry, NodeIntrospection, RouteSnapshot,
};
use prns_runtime::runtime::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DropRouteOutcome, DropRoutesViaOutcome,
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};
use rmpv::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::authentication::{deliver_our_challenge, SharedInstanceCredentials};
use super::dispatch::reply_for_decoded;
use super::framing::{read_auth_frame, read_frame, write_frame, write_frame_header};
#[cfg(target_os = "linux")]
use super::server::{bind_abstract_rpc, RpcBind};
use super::server::{serve_connection, SharedInstanceRpcCompat};
use super::storage::{load_or_seed_rns_rpc_key, reticulum_storage_dir, RnsRpcKeyStorageError};
use super::telemetry::RpcTelemetry;

mod authentication;
mod dialects;
mod framing;
mod listeners;
mod management;
mod queries;
mod routes;
mod storage;
mod support;

use support::*;

fn encode_msgpack(value: Value) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes)
}
