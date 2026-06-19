//! Participating in the host's local Reticulum shared instance — the bus stock RNS apps (Sideband,
//! NomadNet, MeshChat) and other Prns nodes on the same machine share so one process owns the radios
//! and the rest ride them. [`join_local_instance`](TokioPrnsHandle::join_local_instance) elects the
//! node's role: become the instance if none is running, else join the running one as a client, the
//! parity behavior a stock RNS app follows. A host-only concept (one OS, many processes), so the
//! whole module rides the `local` feature, like the bus and control-RPC pieces it builds on.

use std::path::PathBuf;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::interfaces::rns_parity::local::core as local_core;
use crate::interfaces::rns_parity::local::impls::rpc_compat::{
    rpc_key_from_rns_identity, SharedInstanceRpcCompat,
};
use crate::interfaces::rns_parity::local::impls::tokio::{LocalClientInterface, LocalServer};

use super::TokioPrnsHandle;

/// How long the probe waits for the bus to answer before concluding no instance is running.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// How a node should participate in the host's local shared instance.
pub struct LocalInstance {
    /// The Reticulum config directory whose `storage/transport_identity` keys the control RPC
    /// (`rpc_key = sha256(transport_identity)`), so a stock client derives the same key. The caller
    /// owns the identity file (the node was built from the same secret); this only reads it.
    pub identity_dir: PathBuf,
    /// The bus and control ports (RNS defaults 37428 / 37429).
    pub ports: InstancePorts,
    /// What to do when an instance is already running.
    pub on_existing: OnExisting,
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
            bus: local_core::DEFAULT_LOCAL_PORT,
            control: local_core::DEFAULT_LOCAL_PORT + 1,
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

/// Why [`join_local_instance`](TokioPrnsHandle::join_local_instance) took no role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// An instance was already running at `at`, and [`OnExisting::Refuse`] forbade joining it.
    InstanceAlreadyRunning { at: String },
}

impl TokioPrnsHandle {
    /// Participate in the host's local Reticulum shared instance, electing the node's role.
    ///
    /// Probe the bus port: if an instance answers, this node defers to it and joins as a client over
    /// its bus (or, under [`OnExisting::Refuse`], declines) — the honorable parity behavior, the same
    /// a stock RNS app follows when it finds a running instance. If nothing answers, the node becomes
    /// the instance: it supervises the bus for local apps (Sideband, NomadNet, MeshChat, other Prns
    /// nodes) and serves the control RPC keyed on its identity, rendering the live interface fleet
    /// straight from the runtime's own status registry — no app-side bookkeeping.
    ///
    /// A client only attaches the bus link; it does not stand up its own interfaces, the bus, or the
    /// RPC — the instance owns those. The caller decides the rest from the returned [`Role`].
    ///
    /// The probe covers both transports an instance may bind: TCP everywhere, and — since a
    /// default-config Linux app prefers the abstract AF_UNIX socket `\0rns/{name}` over TCP — that
    /// socket too on Linux, so an instance reachable only there is still found rather than collided
    /// with.
    pub async fn join_local_instance(&self, instance: LocalInstance) -> Result<Role, JoinError> {
        let bus_addr = std::format!("127.0.0.1:{}", instance.ports.bus);
        if let Some(stream) = probe_tcp(&bus_addr).await {
            let at = stream
                .peer_addr()
                .map(|addr| addr.to_string())
                .unwrap_or(bus_addr);
            return self.join_or_refuse(stream, at, instance.on_existing);
        }
        #[cfg(target_os = "linux")]
        if let Some(stream) = connect_abstract_bus(local_core::DEFAULT_SOCKET_PATH) {
            let at = std::format!("\\0rns/{}", local_core::DEFAULT_SOCKET_PATH);
            return self.join_or_refuse(stream, at, instance.on_existing);
        }
        self.become_instance(&instance);
        Ok(Role::BecameInstance)
    }

    fn join_or_refuse<S>(
        &self,
        stream: S,
        at: String,
        on_existing: OnExisting,
    ) -> Result<Role, JoinError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        match on_existing {
            OnExisting::JoinAsClient => {
                self.add_interface(LocalClientInterface::new(at.clone().into_bytes(), stream));
                Ok(Role::JoinedAsClient { of: at })
            }
            OnExisting::Refuse => Err(JoinError::InstanceAlreadyRunning { at }),
        }
    }

    fn become_instance(&self, instance: &LocalInstance) {
        self.supervise(LocalServer::with_port(instance.ports.bus));
        let rpc_key = rpc_key_from_rns_identity(&instance.identity_dir.join("storage"), &[]);
        let fleet = self.clone();
        tokio::spawn(
            SharedInstanceRpcCompat::tcp(rpc_key, instance.ports.control, self.clone())
                .with_interfaces(move || fleet.interface_snapshots())
                .run(),
        );
        #[cfg(target_os = "linux")]
        {
            let fleet = self.clone();
            tokio::spawn(
                SharedInstanceRpcCompat::abstract_unix(
                    rpc_key,
                    local_core::DEFAULT_SOCKET_PATH,
                    self.clone(),
                )
                .with_interfaces(move || fleet.interface_snapshots())
                .run(),
            );
        }
    }
}

/// Dial the bus over TCP: `Some(stream)` if an instance is listening (the stream becomes the client
/// link), `None` if nothing answers within [`PROBE_TIMEOUT`].
async fn probe_tcp(addr: &str) -> Option<TcpStream> {
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => Some(stream),
        _ => None,
    }
}

/// Dial the Linux abstract AF_UNIX data bus `\0rns/{socket_path}` an instance binds (the leading null
/// is implied), mirroring how [`bind_abstract_unix`] on the server side binds it. `None` if nothing is
/// listening there — the connect fails fast on a missing abstract socket, so it adds no latency.
#[cfg(target_os = "linux")]
fn connect_abstract_bus(socket_path: &str) -> Option<tokio::net::UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let stream = std::os::unix::net::UnixStream::connect_addr(&addr).ok()?;
    stream.set_nonblocking(true).ok()?;
    tokio::net::UnixStream::from_std(stream).ok()
}
