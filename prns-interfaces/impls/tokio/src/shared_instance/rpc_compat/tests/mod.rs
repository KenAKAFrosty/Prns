use std::string::String;
use std::vec::Vec;

use prns_core::crypto::{hmac_sha256, hmac_sha256_verify};
use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{
    IdentityHash, IdentitySigner, MarkDestinationUsedOutcome, ReleaseDestinationOutcome,
    RetainDestinationOutcome, RetainIdentityOutcome, IDENTITY_SECRET_KEY_LEN,
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

use super::authentication::{
    create_response, deliver_our_challenge, response_authenticates, Digest,
    SharedInstanceCredentials, CHALLENGE, CHALLENGE_NONCE_LEN, DIGEST_PREFIX, FAILURE,
    LEGACY_MD5_DIGEST_LEN, LEGACY_MD5_MESSAGE_LEN, WELCOME,
};
use super::dispatch::reply_for_decoded;
use super::framing::{
    read_auth_frame, read_frame, write_frame, write_frame_header, AUTH_FRAME_MAX_LEN,
};
use super::protocol::{RpcRequest, RpcVerb};
use super::reply::encode_msgpack;
use super::request::{self, RnsRpcRequest};
#[cfg(target_os = "linux")]
use super::server::{bind_abstract_rpc, RpcBind};
use super::server::{serve_connection, SharedInstanceRpcCompat};
use super::storage::{reticulum_storage_dir, rpc_key_from_rns_identity};
use super::telemetry::RpcTelemetry;

mod authentication;
mod framing;
mod listeners;
mod management;
mod protocol;
mod queries;
mod routes;
mod storage;
mod support;

use support::*;
