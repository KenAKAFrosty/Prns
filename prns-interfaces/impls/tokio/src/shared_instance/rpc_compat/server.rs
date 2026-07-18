use std::string::String;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;

use prns_runtime::node_introspection::NodeIntrospection;
use prns_runtime::runtime::{
    DestinationIdentityRetentionControl, IdentityBlackholeControl, IdentityBlackholeSource,
    RoutingControl,
};

use super::authentication::{
    answer_client_challenge, deliver_our_challenge, SharedInstanceCredentials,
};
use super::dispatch::reply_for_decoded;
use super::framing::{read_frame, write_frame};
use super::protocol::RpcRequest;
use super::telemetry::RpcTelemetry;

/// Answers the RNS shared-instance control RPC for stock clients, with the minimal replies that keep attachment delivery from faulting. Stand one up beside a [`LocalServer`](crate::shared_instance::server::LocalServer) and drive it with [`run`](Self::run).
pub struct SharedInstanceRpcCompat<Q, B = Q> {
    credentials: SharedInstanceCredentials,
    pub(super) bind: RpcBind,
    query: Q,
    blackholes: B,
    telemetry: RpcTelemetry,
}

pub(super) enum RpcBind {
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
    /// Answer on a loopback TCP port — RNS's `instance_control_port` (default 37428's sibling 37429), or whatever a client configured. `rpc_key` MUST equal the clients' key: RNS's `full_hash` of the shared transport identity's private key, or a value both sides set as `rpc_key` in config. `query` is the node handle the shim reads engine state through to answer each verb.
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

    /// Answer on the abstract AF_UNIX socket `\0rns/{socket_path}/rpc` that a default-config RNS client uses on Linux. Linux only.
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

    /// Accept control connections forever, serving each on its own task. Returns only if the listener cannot be bound.
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

/// Bind `\0rns/{socket_path}/rpc` in the Linux abstract namespace (leading null implied), mirroring how the data bus binds `\0rns/{socket_path}`.
#[cfg(target_os = "linux")]
pub(super) fn bind_abstract_rpc(socket_path: &str) -> Option<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    listener.set_nonblocking(true).ok()?;
    UnixListener::from_std(listener).ok()
}

pub(super) async fn serve_connection<S, Q, B>(
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
