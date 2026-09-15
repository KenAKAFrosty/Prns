use std::sync::Arc;
use std::time::Duration;

use prns_core::interfaces::shared_instance as instance_core;
use prns_core::interfaces::EffectiveInterfacePolicy;
use prns_runtime::runtime::{BitrateTimingOracle, PrnsNodeHandle};
use tokio::net::TcpStream;

use super::persistence::{RnsBlackholeFiles, RnsPersistedBlackholes};
use super::rns_rpc::{
    SharedInstanceCredentials, SharedInstanceRpcBindError, SharedInstanceRpcClient,
    SharedInstanceRpcEndpoint, SharedInstanceRpcServer,
};
use super::supervision::{
    SharedInstanceClient, SharedInstanceServer, SharedInstanceServerBindError,
};

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

pub struct SharedInstanceIntent {
    pub credentials: SharedInstanceCredentials,
    pub blackhole_source: prns_core::identity::IdentityHash,
    pub transport_identity: prns_core::identity::IdentityHash,
    pub network_identity: Option<prns_core::identity::IdentityHash>,
    pub probe_responder: Option<prns_core::wire::DestinationHash>,
    pub blackhole_files: RnsBlackholeFiles,
    pub ports: SharedInstancePorts,
    pub transport: SharedInstanceTransport,
    pub policy: EffectiveInterfacePolicy,
    pub on_existing: ExistingSharedInstancePolicy,
}

pub struct SharedInstanceClientIntent {
    pub bus_port: u16,
    pub transport: SharedInstanceTransport,
    pub policy: EffectiveInterfacePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceTransport {
    Tcp,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix {
        socket_path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedInstancePorts {
    pub bus: u16,
    pub control: u16,
}

impl Default for SharedInstancePorts {
    fn default() -> Self {
        Self {
            bus: instance_core::DEFAULT_LOCAL_PORT,
            control: instance_core::DEFAULT_LOCAL_PORT + 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExistingSharedInstancePolicy {
    #[default]
    JoinAsClient,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceRole {
    BecameInstance,
    JoinedAsClient { of: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceJoinError {
    InstanceAlreadyRunning {
        at: String,
    },
    EndpointUnavailable {
        endpoint: SharedInstanceEndpoint,
        kind: std::io::ErrorKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceBusEndpoint {
    Tcp {
        port: u16,
    },
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix {
        socket_path: String,
    },
}

impl std::fmt::Display for SharedInstanceBusEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp { port } => write!(formatter, "127.0.0.1:{port}"),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Self::AbstractUnix { socket_path } => write!(formatter, "\\0rns/{socket_path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingSharedInstanceUnavailable {
    pub endpoint: SharedInstanceBusEndpoint,
}

impl std::fmt::Display for ExistingSharedInstanceUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "no shared RNS instance answered at {}",
            self.endpoint
        )
    }
}

impl std::error::Error for ExistingSharedInstanceUnavailable {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceEndpoint {
    TcpBus,
    TcpControl,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnixBus,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnixControl,
}

impl SharedInstanceEndpoint {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TcpBus => "tcp_bus",
            Self::TcpControl => "tcp_control",
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Self::AbstractUnixBus => "abstract_unix_bus",
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Self::AbstractUnixControl => "abstract_unix_control",
        }
    }
}

struct SharedInstanceActivationError {
    endpoint: SharedInstanceEndpoint,
    kind: std::io::ErrorKind,
}

pub async fn join_shared_instance(
    handle: &PrnsNodeHandle,
    instance: SharedInstanceIntent,
) -> Result<SharedInstanceRole, SharedInstanceJoinError> {
    if let Some(role) = join_existing(handle, &instance).await? {
        return Ok(role);
    }
    match become_instance(handle, &instance).await {
        Ok(()) => Ok(SharedInstanceRole::BecameInstance),
        Err(error) => {
            if let Some(role) = join_existing(handle, &instance).await? {
                return Ok(role);
            }
            Err(SharedInstanceJoinError::EndpointUnavailable {
                endpoint: error.endpoint,
                kind: error.kind,
            })
        }
    }
}

pub async fn connect_existing_shared_instance(
    handle: &PrnsNodeHandle,
    instance: SharedInstanceClientIntent,
) -> Result<SharedInstanceRole, ExistingSharedInstanceUnavailable> {
    match instance.transport {
        SharedInstanceTransport::Tcp => {
            let endpoint = SharedInstanceBusEndpoint::Tcp {
                port: instance.bus_port,
            };
            let address = endpoint.to_string();
            let Some(stream) = probe_tcp(&address).await else {
                return Err(ExistingSharedInstanceUnavailable { endpoint });
            };
            let of = stream
                .peer_addr()
                .map(|address| address.to_string())
                .unwrap_or(address);
            Ok(attach_existing(handle, stream, of, instance.policy))
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            let endpoint = SharedInstanceBusEndpoint::AbstractUnix {
                socket_path: socket_path.clone(),
            };
            let Some(stream) = connect_abstract_bus(&socket_path) else {
                return Err(ExistingSharedInstanceUnavailable { endpoint });
            };
            Ok(attach_existing(
                handle,
                stream,
                endpoint.to_string(),
                instance.policy,
            ))
        }
    }
}

/// Join the shared bus and install its authenticated control connection as the transparent
/// timing source for path, link and single-packet watchdogs.
pub async fn connect_existing_shared_instance_with_timing(
    handle: &PrnsNodeHandle,
    instance: SharedInstanceClientIntent,
    timing: Arc<SharedInstanceRpcClient>,
) -> Result<SharedInstanceRole, ExistingSharedInstanceUnavailable> {
    let role = connect_existing_shared_instance(handle, instance).await?;
    handle.install_bitrate_timing_oracle(timing);
    Ok(role)
}

impl BitrateTimingOracle for SharedInstanceRpcClient {
    fn first_hop_timeout(
        &self,
        destination: prns_core::wire::DestinationHash,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Duration>> + Send + '_>> {
        Box::pin(async move {
            SharedInstanceRpcClient::first_hop_timeout(self, destination)
                .await
                .ok()
        })
    }

    fn medium_path_timeout(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Duration>> + Send + '_>> {
        Box::pin(async move {
            SharedInstanceRpcClient::medium_path_timeout(self)
                .await
                .ok()
        })
    }
}

async fn join_existing(
    handle: &PrnsNodeHandle,
    instance: &SharedInstanceIntent,
) -> Result<Option<SharedInstanceRole>, SharedInstanceJoinError> {
    match &instance.transport {
        SharedInstanceTransport::Tcp => {
            let bus_addr = std::format!("127.0.0.1:{}", instance.ports.bus);
            if let Some(stream) = probe_tcp(&bus_addr).await {
                let at = stream
                    .peer_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or(bus_addr);
                let role =
                    join_or_refuse(handle, stream, at, instance.on_existing, instance.policy)?;
                install_existing_instance_timing(handle, instance);
                return Ok(Some(role));
            }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            if let Some(stream) = connect_abstract_bus(socket_path) {
                let at = std::format!("\\0rns/{socket_path}");
                let role =
                    join_or_refuse(handle, stream, at, instance.on_existing, instance.policy)?;
                install_existing_instance_timing(handle, instance);
                return Ok(Some(role));
            }
        }
    }
    Ok(None)
}

fn install_existing_instance_timing(handle: &PrnsNodeHandle, instance: &SharedInstanceIntent) {
    let endpoint = match &instance.transport {
        SharedInstanceTransport::Tcp => SharedInstanceRpcEndpoint::tcp(instance.ports.control),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            SharedInstanceRpcEndpoint::abstract_unix(socket_path.clone())
        }
    };
    handle.install_bitrate_timing_oracle(Arc::new(SharedInstanceRpcClient::new(
        endpoint,
        instance.credentials.rpc_key().clone(),
        Duration::from_secs(15),
    )));
}

fn join_or_refuse<S>(
    handle: &PrnsNodeHandle,
    stream: S,
    at: String,
    on_existing: ExistingSharedInstancePolicy,
    policy: EffectiveInterfacePolicy,
) -> Result<SharedInstanceRole, SharedInstanceJoinError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match on_existing {
        ExistingSharedInstancePolicy::JoinAsClient => {
            Ok(attach_existing(handle, stream, at, policy))
        }
        ExistingSharedInstancePolicy::Refuse => {
            Err(SharedInstanceJoinError::InstanceAlreadyRunning { at })
        }
    }
}

fn attach_existing<S>(
    handle: &PrnsNodeHandle,
    stream: S,
    of: String,
    policy: EffectiveInterfacePolicy,
) -> SharedInstanceRole
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    handle.add_interface(SharedInstanceClient::with_policy(
        of.clone().into_bytes(),
        stream,
        policy,
    ));
    SharedInstanceRole::JoinedAsClient { of }
}

async fn become_instance(
    handle: &PrnsNodeHandle,
    instance: &SharedInstanceIntent,
) -> Result<(), SharedInstanceActivationError> {
    let server = match &instance.transport {
        SharedInstanceTransport::Tcp => SharedInstanceServer::with_port(instance.ports.bus),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            SharedInstanceServer::abstract_unix(socket_path.clone())
        }
    }
    .with_policy(instance.policy)
    .bind()
    .await
    .map_err(SharedInstanceActivationError::from_bus)?;
    let blackholes = RnsPersistedBlackholes::new(
        handle.clone(),
        instance.blackhole_source,
        instance.blackhole_files.clone(),
    );
    let rpc = match &instance.transport {
        SharedInstanceTransport::Tcp => SharedInstanceRpcServer::tcp_with_blackholes(
            instance.credentials.clone(),
            instance.blackhole_source,
            instance.ports.control,
            handle.clone(),
            blackholes,
        ),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            SharedInstanceRpcServer::abstract_unix_with_blackholes(
                instance.credentials.clone(),
                instance.blackhole_source,
                socket_path,
                handle.clone(),
                blackholes,
            )
        }
    }
    .with_transport_identity(instance.transport_identity)
    .with_network_identity(instance.network_identity)
    .with_probe_responder(instance.probe_responder)
    .bind()
    .await
    .map_err(SharedInstanceActivationError::from_control)?;
    handle.supervise(server);
    tokio::spawn(rpc.run());
    Ok(())
}

impl SharedInstanceActivationError {
    fn from_bus(error: SharedInstanceServerBindError) -> Self {
        match error {
            SharedInstanceServerBindError::Tcp(kind) => Self {
                endpoint: SharedInstanceEndpoint::TcpBus,
                kind,
            },
            #[cfg(any(target_os = "linux", target_os = "android"))]
            SharedInstanceServerBindError::AbstractUnix(kind) => Self {
                endpoint: SharedInstanceEndpoint::AbstractUnixBus,
                kind,
            },
        }
    }

    fn from_control(error: SharedInstanceRpcBindError) -> Self {
        match error {
            SharedInstanceRpcBindError::Tcp(kind) => Self {
                endpoint: SharedInstanceEndpoint::TcpControl,
                kind,
            },
            #[cfg(any(target_os = "linux", target_os = "android"))]
            SharedInstanceRpcBindError::AbstractUnix(kind) => Self {
                endpoint: SharedInstanceEndpoint::AbstractUnixControl,
                kind,
            },
        }
    }
}

async fn probe_tcp(addr: &str) -> Option<TcpStream> {
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Some(stream)
        }
        _ => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn connect_abstract_bus(socket_path: &str) -> Option<tokio::net::UnixStream> {
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let stream = std::os::unix::net::UnixStream::connect_addr(&addr).ok()?;
    stream.set_nonblocking(true).ok()?;
    tokio::net::UnixStream::from_std(stream).ok()
}
