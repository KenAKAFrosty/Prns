use std::string::String;
use std::vec::Vec;

use prns_core::crypto::{hmac_sha256, hmac_sha256_verify};
use prns_core::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome, IDENTITY_SECRET_KEY_LEN,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    PacketHashArgument, RnsRpcRequest, RpcAuthenticationControlMessage, RpcAuthenticationKey,
    RpcChallengeNonce, RpcDigest, RpcRequest, RpcVerb, AUTHENTICATION_FRAME_MAX_LENGTH,
    LEGACY_MD5_MESSAGE_LENGTH, RPC_FRAME_MAX_LENGTH,
};
use prns_core::interfaces::PacketPhyStats;
use prns_core::routing::dedup::PacketHash;
use prns_core::routing::{
    BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
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
use super::framing::{
    ensure_frame_length, read_auth_frame, read_frame, write_frame, write_frame_header,
};
use super::persistence::{load_or_seed_rns_rpc_key, reticulum_storage_dir, RnsRpcKeyStorageError};
#[cfg(any(target_os = "linux", target_os = "android"))]
use super::server::{bind_abstract_rpc, RpcBind};
use super::server::{
    serve_connection, RpcService, SharedInstanceRpcBindError, SharedInstanceRpcServer,
    RPC_CONNECTION_IO_TIMEOUT,
};
use super::telemetry::RpcTelemetry;

mod authentication;
mod dialects;
mod framing;
mod listeners;
mod persistence;
mod support;

use support::*;

fn encode_msgpack(value: Value) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes)
}

fn test_rpc_service(
    rpc_key: [u8; 32],
    query: StubQuery,
    telemetry: RpcTelemetry,
) -> RpcService<StubQuery, StubQuery> {
    RpcService {
        credentials: test_credentials(rpc_key),
        blackhole_source: TEST_TRANSPORT_IDENTITY_HASH,
        query: query.clone(),
        blackholes: query,
        telemetry,
        started_at: std::time::Instant::now(),
        transport_identity: TEST_TRANSPORT_IDENTITY_HASH,
        network_identity: None,
        probe_responder: None,
    }
}
