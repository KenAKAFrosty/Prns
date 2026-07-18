//! The RNS shared-instance control RPC, answered for stock clients: the RNS-compatibility
//! control channel, distinct from Prns's own command API. Stock apps (Sideband, NomadNet,
//! MeshChat) lean on it in ways that fault hard if nobody answers: LXMF turns on
//! `link.track_phy_stats`, whose unguarded `Reticulum.get_packet_rssi` raises
//! `ConnectionRefused` through `Link.receive` and hangs an attachment; NomadNet's TextUI calls
//! `get_interface_stats` at startup and crashes if the reply is missing or the wrong shape.
//!
//! This began as a fault-avoidance stub and is now an honest shared instance: `link_count`,
//! `path_table`, `next_hop`, and `next_hop_if_name` are answered from live engine state read
//! through the node handle ([`NodeIntrospection`]); `interface_stats` reports the same canonical
//! inventory folded to its configured logical interfaces. The core's destination identities are
//! RNS's `known_destinations`; its `destination_data` and `identity_data` RPC verbs project onto
//! that same retention seam without carrying the compatibility name inward.
//!
//! Wire protocol is `multiprocessing.connection`: a signed 4-byte big-endian length, or `-1`
//! followed by an unsigned 8-byte length for wide frames; mutual HMAC challenge/response keyed
//! on the shared `rpc_key`; and one request and reply per connection.
//! Negotiated per peer: the HMAC digest exactly as CPython does (`{sha256}` tag vs legacy
//! unprefixed HMAC-MD5), and the payload codec (RNS 1.3.5 msgpack vs legacy pickle), detected
//! from the request's first byte and answered in kind.

use std::string::String;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::vec::Vec;

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;

use prns_core::crypto::{hmac_sha256, hmac_sha256_verify};
use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{
    IdentityHash, IdentitySigner, MarkDestinationUsedOutcome, ReleaseDestinationOutcome,
    RetainDestinationOutcome, IDENTITY_SECRET_KEY_LEN,
};
use prns_core::interfaces::{ConnectionState, InterfaceId, PacketPhyStats};
use prns_core::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use prns_core::routing::types::NextHop;
use prns_core::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
use prns_core::wire::DestinationHash;
use prns_runtime::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot, InterfaceInventoryEntry, NodeIntrospection,
    RouteSnapshot,
};
use prns_runtime::runtime::{
    DestinationIdentityRetentionControl, DestinationIdentityRetentionControlError,
    DropRouteOutcome, IdentityBlackholeControl, IdentityBlackholeControlError,
    IdentityBlackholeSource, IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};
use rmpv::Value;

mod request;

use self::request::{DestinationDataOperation, RnsInteger, RnsRpcRequest};
use super::blackhole_compat::table_value as blackhole_table_value;

/// RNS's `Interface.MODE_FULL` — the default interface mode a stock client renders as "Full". The
/// shim reports it for every interface until per-interface mode is carried through the status seam.
const RNS_INTERFACE_MODE_FULL: i64 = 0x01;

/// Shared-instance RPC counters. Clone this before handing it to
/// [`SharedInstanceRpcCompat::with_telemetry`] and expose snapshots from your host status surface.
#[derive(Clone, Default)]
pub struct RpcTelemetry {
    inner: Arc<RpcTelemetryInner>,
}

#[derive(Default)]
struct RpcTelemetryInner {
    active_clients: AtomicU64,
    total_connections: AtomicU64,
    request_frames: AtomicU64,
    completed_requests: AtomicU64,
    auth_failures: AtomicU64,
    protocol_failures: AtomicU64,
    read_failures: AtomicU64,
    write_failures: AtomicU64,
    invalid_frames: AtomicU64,
    msgpack_requests: AtomicU64,
    pickle_requests: AtomicU64,
    get_interface_stats: AtomicU64,
    get_path_table: AtomicU64,
    get_rate_table: AtomicU64,
    get_link_count: AtomicU64,
    get_next_hop: AtomicU64,
    get_next_hop_if_name: AtomicU64,
    get_first_hop_timeout: AtomicU64,
    get_phy_stats: AtomicU64,
    management_reads: AtomicU64,
    management_writes: AtomicU64,
    unknown_requests: AtomicU64,
}

/// Point-in-time shared-instance RPC counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RpcTelemetrySnapshot {
    pub active_clients: u64,
    pub total_connections: u64,
    pub request_frames: u64,
    pub completed_requests: u64,
    pub auth_failures: u64,
    pub protocol_failures: u64,
    pub read_failures: u64,
    pub write_failures: u64,
    pub invalid_frames: u64,
    pub msgpack_requests: u64,
    pub pickle_requests: u64,
    pub get_interface_stats: u64,
    pub get_path_table: u64,
    pub get_rate_table: u64,
    pub get_link_count: u64,
    pub get_next_hop: u64,
    pub get_next_hop_if_name: u64,
    pub get_first_hop_timeout: u64,
    pub get_phy_stats: u64,
    pub management_reads: u64,
    pub management_writes: u64,
    pub unknown_requests: u64,
}

impl RpcTelemetry {
    /// Snapshot the counters without resetting them.
    #[must_use]
    pub fn snapshot(&self) -> RpcTelemetrySnapshot {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        RpcTelemetrySnapshot {
            active_clients: load(&self.inner.active_clients),
            total_connections: load(&self.inner.total_connections),
            request_frames: load(&self.inner.request_frames),
            completed_requests: load(&self.inner.completed_requests),
            auth_failures: load(&self.inner.auth_failures),
            protocol_failures: load(&self.inner.protocol_failures),
            read_failures: load(&self.inner.read_failures),
            write_failures: load(&self.inner.write_failures),
            invalid_frames: load(&self.inner.invalid_frames),
            msgpack_requests: load(&self.inner.msgpack_requests),
            pickle_requests: load(&self.inner.pickle_requests),
            get_interface_stats: load(&self.inner.get_interface_stats),
            get_path_table: load(&self.inner.get_path_table),
            get_rate_table: load(&self.inner.get_rate_table),
            get_link_count: load(&self.inner.get_link_count),
            get_next_hop: load(&self.inner.get_next_hop),
            get_next_hop_if_name: load(&self.inner.get_next_hop_if_name),
            get_first_hop_timeout: load(&self.inner.get_first_hop_timeout),
            get_phy_stats: load(&self.inner.get_phy_stats),
            management_reads: load(&self.inner.management_reads),
            management_writes: load(&self.inner.management_writes),
            unknown_requests: load(&self.inner.unknown_requests),
        }
    }

    fn connection_opened(&self) -> RpcConnectionGuard {
        self.inner.total_connections.fetch_add(1, Ordering::Relaxed);
        self.inner.active_clients.fetch_add(1, Ordering::Relaxed);
        RpcConnectionGuard {
            telemetry: self.clone(),
        }
    }

    fn record_request_frame(&self) {
        self.inner.request_frames.fetch_add(1, Ordering::Relaxed);
    }

    fn record_request(&self, dialect: RpcDialect, verb: RpcVerb) {
        match dialect {
            RpcDialect::Pickle => self.inner.pickle_requests.fetch_add(1, Ordering::Relaxed),
            RpcDialect::Msgpack => self.inner.msgpack_requests.fetch_add(1, Ordering::Relaxed),
        };
        match verb {
            RpcVerb::InterfaceStats => self
                .inner
                .get_interface_stats
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::PathTable => self.inner.get_path_table.fetch_add(1, Ordering::Relaxed),
            RpcVerb::RateTable => self.inner.get_rate_table.fetch_add(1, Ordering::Relaxed),
            RpcVerb::LinkCount => self.inner.get_link_count.fetch_add(1, Ordering::Relaxed),
            RpcVerb::NextHop => self.inner.get_next_hop.fetch_add(1, Ordering::Relaxed),
            RpcVerb::NextHopIfName => self
                .inner
                .get_next_hop_if_name
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::FirstHopTimeout => self
                .inner
                .get_first_hop_timeout
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::PacketRssi | RpcVerb::PacketSnr | RpcVerb::PacketQuality => {
                self.inner.get_phy_stats.fetch_add(1, Ordering::Relaxed)
            }
            RpcVerb::BlackholedIdentities | RpcVerb::IsBlackholed => {
                self.inner.management_reads.fetch_add(1, Ordering::Relaxed)
            }
            RpcVerb::DropPath
            | RpcVerb::DropAllVia
            | RpcVerb::DropAnnounceQueues
            | RpcVerb::BlackholeIdentity
            | RpcVerb::UnblackholeIdentity
            | RpcVerb::DestinationData
            | RpcVerb::IdentityData => self.inner.management_writes.fetch_add(1, Ordering::Relaxed),
            RpcVerb::Unknown => self.inner.unknown_requests.fetch_add(1, Ordering::Relaxed),
        };
    }

    fn record_completed(&self) {
        self.inner
            .completed_requests
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_auth_failure(&self) {
        self.inner.auth_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn record_protocol_failure(&self) {
        self.inner.protocol_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn record_read_failure(&self, kind: std::io::ErrorKind) {
        self.inner.read_failures.fetch_add(1, Ordering::Relaxed);
        if kind == std::io::ErrorKind::InvalidData {
            self.inner.invalid_frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_write_failure(&self) {
        self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
    }
}

struct RpcConnectionGuard {
    telemetry: RpcTelemetry,
}

impl Drop for RpcConnectionGuard {
    fn drop(&mut self) {
        self.telemetry
            .inner
            .active_clients
            .fetch_sub(1, Ordering::Relaxed);
    }
}

const CHALLENGE: &[u8] = b"#CHALLENGE#";
const WELCOME: &[u8] = b"#WELCOME#";
const FAILURE: &[u8] = b"#FAILURE#";
const DIGEST_PREFIX: &[u8] = b"{sha256}";
const CHALLENGE_NONCE_LEN: usize = 40;
const AUTH_FRAME_MAX_LEN: usize = 256;
const DEFAULT_PER_HOP_TIMEOUT_SECS: i64 = 6;
const LEGACY_MD5_DIGEST_LEN: usize = 16;
const LEGACY_MD5_MESSAGE_LEN: usize = 20;

/// The HMAC digests `multiprocessing.connection` negotiates. A modern peer prefixes its message with
/// `{sha256}`; a legacy peer (Python ≤ 3.11) sends a bare HMAC-MD5 with no prefix at all.
#[derive(Clone, Copy)]
enum Digest {
    Md5,
    Sha256,
}

impl Digest {
    fn label(self) -> &'static [u8] {
        match self {
            Digest::Md5 => b"md5",
            Digest::Sha256 => b"sha256",
        }
    }

    #[allow(clippy::expect_used)]
    fn mac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        match self {
            Digest::Sha256 => hmac_sha256(key, message).to_vec(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.finalize().into_bytes().to_vec()
            }
        }
    }

    #[allow(clippy::expect_used)]
    fn verify(self, key: &[u8], message: &[u8], tag: &[u8]) -> bool {
        match self {
            Digest::Sha256 => hmac_sha256_verify(key, message, tag).is_ok(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.verify_slice(tag).is_ok()
            }
        }
    }
}

/// Mirror of CPython's `_get_digest_name_and_payload`: a message of a legacy length carries a bare
/// HMAC-MD5 (digest [`None`], whole message is the payload); a `{digest}`-prefixed message names its
/// own digest. An unrecognized prefix or digest is rejected (`None`), as CPython raises.
fn negotiated_digest(message: &[u8]) -> Option<(Option<Digest>, &[u8])> {
    if message.len() == LEGACY_MD5_DIGEST_LEN || message.len() == LEGACY_MD5_MESSAGE_LEN {
        return Some((None, message));
    }
    let rest = message.strip_prefix(b"{")?;
    let close = rest.iter().position(|&byte| byte == b'}')?;
    let digest = match &rest[..close] {
        b"sha256" => Digest::Sha256,
        b"md5" => Digest::Md5,
        _ => return None,
    };
    Some((Some(digest), &rest[close + 1..]))
}

/// Mirror of CPython's `_create_response`: answer a peer's challenge `message` with a MAC over the
/// whole message. A legacy (unprefixed) challenge gets a bare HMAC-MD5; a `{digest}`-prefixed
/// challenge gets the same `{digest}` prefix back. [`None`] when the challenge digest is unsupported.
fn create_response(key: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    match negotiated_digest(message)? {
        (None, _) => Some(Digest::Md5.mac(key, message)),
        (Some(digest), _) => {
            let mut reply = std::vec![b'{'];
            reply.extend_from_slice(digest.label());
            reply.push(b'}');
            reply.extend_from_slice(&digest.mac(key, message));
            Some(reply)
        }
    }
}

/// Mirror of CPython's `_verify_challenge`: the peer's `response` to *our* `challenge_message` is a
/// MAC over that message, in whatever digest the response declares (an unprefixed response means the
/// legacy HMAC-MD5). Constant-time, length-checked.
fn response_authenticates(key: &[u8], challenge_message: &[u8], response: &[u8]) -> bool {
    match negotiated_digest(response) {
        Some((digest, mac)) => digest
            .unwrap_or(Digest::Md5)
            .verify(key, challenge_message, mac),
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedInstanceCredentials {
    pub rpc_key: Vec<u8>,
    pub transport_identity_hash: IdentityHash,
}

impl SharedInstanceCredentials {
    pub fn from_identity_secret(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(secret);
        Self {
            rpc_key: prns_core::crypto::sha256(secret).to_vec(),
            transport_identity_hash: identity.identity_hash(),
        }
    }

    pub fn with_rpc_key(mut self, rpc_key: Vec<u8>) -> Self {
        self.rpc_key = rpc_key;
        self
    }
}

/// Answers the RNS shared-instance control RPC for stock clients, with the minimal replies that keep
/// attachment delivery from faulting. Stand one up beside a [`LocalServer`](crate::shared_instance::server::LocalServer)
/// and drive it with [`run`](Self::run).
pub struct SharedInstanceRpcCompat<Q, B = Q> {
    credentials: SharedInstanceCredentials,
    bind: RpcBind,
    query: Q,
    blackholes: B,
    telemetry: RpcTelemetry,
}

enum RpcBind {
    Tcp(String),
    #[cfg(target_os = "linux")]
    Abstract(String),
}

impl<Q> SharedInstanceRpcCompat<Q, Q>
where
    Q: NodeIntrospection
        + RoutingControl
        + DestinationIdentityRetentionControl
        + IdentityBlackholeSource
        + IdentityBlackholeControl
        + Clone
        + Send
        + Sync
        + 'static,
{
    /// Answer on a loopback TCP port — RNS's `instance_control_port` (default 37428's sibling 37429),
    /// or whatever a client configured. `rpc_key` MUST equal the clients' key: RNS's `full_hash` of the
    /// shared transport identity's private key, or a value both sides set as `rpc_key` in config.
    /// `query` is the node handle the shim reads engine state through to answer each verb.
    #[must_use]
    pub fn tcp(credentials: SharedInstanceCredentials, port: u16, query: Q) -> Self {
        Self {
            credentials,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
            blackholes: query.clone(),
            query,
            telemetry: RpcTelemetry::default(),
        }
    }

    /// Answer on the abstract AF_UNIX socket `\0rns/{socket_path}/rpc` that a default-config RNS client
    /// uses on Linux. Linux only.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn abstract_unix(
        credentials: SharedInstanceCredentials,
        socket_path: impl Into<String>,
        query: Q,
    ) -> Self {
        Self {
            credentials,
            bind: RpcBind::Abstract(socket_path.into()),
            blackholes: query.clone(),
            query,
            telemetry: RpcTelemetry::default(),
        }
    }
}

impl<Q, B> SharedInstanceRpcCompat<Q, B>
where
    Q: NodeIntrospection
        + RoutingControl
        + DestinationIdentityRetentionControl
        + Clone
        + Send
        + Sync
        + 'static,
    B: IdentityBlackholeSource + IdentityBlackholeControl + Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn tcp_with_blackholes(
        credentials: SharedInstanceCredentials,
        port: u16,
        query: Q,
        blackholes: B,
    ) -> Self {
        Self {
            credentials,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
            query,
            blackholes,
            telemetry: RpcTelemetry::default(),
        }
    }

    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn abstract_unix_with_blackholes(
        credentials: SharedInstanceCredentials,
        socket_path: impl Into<String>,
        query: Q,
        blackholes: B,
    ) -> Self {
        Self {
            credentials,
            bind: RpcBind::Abstract(socket_path.into()),
            query,
            blackholes,
            telemetry: RpcTelemetry::default(),
        }
    }

    /// Return the server's shared telemetry handle.
    #[must_use]
    pub fn telemetry(&self) -> RpcTelemetry {
        self.telemetry.clone()
    }

    /// Use an app-owned telemetry handle so another task can snapshot it for status output.
    #[must_use]
    pub fn with_telemetry(mut self, telemetry: RpcTelemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Accept control connections forever, serving each on its own task. Returns only if the listener
    /// cannot be bound.
    pub async fn run(self) {
        match self.bind {
            RpcBind::Tcp(addr) => {
                let Ok(listener) = TcpListener::bind(addr.as_str()).await else {
                    return;
                };
                loop {
                    if let Ok((stream, _)) = listener.accept().await {
                        let credentials = self.credentials.clone();
                        let query = self.query.clone();
                        let blackholes = self.blackholes.clone();
                        let telemetry = self.telemetry.clone();
                        tokio::spawn(async move {
                            let _ =
                                serve_connection(stream, credentials, query, blackholes, telemetry)
                                    .await;
                        });
                    }
                }
            }
            #[cfg(target_os = "linux")]
            RpcBind::Abstract(socket_path) => {
                let Some(listener) = bind_abstract_rpc(&socket_path) else {
                    return;
                };
                loop {
                    if let Ok((stream, _)) = listener.accept().await {
                        let credentials = self.credentials.clone();
                        let query = self.query.clone();
                        let blackholes = self.blackholes.clone();
                        let telemetry = self.telemetry.clone();
                        tokio::spawn(async move {
                            let _ =
                                serve_connection(stream, credentials, query, blackholes, telemetry)
                                    .await;
                        });
                    }
                }
            }
        }
    }
}

/// Bind `\0rns/{socket_path}/rpc` in the Linux abstract namespace (leading null implied), mirroring how
/// the data bus binds `\0rns/{socket_path}`.
#[cfg(target_os = "linux")]
fn bind_abstract_rpc(socket_path: &str) -> Option<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    listener.set_nonblocking(true).ok()?;
    UnixListener::from_std(listener).ok()
}

async fn serve_connection<S, Q, B>(
    mut stream: S,
    credentials: SharedInstanceCredentials,
    query: Q,
    blackholes: B,
    telemetry: RpcTelemetry,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    Q: NodeIntrospection + RoutingControl + DestinationIdentityRetentionControl,
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    let _active = telemetry.connection_opened();
    let client_authenticated = match deliver_our_challenge(&mut stream, &credentials.rpc_key).await
    {
        Ok(authenticated) => authenticated,
        Err(err) => {
            telemetry.record_read_failure(err.kind());
            return Err(err);
        }
    };
    if !client_authenticated {
        telemetry.record_auth_failure();
        return Ok(());
    }
    let server_authenticated =
        match answer_client_challenge(&mut stream, &credentials.rpc_key).await {
            Ok(authenticated) => authenticated,
            Err(err) => {
                telemetry.record_read_failure(err.kind());
                return Err(err);
            }
        };
    if !server_authenticated {
        telemetry.record_auth_failure();
        telemetry.record_protocol_failure();
        return Ok(());
    }
    let request = match read_frame(&mut stream).await {
        Ok(request) => request,
        Err(err) => {
            telemetry.record_read_failure(err.kind());
            return Err(err);
        }
    };
    telemetry.record_request_frame();
    let request = match RpcRequest::decode(&request) {
        Ok(request) => request,
        Err(_) => {
            telemetry.record_protocol_failure();
            return Ok(());
        }
    };
    let dialect = request.dialect();
    let verb = request.verb();
    telemetry.record_request(dialect, verb);
    #[cfg(feature = "tracing")]
    tracing::debug!(
        event = "shared_instance_rpc_request",
        dialect = dialect.as_str(),
        verb = verb.as_str()
    );
    let reply = reply_for_decoded(
        &request,
        &query,
        &query,
        &query,
        &blackholes,
        credentials.transport_identity_hash,
    )
    .await?;
    if let Err(err) = write_frame(&mut stream, &reply).await {
        telemetry.record_write_failure();
        return Err(err);
    }
    telemetry.record_completed();
    Ok(())
}

/// Mirror of RNS `Listener.deliver_challenge`: send our `{sha256}`-tagged challenge, accept the
/// client's MAC over it (sha256 if it tags one, else the legacy unprefixed HMAC-MD5), and reply
/// `#WELCOME#`. The MAC covers the digest prefix. Returns whether the client authenticated.
async fn deliver_our_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8],
) -> std::io::Result<bool> {
    let mut nonce = [0u8; CHALLENGE_NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| std::io::Error::other("rpc challenge entropy"))?;
    let mut our_message = DIGEST_PREFIX.to_vec();
    our_message.extend_from_slice(&nonce);
    let mut challenge = CHALLENGE.to_vec();
    challenge.extend_from_slice(&our_message);
    write_frame(stream, &challenge).await?;

    let response = read_auth_frame(stream).await?;
    if !response_authenticates(rpc_key, &our_message, &response) {
        let _ = write_frame(stream, FAILURE).await;
        return Ok(false);
    }
    write_frame(stream, WELCOME).await?;
    Ok(true)
}

/// Mirror of RNS `Listener.answer_challenge`: answer the client's challenge in its own negotiated
/// digest (a `{sha256}` reply for a modern client, a bare HMAC-MD5 for a legacy one) and await its
/// `#WELCOME#`. Returns whether it accepted us.
async fn answer_client_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8],
) -> std::io::Result<bool> {
    let client_challenge = read_auth_frame(stream).await?;
    let Some(client_message) = client_challenge.strip_prefix(CHALLENGE) else {
        return Ok(false);
    };
    let Some(reply) = create_response(rpc_key, client_message) else {
        return Ok(false);
    };
    write_frame(stream, &reply).await?;
    Ok(read_auth_frame(stream).await? == WELCOME)
}

/// The wire codec a client's RPC payload speaks. RNS through 1.3.x carried the request and reply as
/// `multiprocessing.connection`'s pickle (`connection.send`/`recv`); RNS 1.3.5 frames msgpack
/// (`send_bytes(mp.packb(..))` / `mp.unpackb(recv_bytes())`). Both share the same length-prefixed
/// framing and the same auth handshake — only the payload codec differs, so the reply must answer in
/// the dialect the request arrived in or the client mis-decodes it (a pickle `None` reads back as the
/// msgpack integer 78, which a client indexing the result then faults on).
#[derive(Clone, Copy)]
enum RpcDialect {
    Pickle,
    Msgpack,
}

#[cfg(feature = "tracing")]
impl RpcDialect {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pickle => "pickle",
            Self::Msgpack => "msgpack",
        }
    }
}

/// Tell the dialects apart by the request's first byte: every RNS RPC request is a small map, so a
/// msgpack request opens with a fixmap tag (`0x81..=0x8f`), while a pickle stream opens with the PROTO
/// opcode `0x80` (or a protocol-0 opcode) — never `0x81..=0x8f`.
fn dialect_of(request: &[u8]) -> RpcDialect {
    match request.first() {
        Some(0x81..=0x8f | 0xde | 0xdf) => RpcDialect::Msgpack,
        _ => RpcDialect::Pickle,
    }
}

#[derive(Clone, Copy)]
enum RpcVerb {
    InterfaceStats,
    PathTable,
    RateTable,
    LinkCount,
    NextHop,
    NextHopIfName,
    FirstHopTimeout,
    PacketRssi,
    PacketSnr,
    PacketQuality,
    BlackholedIdentities,
    IsBlackholed,
    DropPath,
    DropAllVia,
    DropAnnounceQueues,
    BlackholeIdentity,
    UnblackholeIdentity,
    DestinationData,
    IdentityData,
    Unknown,
}

#[cfg(feature = "tracing")]
impl RpcVerb {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InterfaceStats => "interface_stats",
            Self::PathTable => "path_table",
            Self::RateTable => "rate_table",
            Self::LinkCount => "link_count",
            Self::NextHop => "next_hop",
            Self::NextHopIfName => "next_hop_if_name",
            Self::FirstHopTimeout => "first_hop_timeout",
            Self::PacketRssi => "packet_rssi",
            Self::PacketSnr => "packet_snr",
            Self::PacketQuality => "packet_q",
            Self::BlackholedIdentities => "blackholed_identities",
            Self::IsBlackholed => "is_blackholed",
            Self::DropPath => "drop_path",
            Self::DropAllVia => "drop_all_via",
            Self::DropAnnounceQueues => "drop_announce_queues",
            Self::BlackholeIdentity => "blackhole_identity",
            Self::UnblackholeIdentity => "unblackhole_identity",
            Self::DestinationData => "destination_data",
            Self::IdentityData => "identity_data",
            Self::Unknown => "unknown",
        }
    }
}

enum RpcRequest<'a> {
    Pickle(&'a [u8]),
    Msgpack(RnsRpcRequest),
}

impl<'a> RpcRequest<'a> {
    fn decode(bytes: &'a [u8]) -> Result<Self, request::DecodeError> {
        match dialect_of(bytes) {
            RpcDialect::Pickle => Ok(Self::Pickle(bytes)),
            RpcDialect::Msgpack => request::decode(bytes).map(Self::Msgpack),
        }
    }

    fn dialect(&self) -> RpcDialect {
        match self {
            Self::Pickle(_) => RpcDialect::Pickle,
            Self::Msgpack(_) => RpcDialect::Msgpack,
        }
    }

    fn verb(&self) -> RpcVerb {
        match self {
            Self::Pickle(bytes) => classify_pickle_rpc_verb(bytes),
            Self::Msgpack(request) => match request {
                RnsRpcRequest::InterfaceStats => RpcVerb::InterfaceStats,
                RnsRpcRequest::PathTable { .. } => RpcVerb::PathTable,
                RnsRpcRequest::RateTable => RpcVerb::RateTable,
                RnsRpcRequest::LinkCount => RpcVerb::LinkCount,
                RnsRpcRequest::NextHop { .. } => RpcVerb::NextHop,
                RnsRpcRequest::NextHopInterface { .. } => RpcVerb::NextHopIfName,
                RnsRpcRequest::FirstHopTimeout { .. } => RpcVerb::FirstHopTimeout,
                RnsRpcRequest::PacketRssi { .. } => RpcVerb::PacketRssi,
                RnsRpcRequest::PacketSnr { .. } => RpcVerb::PacketSnr,
                RnsRpcRequest::PacketQuality { .. } => RpcVerb::PacketQuality,
                RnsRpcRequest::BlackholedIdentities => RpcVerb::BlackholedIdentities,
                RnsRpcRequest::IsBlackholed { .. } => RpcVerb::IsBlackholed,
                RnsRpcRequest::DropPath { .. } => RpcVerb::DropPath,
                RnsRpcRequest::DropAllVia { .. } => RpcVerb::DropAllVia,
                RnsRpcRequest::DropAnnounceQueues => RpcVerb::DropAnnounceQueues,
                RnsRpcRequest::BlackholeIdentity { .. } => RpcVerb::BlackholeIdentity,
                RnsRpcRequest::UnblackholeIdentity { .. } => RpcVerb::UnblackholeIdentity,
                RnsRpcRequest::DestinationData { .. } => RpcVerb::DestinationData,
                RnsRpcRequest::RetainIdentity { .. } => RpcVerb::IdentityData,
            },
        }
    }
}

fn classify_pickle_rpc_verb(request: &[u8]) -> RpcVerb {
    if contains(request, b"interface_stats") {
        RpcVerb::InterfaceStats
    } else if contains(request, b"rate_table") {
        RpcVerb::RateTable
    } else if contains(request, b"blackholed_identities") {
        RpcVerb::BlackholedIdentities
    } else if contains(request, b"is_blackholed") {
        RpcVerb::IsBlackholed
    } else if contains(request, b"path_table") {
        RpcVerb::PathTable
    } else if contains(request, b"next_hop_if_name") {
        RpcVerb::NextHopIfName
    } else if contains(request, b"next_hop") {
        RpcVerb::NextHop
    } else if contains(request, b"first_hop_timeout") {
        RpcVerb::FirstHopTimeout
    } else if contains(request, b"link_count") {
        RpcVerb::LinkCount
    } else if contains(request, b"packet_rssi") {
        RpcVerb::PacketRssi
    } else if contains(request, b"packet_snr") {
        RpcVerb::PacketSnr
    } else if contains(request, b"packet_q") {
        RpcVerb::PacketQuality
    } else if contains(request, b"drop") && contains(request, b"announce_queues") {
        RpcVerb::DropAnnounceQueues
    } else if contains(request, b"drop") && contains(request, b"all_via") {
        RpcVerb::DropAllVia
    } else if contains(request, b"drop") && contains(request, b"path") {
        RpcVerb::DropPath
    } else if contains(request, b"unblackhole_identity") {
        RpcVerb::UnblackholeIdentity
    } else if contains(request, b"blackhole_identity") {
        RpcVerb::BlackholeIdentity
    } else if contains(request, b"destination_data") {
        RpcVerb::DestinationData
    } else if contains(request, b"identity_data") {
        RpcVerb::IdentityData
    } else {
        RpcVerb::Unknown
    }
}

async fn reply_for_decoded<B>(
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

/// Purely used for raw compatibility with older RNS clients. Not intended to be complete, just enough to avoid core functionality issues with things like Sideband & Nomadnet on those older clients.
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

/// `get_next_hop` in the client's dialect: the next-hop hash to reach the destination — the transport
/// node a route goes via, or the destination itself when it is directly reachable — as bytes, or
/// [`None`] when no route is held (RNS `Transport.next_hop`).
fn reply_next_hop(dialect: RpcDialect, route: Option<RouteSnapshot>) -> std::io::Result<Vec<u8>> {
    match route {
        Some(entry) => reply_bytes(dialect, next_hop_bytes(&entry)),
        None => reply_none(dialect),
    }
}

/// `get_next_hop_if_name` in the client's dialect: the name of the interface the route is reached over.
/// RNS returns `str(interface)`, so an unknown route is the literal string `"None"`, never nil.
fn reply_next_hop_if_name(
    dialect: RpcDialect,
    route: Option<RouteSnapshot>,
) -> std::io::Result<Vec<u8>> {
    let name = route.map_or_else(
        || String::from("None"),
        |entry| interface_name(entry.interface),
    );
    reply_str(dialect, &name)
}

fn next_hop_bytes(entry: &RouteSnapshot) -> Vec<u8> {
    match entry.via {
        NextHop::Via(transport) => transport.as_bytes().to_vec(),
        NextHop::Direct => entry.destination.as_bytes().to_vec(),
    }
}

/// The path table in the client's dialect. Msgpack renders each known route as RNS's path-table dict
/// (`hash`, `via`, `hops`, `timestamp`, `expires`, `interface`); the legacy pickle dialect gets an
/// empty list — a legacy client lists nothing rather than faulting, and pickle rows are a follow-up.
/// `timestamp`/`expires` carry the engine's learned-at clock in milliseconds; aligning them to a
/// stock client's wall-clock seconds is a refinement, the routes themselves are real.
fn reply_path_table(
    dialect: RpcDialect,
    entries: Vec<RouteSnapshot>,
    max_hops: Option<&RnsInteger>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => {
            let rows = entries
                .into_iter()
                .filter(|entry| match max_hops {
                    None => true,
                    Some(RnsInteger::Negative(_)) => false,
                    Some(RnsInteger::Nonnegative(limit)) => u64::from(entry.hops) <= *limit,
                })
                .map(|entry| {
                    let via = next_hop_bytes(&entry);
                    let last_active_at = prns_core::engine::InstantMillis(
                        entry.learned_at.0.max(entry.last_relayed_at.0),
                    );
                    Value::Map(std::vec![
                        (
                            "hash".into(),
                            Value::Binary(entry.destination.as_bytes().to_vec())
                        ),
                        ("via".into(), Value::Binary(via)),
                        ("hops".into(), Value::from(i64::from(entry.hops))),
                        ("timestamp".into(), rns_timestamp(last_active_at)),
                        ("expires".into(), rns_timestamp(entry.expires_at)),
                        (
                            "interface".into(),
                            Value::from(interface_name(entry.interface))
                        ),
                    ])
                })
                .collect();
            encode_msgpack(Value::Array(rows))
        }
    }
}

/// A human name for an interface in the path table — RNS shows `str(interface)`. The kind names the
/// medium; a short hash prefix disambiguates instances of the same kind.
fn interface_name(id: InterfaceId) -> String {
    let mut name = match id.kind() {
        Some(kind) => std::format!("{kind:?}["),
        None => std::string::String::from("Interface["),
    };
    for byte in id.as_bytes().iter().take(4) {
        name.push_str(&std::format!("{byte:02x}"));
    }
    name.push(']');
    name
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    position_of(haystack, needle).is_some()
}

fn position_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `None` in the client's dialect: pickle protocol-0 `NONE` + `STOP`, or msgpack nil.
fn reply_none(dialect: RpcDialect) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"N.".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Nil),
    }
}

/// A bool in the client's dialect: pickle protocol-0 bool-as-int, or msgpack bool.
fn reply_bool(dialect: RpcDialect, value: bool) -> std::io::Result<Vec<u8>> {
    match (dialect, value) {
        (RpcDialect::Pickle, false) => Ok(b"I00\n.".to_vec()),
        (RpcDialect::Pickle, true) => Ok(b"I01\n.".to_vec()),
        (RpcDialect::Msgpack, value) => encode_msgpack(Value::Boolean(value)),
    }
}

/// An integer in the client's dialect: pickle protocol-0 `INT` + `STOP`, or msgpack through the typed
/// [`Value`] encoder (so any width — not just a fixint — encodes correctly).
fn reply_int(dialect: RpcDialect, value: i64) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => {
            let mut out = std::vec![b'I'];
            out.extend_from_slice(std::format!("{value}").as_bytes());
            out.push(b'\n');
            out.push(b'.');
            Ok(out)
        }
        RpcDialect::Msgpack => encode_msgpack(Value::from(value)),
    }
}

fn reply_rate_table(
    dialect: RpcDialect,
    entries: Vec<AnnounceRateSnapshot>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Array(
            entries
                .into_iter()
                .map(|entry| {
                    let blocked_until = if entry.blocked_until.0 == 0 {
                        Value::from(0)
                    } else {
                        rns_timestamp(entry.blocked_until)
                    };
                    Value::Map(std::vec![
                        (
                            "hash".into(),
                            Value::Binary(entry.destination.as_bytes().to_vec()),
                        ),
                        ("last".into(), rns_timestamp(entry.last_allowed_announce_at)),
                        (
                            "rate_violations".into(),
                            Value::from(u64::from(entry.rate_violations)),
                        ),
                        ("blocked_until".into(), blocked_until),
                        (
                            "timestamps".into(),
                            Value::Array(
                                entry.observed_at.into_iter().map(rns_timestamp).collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        )),
    }
}

fn rns_timestamp(timestamp: prns_core::engine::InstantMillis) -> Value {
    Value::F64(std::time::Duration::from_millis(timestamp.0).as_secs_f64())
}

/// An empty map in the client's dialect: the shape of RNS's blackholed identity table.
fn reply_empty_map(dialect: RpcDialect) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"}.".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Map(std::vec![])),
    }
}

/// A byte string in the client's dialect: msgpack `bin` through the typed [`Value`] encoder, or pickle
/// `SHORT_BINBYTES` (`PROTO 3`, since a `bytes` object needs protocol 3) + `STOP`. Lengths over 255 are
/// truncated by the `as u8`; an RNS hash is 16 bytes, so this is exact for every value the shim sends.
fn reply_bytes(dialect: RpcDialect, value: Vec<u8>) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Msgpack => encode_msgpack(Value::Binary(value)),
        RpcDialect::Pickle => {
            let mut out = std::vec![0x80, 0x03, b'C', value.len() as u8];
            out.extend_from_slice(&value);
            out.push(b'.');
            Ok(out)
        }
    }
}

/// A string in the client's dialect: msgpack `str` through the typed [`Value`] encoder, or pickle
/// protocol-0 `UNICODE` (a newline-terminated string) + `STOP`. Interface names are ASCII (a medium
/// name plus a hex hash prefix), so the raw protocol-0 form needs no escaping.
fn reply_str(dialect: RpcDialect, value: &str) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Msgpack => encode_msgpack(Value::from(value)),
        RpcDialect::Pickle => {
            let mut out = std::vec![b'V'];
            out.extend_from_slice(value.as_bytes());
            out.push(b'\n');
            out.push(b'.');
            Ok(out)
        }
    }
}

/// The node's interface stats in the client's dialect: the shape `rnstatus` and a NomadNet
/// TextUI index by `["interfaces"]`. Msgpack renders each interface's live counters into RNS's
/// per-interface dict; untracked fields are simply absent, which a stock client reads as "not
/// reported" rather than a fabricated zero. The legacy pickle dialect gets the well-formed
/// empty map (`{"interfaces": []}`, `protocol=2`); real pickle rows are a follow-up.
fn reply_interface_stats(
    dialect: RpcDialect,
    inventory: Vec<InterfaceInventoryEntry>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(std::vec![
            0x80, 0x02, 0x7d, 0x71, 0x00, 0x58, 0x0a, 0x00, 0x00, 0x00, b'i', b'n', b't', b'e',
            b'r', b'f', b'a', b'c', b'e', b's', 0x71, 0x01, 0x5d, 0x71, 0x02, 0x73, 0x2e,
        ]),
        RpcDialect::Msgpack => interface_stats_msgpack(&logical_interface_inventory(inventory)),
    }
}

fn interface_stats_msgpack(inventory: &[InterfaceInventoryEntry]) -> std::io::Result<Vec<u8>> {
    let mut total_rxb = 0u64;
    let mut total_txb = 0u64;
    let mut total_rxs = 0u64;
    let mut total_txs = 0u64;
    let rows = inventory
        .iter()
        .map(|entry| {
            let interface = &entry.snapshot;
            let ifac = entry.ifac.as_ref();
            let name = entry
                .name
                .clone()
                .unwrap_or_else(|| interface_name(interface.id));
            let rates = interface
                .transfer_rates
                .unwrap_or(prns_core::interfaces::TransferRates {
                    rx_bps: 0,
                    tx_bps: 0,
                });
            total_rxb += interface.rx_bytes;
            total_txb += interface.tx_bytes;
            total_rxs += u64::from(rates.rx_bps);
            total_txs += u64::from(rates.tx_bps);
            Value::Map(std::vec![
                ("name".into(), Value::from(name.clone())),
                ("short_name".into(), Value::from(name)),
                ("type".into(), Value::from(interface_type(interface.id))),
                (
                    "status".into(),
                    Value::Boolean(is_online(interface.connection))
                ),
                ("mode".into(), Value::from(RNS_INTERFACE_MODE_FULL)),
                ("clients".into(), Value::Nil),
                ("rxb".into(), Value::from(interface.rx_bytes as i64)),
                ("txb".into(), Value::from(interface.tx_bytes as i64)),
                ("rxs".into(), Value::from(i64::from(rates.rx_bps))),
                ("txs".into(), Value::from(i64::from(rates.tx_bps))),
                (
                    "ifac_signature".into(),
                    ifac.map_or(Value::Nil, |ifac| Value::Binary(ifac.signature.to_vec())),
                ),
                (
                    "ifac_size".into(),
                    ifac.map_or(Value::Nil, |ifac| Value::from(ifac.size.bytes() as i64)),
                ),
                (
                    "ifac_netname".into(),
                    ifac.and_then(|ifac| ifac.network_name.as_ref())
                        .map_or(Value::Nil, |name| Value::from(name.clone())),
                ),
            ])
        })
        .collect();
    encode_msgpack(Value::Map(std::vec![
        ("interfaces".into(), Value::Array(rows)),
        ("rxb".into(), Value::from(total_rxb as i64)),
        ("txb".into(), Value::from(total_txb as i64)),
        ("rxs".into(), Value::from(total_rxs as i64)),
        ("txs".into(), Value::from(total_txs as i64)),
        ("rss".into(), Value::Nil),
    ]))
}

fn encode_msgpack(value: Value) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes)
}

/// RNS's per-interface `status` flag is a plain bool: up or down. A `Connected` or `Degraded`
/// interface is up (degraded carries traffic, just impaired); everything else — initializing,
/// reconnecting, failed, disconnected, disabled — is down.
fn is_online(connection: ConnectionState) -> bool {
    matches!(
        connection,
        ConnectionState::Connected | ConnectionState::Degraded
    )
}

/// RNS's per-interface `type` is the interface class name. The kind names the medium; an interface
/// with no kind byte is a bare `Interface`.
fn interface_type(id: InterfaceId) -> String {
    match id.kind() {
        Some(kind) => std::format!("{kind:?}"),
        None => std::string::String::from("Interface"),
    }
}

async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, payload: &[u8]) -> std::io::Result<()> {
    write_frame_header(stream, payload.len()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn write_frame_header<S: AsyncWrite + Unpin>(
    stream: &mut S,
    len: usize,
) -> std::io::Result<()> {
    match i32::try_from(len) {
        Ok(short) => stream.write_all(&short.to_be_bytes()).await,
        Err(_) => {
            let wide = u64::try_from(len)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            stream.write_all(&(-1i32).to_be_bytes()).await?;
            stream.write_all(&wide.to_be_bytes()).await
        }
    }
}

async fn read_frame_length<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<usize> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let signed = i32::from_be_bytes(header);
    let len = if signed == -1 {
        let mut wide = [0u8; 8];
        stream.read_exact(&mut wide).await?;
        usize::try_from(u64::from_be_bytes(wide))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    } else {
        usize::try_from(signed)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    };
    Ok(len)
}

async fn read_frame_body<S: AsyncRead + Unpin>(
    stream: &mut S,
    len: usize,
) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    body.try_reserve_exact(len)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
    body.resize(len, 0);
    stream.read_exact(&mut body).await?;
    Ok(body)
}

async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let len = read_frame_length(stream).await?;
    read_frame_body(stream, len).await
}

async fn read_auth_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let len = read_frame_length(stream).await?;
    if len > AUTH_FRAME_MAX_LEN {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    read_frame_body(stream, len).await
}

/// Adopt the host's RNS transport identity as the shared-instance `rpc_key`, so a
/// default-config client derives the same key with no manual step: RNS persists the raw
/// private key at `{storage_dir}/transport_identity` and its `rpc_key` is
/// `full_hash(get_private_key())`. A present identity is honored untouched; an absent one is
/// seeded from `seed_if_absent` (owning the identity as a shared instance does).
#[must_use]
pub fn rpc_key_from_rns_identity(storage_dir: &std::path::Path, seed_if_absent: &[u8]) -> [u8; 32] {
    let path = storage_dir.join("transport_identity");
    let private = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = std::fs::create_dir_all(storage_dir);
            let _ = std::fs::write(&path, seed_if_absent);
            seed_if_absent.to_vec()
        }
    };
    prns_core::crypto::sha256(&private)
}

/// RNS's storage directory: `$RETICULUM_CONFIG_DIR/storage`, else `~/.reticulum/storage` — the layout
/// a stock client uses by default.
#[must_use]
pub fn reticulum_storage_dir() -> std::path::PathBuf {
    std::env::var_os("RETICULUM_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".reticulum")
        })
        .join("storage")
}

#[cfg(test)]
mod tests;
