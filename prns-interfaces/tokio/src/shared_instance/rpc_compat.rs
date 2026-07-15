//! The RNS shared-instance control RPC, answered for stock clients: the RNS-compatibility
//! control channel, distinct from Prns's own command API. Stock apps (Sideband, NomadNet,
//! MeshChat) lean on it in ways that fault hard if nobody answers: LXMF turns on
//! `link.track_phy_stats`, whose unguarded `Reticulum.get_packet_rssi` raises
//! `ConnectionRefused` through `Link.receive` and hangs an attachment; NomadNet's TextUI calls
//! `get_interface_stats` at startup and crashes if the reply is missing or the wrong shape.
//!
//! This began as a fault-avoidance stub and is now an honest shared instance: `link_count`,
//! `path_table`, `next_hop`, and `next_hop_if_name` are answered from live engine state read
//! through the node handle ([`InspectionSource`]); `interface_stats` reports the same canonical
//! interface inventory. The wider management surface
//! gets the right shape with conservative semantics (empty tables, no-op
//! drops, `false` writes) until backed by real engine state.
//!
//! Wire protocol is `multiprocessing.connection`: a 4-byte big-endian length frame, a mutual
//! HMAC challenge/response keyed on the shared `rpc_key`, one request and reply per connection.
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
use prns_core::inspection::{InspectionSource, InterfaceInventoryEntry, RouteSnapshot};
use prns_core::interfaces::{ConnectionState, InterfaceId};
use prns_core::routing::types::NextHop;
use prns_core::wire::DestinationHash;
use rmpv::Value;

mod request;

use self::request::RnsRpcRequest;

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
            RpcVerb::PhyStats => self.inner.get_phy_stats.fetch_add(1, Ordering::Relaxed),
            RpcVerb::ManagementRead => self.inner.management_reads.fetch_add(1, Ordering::Relaxed),
            RpcVerb::ManagementWrite => {
                self.inner.management_writes.fetch_add(1, Ordering::Relaxed)
            }
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
const MAX_FRAME_LEN: usize = 4096;
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
fn create_response(key: &[u8; 32], message: &[u8]) -> Option<Vec<u8>> {
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
fn response_authenticates(key: &[u8; 32], challenge_message: &[u8], response: &[u8]) -> bool {
    match negotiated_digest(response) {
        Some((digest, mac)) => digest
            .unwrap_or(Digest::Md5)
            .verify(key, challenge_message, mac),
        None => false,
    }
}

/// Answers the RNS shared-instance control RPC for stock clients, with the minimal replies that keep
/// attachment delivery from faulting. Stand one up beside a [`LocalServer`](crate::shared_instance::server::LocalServer)
/// and drive it with [`run`](Self::run).
pub struct SharedInstanceRpcCompat<Q> {
    rpc_key: [u8; 32],
    bind: RpcBind,
    query: Q,
    telemetry: RpcTelemetry,
}

enum RpcBind {
    Tcp(String),
    #[cfg(target_os = "linux")]
    Abstract(String),
}

impl<Q: InspectionSource + Clone + Send + Sync + 'static> SharedInstanceRpcCompat<Q> {
    /// Answer on a loopback TCP port — RNS's `instance_control_port` (default 37428's sibling 37429),
    /// or whatever a client configured. `rpc_key` MUST equal the clients' key: RNS's `full_hash` of the
    /// shared transport identity's private key, or a value both sides set as `rpc_key` in config.
    /// `query` is the node handle the shim reads engine state through to answer each verb.
    #[must_use]
    pub fn tcp(rpc_key: [u8; 32], port: u16, query: Q) -> Self {
        Self {
            rpc_key,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
            query,
            telemetry: RpcTelemetry::default(),
        }
    }

    /// Answer on the abstract AF_UNIX socket `\0rns/{socket_path}/rpc` that a default-config RNS client
    /// uses on Linux. Linux only.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn abstract_unix(rpc_key: [u8; 32], socket_path: impl Into<String>, query: Q) -> Self {
        Self {
            rpc_key,
            bind: RpcBind::Abstract(socket_path.into()),
            query,
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
                        let key = self.rpc_key;
                        let query = self.query.clone();
                        let telemetry = self.telemetry.clone();
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, key, query, telemetry).await;
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
                        let key = self.rpc_key;
                        let query = self.query.clone();
                        let telemetry = self.telemetry.clone();
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, key, query, telemetry).await;
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

async fn serve_connection<S, Q>(
    mut stream: S,
    rpc_key: [u8; 32],
    query: Q,
    telemetry: RpcTelemetry,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    Q: InspectionSource,
{
    let _active = telemetry.connection_opened();
    let client_authenticated = match deliver_our_challenge(&mut stream, &rpc_key).await {
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
    let server_authenticated = match answer_client_challenge(&mut stream, &rpc_key).await {
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
    telemetry.record_request(request.dialect(), request.verb());
    let reply = reply_for_decoded(&request, &query).await?;
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
    rpc_key: &[u8; 32],
) -> std::io::Result<bool> {
    let mut nonce = [0u8; CHALLENGE_NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| std::io::Error::other("rpc challenge entropy"))?;
    let mut our_message = DIGEST_PREFIX.to_vec();
    our_message.extend_from_slice(&nonce);
    let mut challenge = CHALLENGE.to_vec();
    challenge.extend_from_slice(&our_message);
    write_frame(stream, &challenge).await?;

    let response = read_frame(stream).await?;
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
    rpc_key: &[u8; 32],
) -> std::io::Result<bool> {
    let client_challenge = read_frame(stream).await?;
    let Some(client_message) = client_challenge.strip_prefix(CHALLENGE) else {
        return Ok(false);
    };
    let Some(reply) = create_response(rpc_key, client_message) else {
        return Ok(false);
    };
    write_frame(stream, &reply).await?;
    Ok(read_frame(stream).await? == WELCOME)
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
    PhyStats,
    ManagementRead,
    ManagementWrite,
    Unknown,
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
                RnsRpcRequest::PacketRssi { .. }
                | RnsRpcRequest::PacketSnr { .. }
                | RnsRpcRequest::PacketQuality { .. } => RpcVerb::PhyStats,
                RnsRpcRequest::BlackholedIdentities | RnsRpcRequest::IsBlackholed { .. } => {
                    RpcVerb::ManagementRead
                }
                RnsRpcRequest::DropPath { .. }
                | RnsRpcRequest::DropAllVia { .. }
                | RnsRpcRequest::DropAnnounceQueues
                | RnsRpcRequest::BlackholeIdentity { .. }
                | RnsRpcRequest::UnblackholeIdentity { .. }
                | RnsRpcRequest::DestinationData { .. }
                | RnsRpcRequest::RetainIdentity { .. } => RpcVerb::ManagementWrite,
            },
        }
    }
}

fn classify_pickle_rpc_verb(request: &[u8]) -> RpcVerb {
    if contains(request, b"interface_stats") {
        RpcVerb::InterfaceStats
    } else if contains(request, b"rate_table") {
        RpcVerb::RateTable
    } else if contains(request, b"blackholed_identities") || contains(request, b"is_blackholed") {
        RpcVerb::ManagementRead
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
    } else if contains(request, b"packet_rssi")
        || contains(request, b"packet_snr")
        || contains(request, b"packet_q")
    {
        RpcVerb::PhyStats
    } else if contains(request, b"drop")
        || contains(request, b"blackhole_identity")
        || contains(request, b"unblackhole_identity")
        || contains(request, b"destination_data")
        || contains(request, b"identity_data")
    {
        RpcVerb::ManagementWrite
    } else {
        RpcVerb::Unknown
    }
}

#[cfg(test)]
async fn reply_for(request: &[u8], query: &impl InspectionSource) -> Vec<u8> {
    let Ok(request) = RpcRequest::decode(request) else {
        return Vec::new();
    };
    reply_for_decoded(&request, query).await.unwrap_or_default()
}

async fn reply_for_decoded(
    request: &RpcRequest<'_>,
    query: &impl InspectionSource,
) -> std::io::Result<Vec<u8>> {
    match request {
        RpcRequest::Pickle(request) => reply_for_pickle(request, query).await,
        RpcRequest::Msgpack(request) => reply_for_msgpack(request, query).await,
    }
}

async fn reply_for_msgpack(
    request: &RnsRpcRequest,
    query: &impl InspectionSource,
) -> std::io::Result<Vec<u8>> {
    let dialect = RpcDialect::Msgpack;
    match request {
        RnsRpcRequest::InterfaceStats => {
            reply_interface_stats(dialect, query.interface_inventory())
        }
        RnsRpcRequest::PathTable { .. } => reply_path_table(dialect, query.routes().await),
        RnsRpcRequest::RateTable => reply_empty_array(dialect),
        RnsRpcRequest::NextHopInterface { destination_hash } => {
            reply_next_hop_if_name(dialect, query.route(*destination_hash).await)
        }
        RnsRpcRequest::NextHop { destination_hash } => {
            reply_next_hop(dialect, query.route(*destination_hash).await)
        }
        RnsRpcRequest::FirstHopTimeout { .. } => reply_int(dialect, DEFAULT_PER_HOP_TIMEOUT_SECS),
        RnsRpcRequest::LinkCount => reply_int(dialect, i64::from(query.link_count().await)),
        RnsRpcRequest::PacketRssi { .. }
        | RnsRpcRequest::PacketSnr { .. }
        | RnsRpcRequest::PacketQuality { .. } => reply_none(dialect),
        RnsRpcRequest::BlackholedIdentities => reply_empty_map(dialect),
        RnsRpcRequest::IsBlackholed { .. }
        | RnsRpcRequest::DropPath { .. }
        | RnsRpcRequest::BlackholeIdentity { .. }
        | RnsRpcRequest::UnblackholeIdentity { .. }
        | RnsRpcRequest::DestinationData { .. }
        | RnsRpcRequest::RetainIdentity { .. } => reply_bool(dialect, false),
        RnsRpcRequest::DropAllVia { .. } => reply_int(dialect, 0),
        RnsRpcRequest::DropAnnounceQueues => reply_none(dialect),
    }
}

async fn reply_for_pickle(
    request: &[u8],
    query: &impl InspectionSource,
) -> std::io::Result<Vec<u8>> {
    let dialect = RpcDialect::Pickle;
    if contains(request, b"interface_stats") {
        reply_interface_stats(dialect, query.interface_inventory())
    } else if contains(request, b"rate_table") {
        reply_empty_array(dialect)
    } else if contains(request, b"blackholed_identities") {
        reply_empty_map(dialect)
    } else if contains(request, b"is_blackholed") {
        reply_bool(dialect, false)
    } else if contains(request, b"path_table") {
        reply_path_table(dialect, query.routes().await)
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

async fn legacy_route_arg(request: &[u8], query: &impl InspectionSource) -> Option<RouteSnapshot> {
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
fn reply_path_table(dialect: RpcDialect, entries: Vec<RouteSnapshot>) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => {
            let rows = entries
                .into_iter()
                .map(|entry| {
                    let via = next_hop_bytes(&entry);
                    let learned_ms = entry.learned_at.0 as i64;
                    Value::Map(std::vec![
                        (
                            "hash".into(),
                            Value::Binary(entry.destination.as_bytes().to_vec())
                        ),
                        ("via".into(), Value::Binary(via)),
                        ("hops".into(), Value::from(i64::from(entry.hops))),
                        ("timestamp".into(), Value::from(learned_ms)),
                        ("expires".into(), Value::from(learned_ms)),
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

/// An empty list in the client's dialect: common for path/rate tables with no entries.
fn reply_empty_array(dialect: RpcDialect) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Array(std::vec![])),
    }
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
        RpcDialect::Msgpack => interface_stats_msgpack(&inventory),
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
                ("name".into(), Value::from(interface_name(interface.id))),
                (
                    "short_name".into(),
                    Value::from(interface_name(interface.id))
                ),
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
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
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
    if len > MAX_FRAME_LEN {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    let mut body = std::vec![0u8; len];
    stream.read_exact(&mut body).await?;
    Ok(body)
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
mod tests {
    use tokio::io::{AsyncRead, AsyncWriteExt};

    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvVarRestore {
        key: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvVarRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                value: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            match &self.value {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    async fn read_test_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
        tokio::time::timeout(std::time::Duration::from_secs(1), read_frame(stream))
            .await
            .expect("test RPC frame arrives before the timeout")
            .unwrap()
    }

    async fn read_frame_dup(c: &mut tokio::io::DuplexStream) -> Vec<u8> {
        read_test_frame(c).await
    }

    async fn write_frame_dup(c: &mut tokio::io::DuplexStream, payload: &[u8]) {
        c.write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        c.write_all(payload).await.unwrap();
        c.flush().await.unwrap();
    }

    async fn authenticate_modern_client(client: &mut tokio::io::DuplexStream, rpc_key: &[u8; 32]) {
        let server_challenge = read_frame_dup(client).await;
        let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
        let mut response = DIGEST_PREFIX.to_vec();
        response.extend_from_slice(&hmac_sha256(rpc_key, server_message));
        write_frame_dup(client, &response).await;
        assert_eq!(read_frame_dup(client).await, WELCOME);

        let mut our_message = DIGEST_PREFIX.to_vec();
        our_message.extend_from_slice(&[0x11u8; CHALLENGE_NONCE_LEN]);
        let mut our_challenge = CHALLENGE.to_vec();
        our_challenge.extend_from_slice(&our_message);
        write_frame_dup(client, &our_challenge).await;
        let server_reply = read_frame_dup(client).await;
        let server_mac = server_reply.strip_prefix(DIGEST_PREFIX).unwrap();
        assert!(hmac_sha256_verify(rpc_key, &our_message, server_mac).is_ok());
        write_frame_dup(client, WELCOME).await;
    }

    fn msgpack_request(entries: Vec<(&str, Value)>) -> Vec<u8> {
        let value = Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (Value::from(key), value))
                .collect(),
        );
        encode_msgpack(value).unwrap()
    }

    #[test]
    fn telemetry_classification_uses_the_decoded_operation() {
        let bytes = msgpack_request(std::vec![
            ("blackhole_identity", Value::Binary(std::vec![5; 16])),
            ("until", Value::Nil),
            ("reason", Value::from("interface_stats next_hop")),
        ]);
        let request = RpcRequest::decode(&bytes).unwrap();
        assert!(matches!(request.verb(), RpcVerb::ManagementWrite));
    }

    #[test]
    fn msgpack_reply_codec_carries_binary_map_keys_and_floats() {
        let value = Value::Map(std::vec![(
            Value::Binary(std::vec![0x5a; 16]),
            Value::Map(std::vec![(Value::from("until"), Value::F64(123.5))]),
        )]);
        let bytes = encode_msgpack(value.clone()).unwrap();
        let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoded, value);
    }

    #[derive(Clone)]
    struct StubQuery {
        links: u32,
        routes: Vec<RouteSnapshot>,
        interfaces: Vec<InterfaceInventoryEntry>,
    }

    impl InspectionSource for StubQuery {
        fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry> {
            self.interfaces.clone()
        }

        async fn link_count(&self) -> u32 {
            self.links
        }

        async fn routes(&self) -> Vec<RouteSnapshot> {
            self.routes.clone()
        }

        async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
            self.routes
                .iter()
                .find(|entry| entry.destination == destination)
                .cloned()
        }
    }

    #[tokio::test]
    async fn the_set_answers_phy_stats_none_timeout_default_and_a_real_link_count() {
        let query = StubQuery {
            links: 2,
            routes: std::vec![],
            interfaces: std::vec![],
        };
        let rssi = b"\x80\x04\x95...{'get': 'packet_rssi'}";
        assert_eq!(reply_for(rssi, &query).await, b"N.");
        let timeout = b"{'get': 'first_hop_timeout'}";
        assert_eq!(reply_for(timeout, &query).await, b"I6\n.");
        let links = b"{'get': 'link_count'}";
        assert_eq!(reply_for(links, &query).await, b"I2\n.");
        let path_table = b"{'get': 'path_table'}";
        assert_eq!(reply_for(path_table, &query).await, b"].");
        let rate_table = b"{'get': 'rate_table'}";
        assert_eq!(reply_for(rate_table, &query).await, b"].");
        let blackholes = b"{'get': 'blackholed_identities'}";
        assert_eq!(reply_for(blackholes, &query).await, b"}.");
    }

    #[tokio::test]
    async fn a_msgpack_client_gets_msgpack_replies_in_its_own_dialect() {
        let query = StubQuery {
            links: 2,
            routes: std::vec![],
            interfaces: std::vec![],
        };
        let interface_stats = b"\x81\xa3get\xafinterface_stats";
        assert_eq!(
            reply_for(interface_stats, &query).await,
            b"\x86\xaainterfaces\x90\xa3rxb\x00\xa3txb\x00\xa3rxs\x00\xa3txs\x00\xa3rss\xc0",
            "no status handles -> an empty interface list with zeroed totals"
        );
        let timeout = msgpack_request(std::vec![
            ("get", Value::from("first_hop_timeout")),
            ("destination_hash", Value::Binary(std::vec![0; 16])),
        ]);
        assert_eq!(reply_for(&timeout, &query).await, b"\x06");
        let links = b"\x81\xa3get\xaalink_count";
        assert_eq!(reply_for(links, &query).await, b"\x02");
        let rssi = msgpack_request(std::vec![
            ("get", Value::from("packet_rssi")),
            ("packet_hash", Value::Binary(std::vec![0; 32])),
        ]);
        assert_eq!(reply_for(&rssi, &query).await, b"\xc0");
        let path_table = msgpack_request(std::vec![
            ("get", Value::from("path_table")),
            ("max_hops", Value::Nil),
        ]);
        assert_eq!(reply_for(&path_table, &query).await, b"\x90");
        let rate_table = b"\x81\xa3get\xaarate_table";
        assert_eq!(reply_for(rate_table, &query).await, b"\x90");
        let blackholes = msgpack_request(std::vec![("get", Value::from("blackholed_identities"),)]);
        assert_eq!(reply_for(&blackholes, &query).await, b"\x80");
    }

    #[tokio::test]
    async fn rns_138_management_verbs_get_typed_conservative_replies() {
        let query = StubQuery {
            links: 0,
            routes: std::vec![],
            interfaces: std::vec![],
        };

        let is_blackholed = msgpack_request(std::vec![
            ("get", Value::from("is_blackholed")),
            ("identity_hash", Value::Binary(std::vec![0; 16])),
        ]);
        assert_eq!(
            reply_for(&is_blackholed, &query).await,
            b"\xc2",
            "an unknown identity is not blackholed"
        );
        let drop_path = msgpack_request(std::vec![
            ("drop", Value::from("path")),
            ("destination_hash", Value::Binary(std::vec![0; 16])),
        ]);
        assert_eq!(
            reply_for(&drop_path, &query).await,
            b"\xc2",
            "unknown path drops report false"
        );
        let drop_all_via = msgpack_request(std::vec![
            ("drop", Value::from("all_via")),
            ("destination_hash", Value::Binary(std::vec![0; 16])),
        ]);
        assert_eq!(
            reply_for(&drop_all_via, &query).await,
            b"\x00",
            "no routes were dropped via an unknown transport"
        );
        assert_eq!(
            reply_for(b"\x81\xa4drop\xafannounce_queues", &query).await,
            b"\xc0",
            "RNS drop_announce_queues returns None"
        );
        let blackhole = msgpack_request(std::vec![
            ("blackhole_identity", Value::Binary(std::vec![0; 16])),
            ("until", Value::Nil),
            ("reason", Value::Nil),
        ]);
        assert_eq!(
            reply_for(&blackhole, &query).await,
            b"\xc2",
            "blackhole writes are no-ops until backed by state"
        );
        let destination_used = msgpack_request(std::vec![
            ("destination_data", Value::from("used")),
            ("destination_hash", Value::Binary(std::vec![0; 16])),
        ]);
        assert_eq!(
            reply_for(&destination_used, &query).await,
            b"\xc2",
            "destination retain/use hooks do not pretend to persist"
        );
        assert_eq!(
            reply_for(b"{'drop': 'path'}", &query).await,
            b"I00\n.",
            "legacy clients get the same false value in pickle"
        );
    }

    #[tokio::test]
    async fn a_msgpack_path_table_renders_each_route_as_a_dict() {
        let query = StubQuery {
            links: 0,
            routes: std::vec![RouteSnapshot {
                destination: prns_core::wire::DestinationHash::new([0xab; 16]),
                hops: 3,
                via: NextHop::Direct,
                learned_at: prns_core::engine::InstantMillis(0),
                interface: InterfaceId::new([0x07; 8]),
            }],
            interfaces: std::vec![],
        };
        let request = msgpack_request(std::vec![
            ("get", Value::from("path_table")),
            ("max_hops", Value::Nil),
        ]);
        let reply = reply_for(&request, &query).await;
        assert_eq!(reply[0], 0x91, "a one-row path table is a 1-element array");
        assert_eq!(reply[1], 0x86, "each row is a 6-entry map");
        let contains = |needle: &[u8]| reply.windows(needle.len()).any(|w| w == needle);
        assert!(contains(b"hash") && contains(b"via") && contains(b"hops"));
        assert!(contains(b"interface") && contains(&[0xab; 16]));
    }

    #[tokio::test]
    async fn interface_stats_renders_each_held_interface_with_its_live_counters() {
        let query = StubQuery {
            links: 0,
            routes: std::vec![],
            interfaces: std::vec![
                InterfaceInventoryEntry {
                    snapshot: prns_core::interfaces::InterfaceSnapshot {
                        id: InterfaceId::new([0x07; 8]),
                        connection: ConnectionState::Connected,
                        failure_reason: None,
                        rx_bytes: 1234,
                        tx_bytes: 56,
                        transfer_rates: Some(prns_core::interfaces::TransferRates {
                            rx_bps: 800,
                            tx_bps: 100,
                        }),
                        destinations: 0,
                        links: 0,
                        transported_links: 0,
                        membership: prns_core::interfaces::Membership::Independent,
                    },
                    ifac: Some(prns_core::inspection::InterfaceIfacSnapshot {
                        signature: [0x5a; 64],
                        size: prns_core::interfaces::ifac::IfacSize::WIDE,
                        network_name: Some("private-net".into()),
                    }),
                },
                InterfaceInventoryEntry {
                    snapshot: prns_core::interfaces::InterfaceSnapshot {
                        id: InterfaceId::new([0x09; 8]),
                        connection: ConnectionState::Reconnecting,
                        failure_reason: None,
                        rx_bytes: 10,
                        tx_bytes: 2,
                        transfer_rates: Some(prns_core::interfaces::TransferRates {
                            rx_bps: 5,
                            tx_bps: 7,
                        }),
                        destinations: 0,
                        links: 0,
                        transported_links: 0,
                        membership: prns_core::interfaces::Membership::Independent,
                    },
                    ifac: None,
                },
            ],
        };
        let reply = reply_for(b"\x81\xa3get\xafinterface_stats", &query).await;
        assert_eq!(reply[0], 0x86, "the top dict still has its 6 keys");
        let contains = |needle: &[u8]| reply.windows(needle.len()).any(|w| w == needle);
        assert!(
            contains(b"interfaces\x92"),
            "the interfaces value is a 2-element array"
        );
        assert!(contains(b"rxb") && contains(b"status") && contains(b"mode"));
        assert!(contains(b"ifac_signature") && contains(b"ifac_netname"));
        assert!(contains(b"private-net") && contains(&[0xc4, 64, 0x5a, 0x5a]));
        assert!(contains(b"\xa4type\xa8AutoWifi"));
        assert!(contains(b"\xa4type\xabLocalServer"));
        assert!(
            contains(&[0xc3]) && contains(&[0xc2]),
            "the connected interface is up (true), the reconnecting one is down (false)"
        );
        assert!(
            contains(&[
                0xa3, b'r', b'x', b'b', 0xcd, 0x04, 0xdc, 0xa3, b't', b'x', b'b', 0x3a, 0xa3, b'r',
                b'x', b's', 0xcd, 0x03, 0x25, 0xa3, b't', b'x', b's', 0x6b,
            ]),
            "top-level counters sum every interface row"
        );
    }

    fn mp_route_request(verb: &[u8], destination: &[u8; 16]) -> Vec<u8> {
        let mut request = std::vec![0x82, 0xa3];
        request.extend_from_slice(b"get");
        request.push(0xa0 | verb.len() as u8);
        request.extend_from_slice(verb);
        request.push(0xb0);
        request.extend_from_slice(b"destination_hash");
        request.extend_from_slice(&[0xc4, 0x10]);
        request.extend_from_slice(destination);
        request
    }

    fn one_via_route() -> StubQuery {
        StubQuery {
            links: 0,
            routes: std::vec![RouteSnapshot {
                destination: DestinationHash::new([0xab; 16]),
                hops: 2,
                via: NextHop::Via(prns_core::wire::TransportId::new([0xcd; 16])),
                learned_at: prns_core::engine::InstantMillis(0),
                interface: InterfaceId::new([0x07; 8]),
            }],
            interfaces: std::vec![],
        }
    }

    #[tokio::test]
    async fn next_hop_answers_the_via_hash_or_nil_for_an_unknown_destination() {
        let query = one_via_route();

        let reply = reply_for(&mp_route_request(b"next_hop", &[0xab; 16]), &query).await;
        assert_eq!(
            &reply[..2],
            &[0xc4, 0x10],
            "a next-hop hash is a 16-byte bin"
        );
        assert_eq!(&reply[2..], &[0xcd; 16], "the hash is the via transport");

        let unknown = reply_for(&mp_route_request(b"next_hop", &[0x11; 16]), &query).await;
        assert_eq!(unknown, b"\xc0", "an unknown destination has no next hop");
    }

    #[tokio::test]
    async fn a_directly_reachable_next_hop_is_the_destination_itself() {
        let query = StubQuery {
            links: 0,
            routes: std::vec![RouteSnapshot {
                destination: DestinationHash::new([0xab; 16]),
                hops: 1,
                via: NextHop::Direct,
                learned_at: prns_core::engine::InstantMillis(0),
                interface: InterfaceId::new([0x07; 8]),
            }],
            interfaces: std::vec![],
        };
        let reply = reply_for(&mp_route_request(b"next_hop", &[0xab; 16]), &query).await;
        assert_eq!(
            &reply[2..],
            &[0xab; 16],
            "a direct route hops straight to it"
        );
    }

    #[tokio::test]
    async fn next_hop_if_name_is_the_interface_name_or_the_string_none() {
        let query = one_via_route();

        let reply = reply_for(&mp_route_request(b"next_hop_if_name", &[0xab; 16]), &query).await;
        assert_eq!(reply[0] & 0xe0, 0xa0, "a short name is a msgpack fixstr");
        let name = std::str::from_utf8(&reply[1..]).unwrap();
        assert!(name.contains('['), "renders kind[hashprefix]: {name}");

        let unknown = reply_for(&mp_route_request(b"next_hop_if_name", &[0x11; 16]), &query).await;
        assert_eq!(unknown, b"\xa4None", "an unknown route's name is str(None)");
    }

    #[tokio::test]
    async fn a_legacy_pickle_client_gets_next_hop_in_pickle() {
        let query = one_via_route();
        let mut request = std::vec![0x80, 0x02];
        request.extend_from_slice(b"next_hopdestination_hash");
        request.extend_from_slice(&[b'C', 0x10]);
        request.extend_from_slice(&[0xab; 16]);

        let reply = reply_for(&request, &query).await;
        assert_eq!(
            &reply[..4],
            &[0x80, 0x03, b'C', 0x10],
            "pickle SHORT_BINBYTES"
        );
        assert_eq!(&reply[4..20], &[0xcd; 16]);
        assert_eq!(reply[20], b'.', "pickle STOP");
    }

    #[test]
    fn it_seeds_a_missing_rns_identity_then_honors_the_seeded_one() {
        let dir =
            std::env::temp_dir().join(std::format!("prns-rpc-compat-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let seed = [0x33u8; 64];
        let seeded = rpc_key_from_rns_identity(&dir, &seed);
        assert_eq!(seeded, prns_core::crypto::sha256(&seed));

        let honored = rpc_key_from_rns_identity(&dir, &[0x99u8; 64]);
        assert_eq!(honored, seeded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reticulum_storage_dir_uses_the_explicit_config_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvVarRestore::capture("RETICULUM_CONFIG_DIR");
        let config_dir =
            std::env::temp_dir().join(std::format!("prns-reticulum-config-{}", std::process::id()));
        std::env::set_var("RETICULUM_CONFIG_DIR", &config_dir);

        assert_eq!(reticulum_storage_dir(), config_dir.join("storage"));
    }

    #[tokio::test]
    async fn a_modern_sha256_client_completes_the_mutual_auth_and_gets_a_reply() {
        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let telemetry = RpcTelemetry::default();
        let server_telemetry = telemetry.clone();
        let server_task = tokio::spawn(async move {
            let _ = serve_connection(
                server,
                rpc_key,
                StubQuery {
                    links: 0,
                    routes: std::vec![],
                    interfaces: std::vec![],
                },
                server_telemetry,
            )
            .await;
        });

        authenticate_modern_client(&mut client, &rpc_key).await;

        let request = msgpack_request(std::vec![
            ("get", Value::from("packet_rssi")),
            ("packet_hash", Value::Binary(std::vec![0; 32])),
        ]);
        write_frame_dup(&mut client, &request).await;
        assert_eq!(read_frame_dup(&mut client).await, b"\xc0");

        let _ = server_task.await;
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.active_clients, 0);
        assert_eq!(snapshot.total_connections, 1);
        assert_eq!(snapshot.request_frames, 1);
        assert_eq!(snapshot.completed_requests, 1);
        assert_eq!(snapshot.pickle_requests, 0);
        assert_eq!(snapshot.msgpack_requests, 1);
        assert_eq!(snapshot.get_phy_stats, 1);
        assert_eq!(snapshot.auth_failures, 0);
        assert_eq!(snapshot.read_failures, 0);
        assert_eq!(snapshot.write_failures, 0);
    }

    #[tokio::test]
    async fn malformed_msgpack_is_a_protocol_failure_before_dispatch() {
        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let telemetry = RpcTelemetry::default();
        let server_telemetry = telemetry.clone();
        let server_task = tokio::spawn(async move {
            serve_connection(
                server,
                rpc_key,
                StubQuery {
                    links: 0,
                    routes: std::vec![],
                    interfaces: std::vec![],
                },
                server_telemetry,
            )
            .await
        });

        authenticate_modern_client(&mut client, &rpc_key).await;
        let request = msgpack_request(std::vec![
            ("get", Value::from("link_count")),
            ("reason", Value::from("interface_stats")),
        ]);
        write_frame_dup(&mut client, &request).await;

        assert!(server_task.await.unwrap().is_ok());
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.request_frames, 1);
        assert_eq!(snapshot.completed_requests, 0);
        assert_eq!(snapshot.protocol_failures, 1);
        assert_eq!(snapshot.msgpack_requests, 0);
        assert_eq!(snapshot.get_interface_stats, 0);
        assert_eq!(snapshot.get_link_count, 0);
    }

    #[tokio::test]
    async fn a_legacy_md5_client_without_a_digest_prefix_still_authenticates() {
        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let telemetry = RpcTelemetry::default();
        let server_telemetry = telemetry.clone();
        let server_task = tokio::spawn(async move {
            let _ = serve_connection(
                server,
                rpc_key,
                StubQuery {
                    links: 0,
                    routes: std::vec![],
                    interfaces: std::vec![],
                },
                server_telemetry,
            )
            .await;
        });

        let server_challenge = read_frame_dup(&mut client).await;
        let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
        write_frame_dup(&mut client, &Digest::Md5.mac(&rpc_key, server_message)).await;
        assert_eq!(read_frame_dup(&mut client).await, WELCOME);

        let our_message = [0x22u8; LEGACY_MD5_MESSAGE_LEN];
        let mut our_challenge = CHALLENGE.to_vec();
        our_challenge.extend_from_slice(&our_message);
        write_frame_dup(&mut client, &our_challenge).await;
        let server_reply = read_frame_dup(&mut client).await;
        assert_eq!(server_reply, Digest::Md5.mac(&rpc_key, &our_message));
        write_frame_dup(&mut client, WELCOME).await;

        write_frame_dup(&mut client, b"{'get': 'packet_rssi'}").await;
        assert_eq!(read_frame_dup(&mut client).await, b"N.");

        let _ = server_task.await;
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.active_clients, 0);
        assert_eq!(snapshot.total_connections, 1);
        assert_eq!(snapshot.completed_requests, 1);
        assert_eq!(snapshot.pickle_requests, 1);
        assert_eq!(snapshot.get_phy_stats, 1);
    }

    #[test]
    fn explicit_md5_digest_messages_are_supported_and_bad_macs_fail() {
        let rpc_key = [0x5au8; 32];
        let mut md5_message = b"{md5}".to_vec();
        md5_message.extend_from_slice(b"client nonce");

        let response = create_response(&rpc_key, &md5_message).unwrap();
        let mac = response.strip_prefix(b"{md5}").unwrap();
        assert!(Digest::Md5.verify(&rpc_key, &md5_message, mac));
        assert!(response_authenticates(&rpc_key, &md5_message, &response));

        let mut bad_md5 = b"{md5}".to_vec();
        bad_md5.extend_from_slice(&[0u8; LEGACY_MD5_DIGEST_LEN]);
        assert!(!response_authenticates(&rpc_key, &md5_message, &bad_md5));

        let mut sha_message = DIGEST_PREFIX.to_vec();
        sha_message.extend_from_slice(b"server nonce");
        let mut bad_sha = DIGEST_PREFIX.to_vec();
        bad_sha.extend_from_slice(&[0u8; 32]);
        assert!(!response_authenticates(&rpc_key, &sha_message, &bad_sha));
    }

    #[tokio::test]
    async fn deliver_our_challenge_rejects_a_bad_client_mac() {
        let rpc_key = [0x5au8; 32];
        let (mut client, mut server) = tokio::io::duplex(8192);
        let server_task =
            tokio::spawn(async move { deliver_our_challenge(&mut server, &rpc_key).await });

        let challenge = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_frame_dup(&mut client),
        )
        .await
        .expect("server sends a challenge before authenticating");
        assert!(challenge.starts_with(CHALLENGE));

        let mut bad_response = DIGEST_PREFIX.to_vec();
        bad_response.extend_from_slice(&[0u8; 32]);
        write_frame_dup(&mut client, &bad_response).await;

        let failure = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_frame_dup(&mut client),
        )
        .await
        .expect("server rejects a bad response with #FAILURE#");
        assert_eq!(failure, FAILURE);
        assert!(!server_task.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn frame_reader_accepts_the_maximum_size_and_rejects_one_byte_more() {
        let max_payload = std::vec![0x42; MAX_FRAME_LEN];
        let (mut client, mut server) = tokio::io::duplex(MAX_FRAME_LEN + 16);
        write_frame_dup(&mut client, &max_payload).await;
        assert_eq!(read_frame(&mut server).await.unwrap(), max_payload);

        let over_payload = std::vec![0x24; MAX_FRAME_LEN + 1];
        let (mut client, mut server) = tokio::io::duplex(MAX_FRAME_LEN + 16);
        client
            .write_all(&(over_payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        client.write_all(&over_payload).await.unwrap();
        client.flush().await.unwrap();
        assert_eq!(
            read_frame(&mut server).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn tcp_run_accepts_a_modern_client_connection() {
        let rpc_key = [0x5au8; 32];
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let server = SharedInstanceRpcCompat::tcp(
            rpc_key,
            port,
            StubQuery {
                links: 7,
                routes: std::vec![],
                interfaces: std::vec![],
            },
        );
        let server_task = tokio::spawn(server.run());

        let mut stream = None;
        for _ in 0..20 {
            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
            }
        }
        let mut client = stream.expect("RPC listener accepts loopback clients");

        let server_challenge = read_test_frame(&mut client).await;
        let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
        let mut response = DIGEST_PREFIX.to_vec();
        response.extend_from_slice(&hmac_sha256(&rpc_key, server_message));
        write_frame(&mut client, &response).await.unwrap();
        assert_eq!(read_test_frame(&mut client).await, WELCOME);

        let mut our_msg = DIGEST_PREFIX.to_vec();
        our_msg.extend_from_slice(&[0x44u8; CHALLENGE_NONCE_LEN]);
        let mut our_challenge = CHALLENGE.to_vec();
        our_challenge.extend_from_slice(&our_msg);
        write_frame(&mut client, &our_challenge).await.unwrap();
        let server_reply = read_test_frame(&mut client).await;
        let server_mac = server_reply.strip_prefix(DIGEST_PREFIX).unwrap();
        assert!(hmac_sha256_verify(&rpc_key, &our_msg, server_mac).is_ok());
        write_frame(&mut client, WELCOME).await.unwrap();

        write_frame(&mut client, b"\x81\xa3get\xaalink_count")
            .await
            .unwrap();
        assert_eq!(read_test_frame(&mut client).await, b"\x07");

        server_task.abort();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn abstract_unix_constructor_and_binder_are_wired() {
        let server = SharedInstanceRpcCompat::abstract_unix(
            [0x5au8; 32],
            "mutation-proof",
            StubQuery {
                links: 0,
                routes: std::vec![],
                interfaces: std::vec![],
            },
        );
        match server.bind {
            RpcBind::Abstract(path) => assert_eq!(path, "mutation-proof"),
            RpcBind::Tcp(_) => panic!("abstract_unix must not create a TCP bind"),
        }

        let socket_name = std::format!("mutation-proof-{}", std::process::id());
        assert!(bind_abstract_rpc(&socket_name).is_some());
    }
}
