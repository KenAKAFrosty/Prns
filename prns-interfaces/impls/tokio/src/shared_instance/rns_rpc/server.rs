use std::string::String;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
#[cfg(any(target_os = "linux", target_os = "android"))]
use tokio::net::UnixListener;

use prns_core::interfaces::rns_management::RnsTransportStatus;
use prns_core::interfaces::shared_instance::rns_rpc::{RpcDialect, RpcRequest};
use prns_runtime::node_introspection::NodeIntrospection;
use prns_runtime::runtime::rns_rpc;
use prns_runtime::runtime::{
    DestinationIdentityRetentionControl, IdentityBlackholeControl, IdentityBlackholeSource,
    RoutingControl,
};

use super::authentication::{
    answer_client_challenge, deliver_our_challenge, SharedInstanceCredentials,
};
use super::framing::{read_frame, write_frame};
use super::legacy;
use super::telemetry::RpcTelemetry;

pub struct SharedInstanceRpcServer<Q, B = Q> {
    credentials: SharedInstanceCredentials,
    pub(super) blackhole_source: prns_core::identity::IdentityHash,
    pub(super) bind: RpcBind,
    query: Q,
    blackholes: B,
    telemetry: RpcTelemetry,
    started_at: std::time::Instant,
    transport_identity: prns_core::identity::IdentityHash,
    network_identity: Option<prns_core::identity::IdentityHash>,
    probe_responder: Option<prns_core::wire::DestinationHash>,
}

pub struct SharedInstanceRpcListener<Q, B = Q> {
    listener: RpcListener,
    service: RpcService<Q, B>,
}

#[derive(Clone)]
pub(super) struct RpcService<Q, B> {
    pub(super) credentials: SharedInstanceCredentials,
    pub(super) blackhole_source: prns_core::identity::IdentityHash,
    pub(super) query: Q,
    pub(super) blackholes: B,
    pub(super) telemetry: RpcTelemetry,
    pub(super) started_at: std::time::Instant,
    pub(super) transport_identity: prns_core::identity::IdentityHash,
    pub(super) network_identity: Option<prns_core::identity::IdentityHash>,
    pub(super) probe_responder: Option<prns_core::wire::DestinationHash>,
}

pub(super) enum RpcBind {
    Tcp(String),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    Abstract(String),
}

enum RpcListener {
    Tcp(TcpListener),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix(UnixListener),
}

enum RpcListenerKind {
    Tcp,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix,
}

impl RpcListenerKind {
    #[cfg(feature = "tracing")]
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Self::AbstractUnix => "abstract_unix",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SharedInstanceRpcBindError {
    Tcp(std::io::ErrorKind),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix(std::io::ErrorKind),
}

const ACCEPT_RETRY_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const RPC_CONNECTION_IO_TIMEOUT: Duration = Duration::from_secs(10);

impl<Q> SharedInstanceRpcServer<Q, Q>
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
    /// Answer on a loopback TCP port — RNS's `instance_control_port` (default 37428's sibling 37429), or whatever a client configured. `rpc_key` MUST equal the clients' key: RNS's `full_hash` of the shared transport identity's private key, or a value both sides set as `rpc_key` in config. `query` is the node handle the shim reads engine state through to answer each verb.
    #[must_use]
    pub fn tcp(credentials: SharedInstanceCredentials, port: u16, query: Q) -> Self {
        let blackhole_source = credentials.transport_identity_hash();
        let transport_identity = credentials.transport_identity_hash();
        Self {
            credentials,
            blackhole_source,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
            blackholes: query.clone(),
            query,
            telemetry: RpcTelemetry::default(),
            started_at: std::time::Instant::now(),
            transport_identity,
            network_identity: None,
            probe_responder: None,
        }
    }

    /// Answer on the abstract AF_UNIX socket `\0rns/{socket_path}/rpc` that a default-config RNS client uses on Linux. Linux only.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn abstract_unix(
        credentials: SharedInstanceCredentials,
        socket_path: impl Into<String>,
        query: Q,
    ) -> Self {
        let blackhole_source = credentials.transport_identity_hash();
        let transport_identity = credentials.transport_identity_hash();
        Self {
            credentials,
            blackhole_source,
            bind: RpcBind::Abstract(socket_path.into()),
            blackholes: query.clone(),
            query,
            telemetry: RpcTelemetry::default(),
            started_at: std::time::Instant::now(),
            transport_identity,
            network_identity: None,
            probe_responder: None,
        }
    }
}

impl<Q, B> SharedInstanceRpcServer<Q, B>
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
        blackhole_source: prns_core::identity::IdentityHash,
        port: u16,
        query: Q,
        blackholes: B,
    ) -> Self {
        let transport_identity = credentials.transport_identity_hash();
        Self {
            credentials,
            blackhole_source,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
            query,
            blackholes,
            telemetry: RpcTelemetry::default(),
            started_at: std::time::Instant::now(),
            transport_identity,
            network_identity: None,
            probe_responder: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn abstract_unix_with_blackholes(
        credentials: SharedInstanceCredentials,
        blackhole_source: prns_core::identity::IdentityHash,
        socket_path: impl Into<String>,
        query: Q,
        blackholes: B,
    ) -> Self {
        let transport_identity = credentials.transport_identity_hash();
        Self {
            credentials,
            blackhole_source,
            bind: RpcBind::Abstract(socket_path.into()),
            query,
            blackholes,
            telemetry: RpcTelemetry::default(),
            started_at: std::time::Instant::now(),
            transport_identity,
            network_identity: None,
            probe_responder: None,
        }
    }

    #[must_use]
    pub fn telemetry(&self) -> RpcTelemetry {
        self.telemetry.clone()
    }

    #[must_use]
    pub fn with_telemetry(mut self, telemetry: RpcTelemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    #[must_use]
    pub fn with_network_identity(
        mut self,
        network_identity: Option<prns_core::identity::IdentityHash>,
    ) -> Self {
        self.network_identity = network_identity;
        self
    }

    #[must_use]
    pub fn with_transport_identity(
        mut self,
        transport_identity: prns_core::identity::IdentityHash,
    ) -> Self {
        self.transport_identity = transport_identity;
        self
    }

    #[must_use]
    pub fn with_probe_responder(
        mut self,
        probe_responder: Option<prns_core::wire::DestinationHash>,
    ) -> Self {
        self.probe_responder = probe_responder;
        self
    }

    pub async fn bind(self) -> Result<SharedInstanceRpcListener<Q, B>, SharedInstanceRpcBindError> {
        let Self {
            credentials,
            blackhole_source,
            bind,
            query,
            blackholes,
            telemetry,
            started_at,
            transport_identity,
            network_identity,
            probe_responder,
        } = self;
        let listener = match bind {
            RpcBind::Tcp(address) => RpcListener::Tcp(
                TcpListener::bind(address)
                    .await
                    .map_err(|error| SharedInstanceRpcBindError::Tcp(error.kind()))?,
            ),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            RpcBind::Abstract(socket_path) => RpcListener::AbstractUnix(
                bind_abstract_rpc(&socket_path)
                    .map_err(|error| SharedInstanceRpcBindError::AbstractUnix(error.kind()))?,
            ),
        };
        Ok(SharedInstanceRpcListener {
            listener,
            service: RpcService {
                credentials,
                blackhole_source,
                query,
                blackholes,
                telemetry,
                started_at,
                transport_identity,
                network_identity,
                probe_responder,
            },
        })
    }
}

impl<Q, B> SharedInstanceRpcListener<Q, B>
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
    pub async fn run(self) {
        match self.listener {
            RpcListener::Tcp(listener) => loop {
                match listener.accept().await {
                    Ok((stream, _)) => self.service.serve(stream),
                    Err(error) => recover_from_accept_error(error, RpcListenerKind::Tcp).await,
                }
            },
            #[cfg(any(target_os = "linux", target_os = "android"))]
            RpcListener::AbstractUnix(listener) => loop {
                match listener.accept().await {
                    Ok((stream, _)) => self.service.serve(stream),
                    Err(error) => {
                        recover_from_accept_error(error, RpcListenerKind::AbstractUnix).await;
                    }
                }
            },
        }
    }
}

impl<Q, B> RpcService<Q, B>
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
    fn serve<S>(&self, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let service = self.clone();
        tokio::spawn(async move {
            let _ = serve_connection(stream, service).await;
        });
    }
}

async fn recover_from_accept_error(error: std::io::Error, listener: RpcListenerKind) {
    #[cfg(feature = "tracing")]
    tracing::warn!(
        event = "shared_instance_rpc_accept_failed",
        transport = listener.as_str(),
        error_kind = ?error.kind(),
    );
    #[cfg(not(feature = "tracing"))]
    let _ = (error, listener);
    tokio::time::sleep(ACCEPT_RETRY_INTERVAL).await;
}

/// Bind `\0rns/{socket_path}/rpc` in the Linux abstract namespace (leading null implied), mirroring how the data bus binds `\0rns/{socket_path}`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn bind_abstract_rpc(socket_path: &str) -> std::io::Result<UnixListener> {
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr)?;
    listener.set_nonblocking(true)?;
    UnixListener::from_std(listener)
}

pub(super) async fn serve_connection<S, Q, B>(
    mut stream: S,
    service: RpcService<Q, B>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    Q: NodeIntrospection + RoutingControl + DestinationIdentityRetentionControl,
    B: IdentityBlackholeSource + IdentityBlackholeControl,
{
    let RpcService {
        credentials,
        blackhole_source,
        query,
        blackholes,
        telemetry,
        started_at,
        transport_identity,
        network_identity,
        probe_responder,
    } = service;
    let _active = telemetry.connection_opened();
    let client_authenticated =
        match with_io_timeout(deliver_our_challenge(&mut stream, credentials.rpc_key())).await {
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
        match with_io_timeout(answer_client_challenge(&mut stream, credentials.rpc_key())).await {
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
    let request = match with_io_timeout(read_frame(&mut stream)).await {
        Ok(request) => request,
        Err(err) => {
            telemetry.record_read_failure(err.kind());
            return Err(err);
        }
    };
    telemetry.record_request_frame();
    let decoded = match RpcRequest::decode(&request) {
        Ok(request) => request,
        Err(_) => {
            telemetry.record_protocol_failure();
            return Ok(());
        }
    };
    let dialect = decoded.dialect();
    let request = match decoded {
        RpcRequest::Msgpack(request) => request,
        RpcRequest::Pickle(_) => match legacy::decode_request(&request) {
            Ok(request) => request,
            Err(_) => {
                telemetry.record_protocol_failure();
                return Ok(());
            }
        },
    };
    let verb = request.verb();
    telemetry.record_request(dialect, verb);
    #[cfg(feature = "tracing")]
    tracing::debug!(
        event = "shared_instance_rpc_request",
        dialect = dialect.as_str(),
        verb = verb.as_str()
    );
    let reply = rns_rpc::reply_decoded(
        &request,
        &query,
        &query,
        &query,
        &blackholes,
        blackhole_source,
        Some(
            RnsTransportStatus::new(transport_identity, network_identity, started_at.elapsed())
                .with_probe_responder(probe_responder),
        ),
    )
    .await
    .map_err(std::io::Error::other)?;
    let reply = match dialect {
        RpcDialect::Msgpack => reply,
        RpcDialect::Pickle => legacy::encode_reply(&reply).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "legacy shared-instance RPC reply could not be encoded",
            )
        })?,
    };
    if let Err(err) = with_io_timeout(write_frame(&mut stream, &reply)).await {
        telemetry.record_write_failure();
        return Err(err);
    }
    telemetry.record_completed();
    Ok(())
}

async fn with_io_timeout<T>(
    operation: impl core::future::Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    tokio::time::timeout(RPC_CONNECTION_IO_TIMEOUT, operation)
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}
