//! Participating in the host's local Reticulum shared instance — the bus stock RNS apps (Sideband,
//! NomadNet, MeshChat) and other Prns nodes on the same machine share so one process owns the radios
//! and the rest ride them. [`join_shared_instance`] elects the node's role: become the instance if
//! none is running, else join the running one as a client, the parity behavior a stock RNS app
//! follows. A host-only concept (one OS, many processes), so it lives beside the bus and control-RPC
//! pieces it builds on.

use std::time::Duration;

use prns_core::interfaces::shared_instance::core as instance_core;
use prns_core::interfaces::EffectiveInterfacePolicy;
use prns_runtime::runtime::TokioPrnsHandle;
use tokio::net::TcpStream;

use super::blackhole_compat::{RnsLocalBlackholeFile, RnsPersistedBlackholes};
use super::rpc_compat::{SharedInstanceCredentials, SharedInstanceRpcCompat};
use super::server::{LocalClientInterface, LocalServer, LocalServerBindError};

/// How long the probe waits for the bus to answer before concluding no instance is running.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// How a node should participate in the host's local shared instance.
pub struct SharedInstanceIntent {
    pub credentials: SharedInstanceCredentials,
    pub blackhole_file: RnsLocalBlackholeFile,
    /// The bus and control ports (RNS defaults 37428 / 37429).
    pub ports: InstancePorts,
    pub transport: SharedInstanceTransport,
    pub policy: EffectiveInterfacePolicy,
    /// What to do when an instance is already running.
    pub on_existing: OnExisting,
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
    InstanceBusUnavailable {
        endpoint: SharedInstanceEndpoint,
        kind: std::io::ErrorKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceEndpoint {
    TcpBus,
    #[cfg(target_os = "linux")]
    AbstractUnixBus,
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
    handle: &TokioPrnsHandle,
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
            let (endpoint, kind) = match error {
                LocalServerBindError::Tcp(kind) => (SharedInstanceEndpoint::TcpBus, kind),
                #[cfg(target_os = "linux")]
                LocalServerBindError::AbstractUnix(kind) => {
                    (SharedInstanceEndpoint::AbstractUnixBus, kind)
                }
            };
            Err(JoinError::InstanceBusUnavailable { endpoint, kind })
        }
    }
}

async fn join_existing(
    handle: &TokioPrnsHandle,
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
    handle: &TokioPrnsHandle,
    stream: S,
    at: String,
    on_existing: OnExisting,
    policy: EffectiveInterfacePolicy,
) -> Result<Role, JoinError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match on_existing {
        OnExisting::JoinAsClient => {
            handle.add_interface(LocalClientInterface::with_policy(
                at.clone().into_bytes(),
                stream,
                policy,
            ));
            Ok(Role::JoinedAsClient { of: at })
        }
        OnExisting::Refuse => Err(JoinError::InstanceAlreadyRunning { at }),
    }
}

async fn become_instance(
    handle: &TokioPrnsHandle,
    instance: &SharedInstanceIntent,
) -> Result<(), LocalServerBindError> {
    let server = match &instance.transport {
        SharedInstanceTransport::Tcp => LocalServer::with_port(instance.ports.bus),
        #[cfg(target_os = "linux")]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            LocalServer::abstract_unix(socket_path.clone())
        }
    }
    .with_policy(instance.policy)
    .bind()
    .await?;
    handle.supervise(server);
    let blackholes = RnsPersistedBlackholes::new(
        handle.clone(),
        instance.credentials.transport_identity_hash,
        instance.blackhole_file.clone(),
    );
    match &instance.transport {
        SharedInstanceTransport::Tcp => {
            tokio::spawn(
                SharedInstanceRpcCompat::tcp_with_blackholes(
                    instance.credentials.clone(),
                    instance.ports.control,
                    handle.clone(),
                    blackholes,
                )
                .run(),
            );
        }
        #[cfg(target_os = "linux")]
        SharedInstanceTransport::AbstractUnix { socket_path } => {
            tokio::spawn(
                SharedInstanceRpcCompat::abstract_unix_with_blackholes(
                    instance.credentials.clone(),
                    socket_path,
                    handle.clone(),
                    blackholes,
                )
                .run(),
            );
        }
    }
    Ok(())
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
