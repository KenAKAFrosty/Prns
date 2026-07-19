//! Participating in the host's local Reticulum shared instance — the bus stock RNS apps (Sideband,
//! NomadNet, MeshChat) and other Prns nodes on the same machine share so one process owns the radios
//! and the rest ride them. [`join_shared_instance`] elects the node's role: become the instance if
//! none is running, else join the running one as a client, the parity behavior a stock RNS app
//! follows. A host-only concept (one OS, many processes), so it lives beside the bus and control-RPC
//! pieces it builds on.

use std::time::Duration;

use prns_core::interfaces::shared_instance::core as instance_core;
use prns_core::interfaces::EffectiveInterfacePolicy;
use prns_runtime::runtime::PrnsNodeHandle;
use tokio::net::TcpStream;

use super::blackhole_compat::{RnsBlackholeFiles, RnsPersistedBlackholes};
use super::rpc_compat::{
    SharedInstanceCredentials, SharedInstanceRpcBindError, SharedInstanceRpcCompat,
};
use super::server::{LocalClientInterface, LocalServer, LocalServerBindError};

/// How long the probe waits for the bus to answer before concluding no instance is running.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// How a node should participate in the host's local shared instance.
pub struct SharedInstanceIntent {
    pub credentials: SharedInstanceCredentials,
    pub blackhole_source: prns_core::identity::IdentityHash,
    pub transport_identity: prns_core::identity::IdentityHash,
    pub network_identity: Option<prns_core::identity::IdentityHash>,
    pub blackhole_files: RnsBlackholeFiles,
    /// The bus and control ports (RNS defaults 37428 / 37429).
    pub ports: InstancePorts,
    pub transport: SharedInstanceTransport,
    pub policy: EffectiveInterfacePolicy,
    /// What to do when an instance is already running.
    pub on_existing: OnExisting,
}

pub struct SharedInstanceClientIntent {
    pub bus_port: u16,
    pub transport: SharedInstanceTransport,
    pub policy: EffectiveInterfacePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceTransport {
    Tcp,
    #[cfg(target_os = "linux")]
    AbstractUnix {
        socket_path: String,
    },
}

/// The shared instance's two ports: the data bus local apps connect to, and the control RPC that
/// answers their diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstancePorts {
    pub bus: u16,
    pub control: u16,
}

impl Default for InstancePorts {
    fn default() -> Self {
        Self {
            bus: instance_core::DEFAULT_LOCAL_PORT,
            control: instance_core::DEFAULT_LOCAL_PORT + 1,
        }
    }
}

/// What to do when a shared instance is already running on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnExisting {
    /// Defer to it and ride its bus as a client — the parity behavior a stock RNS app follows.
    #[default]
    JoinAsClient,
    /// Decline to take any role, returning [`JoinError::InstanceAlreadyRunning`] — for a process that
    /// must own the instance or not run at all.
    Refuse,
}

/// The role a node took in the host's shared instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// No instance was running, so this node became it: it now serves the bus for local apps and
    /// answers the control RPC.
    BecameInstance,
    /// An instance was already running, so this node joined it as a client over its bus at `of` (a
    /// TCP address, or on Linux the abstract socket the instance binds). The instance owns the
    /// interfaces; the node rides them and should not stand up its own.
    JoinedAsClient { of: String },
}

/// Why [`join_shared_instance`] took no role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// An instance was already running at `at`, and [`OnExisting::Refuse`] forbade joining it.
    InstanceAlreadyRunning { at: String },
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
    #[cfg(target_os = "linux")]
    AbstractUnix {
        socket_path: String,
    },
}

impl std::fmt::Display for SharedInstanceBusEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp { port } => write!(formatter, "127.0.0.1:{port}"),
            #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    AbstractUnixBus,
    #[cfg(target_os = "linux")]
    AbstractUnixControl,
}

impl SharedInstanceEndpoint {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TcpBus => "tcp_bus",
            Self::TcpControl => "tcp_control",
            #[cfg(target_os = "linux")]
            Self::AbstractUnixBus => "abstract_unix_bus",
            #[cfg(target_os = "linux")]
            Self::AbstractUnixControl => "abstract_unix_control",
        }
    }
}

struct SharedInstanceActivationError {
    endpoint: SharedInstanceEndpoint,
    kind: std::io::ErrorKind,
}

/// Participate in the host's local Reticulum shared instance, electing the node's role.
///
/// Probe the bus port: if an instance answers, this node defers and joins as a client over its
/// bus (or, under [`OnExisting::Refuse`], declines), the same honorable behavior a stock RNS
/// app follows. If nothing answers, the node becomes the instance: it supervises the bus for
/// local apps and serves the control RPC keyed on its identity. A client only attaches the bus
/// link; the instance owns the interfaces, bus, and RPC. The probe and listener use the one
/// selected transport: TCP everywhere, or on Linux the abstract AF_UNIX socket `\0rns/{name}`.
pub async fn join_shared_instance(
    handle: &PrnsNodeHandle,
    instance: SharedInstanceIntent,
) -> Result<Role, JoinError> {
    if let Some(role) = join_existing(handle, &instance).await? {
        return Ok(role);
    }
    match become_instance(handle, &instance).await {
        Ok(()) => Ok(Role::BecameInstance),
        Err(error) => {
            if let Some(role) = join_existing(handle, &instance).await? {
                return Ok(role);
            }
            Err(JoinError::EndpointUnavailable {
                endpoint: error.endpoint,
                kind: error.kind,
            })
        }
    }
}

pub async fn connect_existing_shared_instance(
    handle: &PrnsNodeHandle,
    instance: SharedInstanceClientIntent,
) -> Result<Role, ExistingSharedInstanceUnavailable> {
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
        #[cfg(target_os = "linux")]
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

async fn join_existing(
    handle: &PrnsNodeHandle,
    instance: &SharedInstanceIntent,
) -> Result<Option<Role>, JoinError> {
    match &instance.transport {
        SharedInstanceTransport::Tcp => {
            let bus_addr = std::format!("127.0.0.1:{}", instance.ports.bus);
            if let Some(stream) = probe_tcp(&bus_addr).await {
                let at = stream
                    .peer_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or(bus_addr);
                return join_or_refuse(handle, stream, at, instance.on_existing, instance.policy)
                    .map(Some);
            }
        }
        #[cfg(target_os = "linux")]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            if let Some(stream) = connect_abstract_bus(socket_path) {
                let at = std::format!("\\0rns/{socket_path}");
                return join_or_refuse(handle, stream, at, instance.on_existing, instance.policy)
                    .map(Some);
            }
        }
    }
    Ok(None)
}

fn join_or_refuse<S>(
    handle: &PrnsNodeHandle,
    stream: S,
    at: String,
    on_existing: OnExisting,
    policy: EffectiveInterfacePolicy,
) -> Result<Role, JoinError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match on_existing {
        OnExisting::JoinAsClient => Ok(attach_existing(handle, stream, at, policy)),
        OnExisting::Refuse => Err(JoinError::InstanceAlreadyRunning { at }),
    }
}

fn attach_existing<S>(
    handle: &PrnsNodeHandle,
    stream: S,
    of: String,
    policy: EffectiveInterfacePolicy,
) -> Role
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    handle.add_interface(LocalClientInterface::with_policy(
        of.clone().into_bytes(),
        stream,
        policy,
    ));
    Role::JoinedAsClient { of }
}

async fn become_instance(
    handle: &PrnsNodeHandle,
    instance: &SharedInstanceIntent,
) -> Result<(), SharedInstanceActivationError> {
    let server = match &instance.transport {
        SharedInstanceTransport::Tcp => LocalServer::with_port(instance.ports.bus),
        #[cfg(target_os = "linux")]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            LocalServer::abstract_unix(socket_path.clone())
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
        SharedInstanceTransport::Tcp => SharedInstanceRpcCompat::tcp_with_blackholes(
            instance.credentials.clone(),
            instance.blackhole_source,
            instance.ports.control,
            handle.clone(),
            blackholes,
        ),
        #[cfg(target_os = "linux")]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            SharedInstanceRpcCompat::abstract_unix_with_blackholes(
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
    .bind()
    .await
    .map_err(SharedInstanceActivationError::from_control)?;
    handle.supervise(server);
    tokio::spawn(rpc.run());
    Ok(())
}

impl SharedInstanceActivationError {
    fn from_bus(error: LocalServerBindError) -> Self {
        match error {
            LocalServerBindError::Tcp(kind) => Self {
                endpoint: SharedInstanceEndpoint::TcpBus,
                kind,
            },
            #[cfg(target_os = "linux")]
            LocalServerBindError::AbstractUnix(kind) => Self {
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
            #[cfg(target_os = "linux")]
            SharedInstanceRpcBindError::AbstractUnix(kind) => Self {
                endpoint: SharedInstanceEndpoint::AbstractUnixControl,
                kind,
            },
        }
    }
}

/// Dial the bus over TCP: `Some(stream)` if an instance is listening (the stream becomes the client
/// link), `None` if nothing answers within [`PROBE_TIMEOUT`].
async fn probe_tcp(addr: &str) -> Option<TcpStream> {
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Some(stream)
        }
        _ => None,
    }
}

/// Dial the Linux abstract AF_UNIX data bus `\0rns/{socket_path}` an instance binds (the leading null
/// is implied), mirroring how the server side binds it. `None` if nothing is listening there — the
/// connect fails fast on a missing abstract socket, so it adds no latency.
#[cfg(target_os = "linux")]
fn connect_abstract_bus(socket_path: &str) -> Option<tokio::net::UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let stream = std::os::unix::net::UnixStream::connect_addr(&addr).ok()?;
    stream.set_nonblocking(true).ok()?;
    tokio::net::UnixStream::from_std(stream).ok()
}
