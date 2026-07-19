use std::string::String;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;

use crate::framed_stream;
use prns_core::interfaces::shared_instance::core;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::reactor::airtime::AirtimeLedger;
use prns_runtime::reactor::driver::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::reactor::throughput::ThroughputLedger;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

/// One app connected to our shared instance: the server-spawned side of RNS
/// `LocalClientInterface`, a distinct engine interface over an already-accepted loopback or
/// AF_UNIX stream speaking the same HDLC framing as TCP. Never reconnects (a vanished app is
/// just gone); generic over the stream so one body serves both a `TcpStream` and a `UnixStream`.
pub struct LocalClientInterface<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
}

impl<S> LocalClientInterface<S> {
    /// Wrap an accepted connection. `channel_tag` uniquely tags this connection within the
    /// local-client medium — the peer's `ip:port` for loopback TCP, or a per-connection counter for
    /// AF_UNIX. The supervisor owes its uniqueness across concurrent clients; the attach path
    /// rejects a live collision loudly.
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S) -> Self {
        Self::with_policy(
            channel_tag,
            stream,
            core::configured_policy(Default::default()),
        )
    }

    #[must_use]
    pub fn with_policy(channel_tag: Vec<u8>, stream: S, policy: EffectiveInterfacePolicy) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            policy,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
        }
    }

    /// This interface's id, minted from the channel tag. The supervisor holds it to deregister
    /// the member when the connection drops.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this member's live-status handle, for a face to render the connected app beside
    /// the aggregate. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for LocalClientInterface<S> {
    const HW_MTU: usize = core::HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::LocalClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers = framed_stream::FramedBuffers::<
            framed_stream::HdlcFraming,
            { core::READ_BUF_LEN },
            { core::FRAMED_LEN },
        >::new();
        framed_stream::serve::<
            framed_stream::HdlcFraming,
            { core::READ_BUF_LEN },
            { core::FRAMED_LEN },
            _,
            _,
        >(
            stream,
            &mut buffers,
            &mut seam,
            &mut framed_stream::WireMeters {
                status: &self.status,
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: self.policy.bitrate,
                started,
            },
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

/// The shared-instance listener, RNS `LocalServerInterface`. The node that owns the local bus
/// binds the selected local endpoint; every other RNS app on the host that fails to bind it connects
/// here, and this supervisor stands up one [`LocalClientInterface`] per connection. It owns no
/// wire of its own; members deregister themselves as their run futures end, and teardown cascades.
///
/// It listens on either the loopback TCP port or, on Linux, the selected abstract AF_UNIX socket
/// `\0rns/{socket_path}`.
pub struct LocalServer {
    channel_tag: Vec<u8>,
    bind_addr: Option<String>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    #[cfg(target_os = "linux")]
    socket_path: Option<String>,
}

pub(crate) struct BoundLocalServer {
    channel_tag: Vec<u8>,
    tcp_listener: Option<TcpListener>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    #[cfg(target_os = "linux")]
    abstract_unix: BoundAbstractUnix,
}

#[cfg(target_os = "linux")]
enum BoundAbstractUnix {
    Disabled,
    Listening {
        socket_path: String,
        listener: UnixListener,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalServerBindError {
    Tcp(std::io::ErrorKind),
    #[cfg(target_os = "linux")]
    AbstractUnix(std::io::ErrorKind),
}

impl LocalServer {
    /// Bind RNS's default loopback TCP shared-instance endpoint (`127.0.0.1:37428`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(core::DEFAULT_LOCAL_PORT)
    }

    /// Bind a chosen loopback port — a second instance, or a test on an ephemeral port.
    #[must_use]
    pub fn with_port(port: u16) -> Self {
        let bind_addr = std::format!("127.0.0.1:{port}");
        let channel_tag = bind_addr.clone().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &channel_tag);
        Self {
            channel_tag,
            bind_addr: Some(bind_addr),
            policy: core::configured_policy(Default::default()),
            status: TokioInterfaceStatus::new(id, ConnectionState::Disconnected),
            #[cfg(target_os = "linux")]
            socket_path: None,
        }
    }

    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn abstract_unix(socket_path: impl Into<String>) -> Self {
        let socket_path = socket_path.into();
        let channel_tag = std::format!("unix:{socket_path}").into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &channel_tag);
        Self {
            channel_tag,
            bind_addr: None,
            policy: core::configured_policy(Default::default()),
            status: TokioInterfaceStatus::new(id, ConnectionState::Disconnected),
            socket_path: Some(socket_path),
        }
    }

    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set the AF_UNIX `socket_path` to match an app configured with a non-default `local_socket_path`
    /// (the abstract socket bound becomes `\0rns/{socket_path}`). Linux only.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn with_socket_path(mut self, socket_path: impl Into<String>) -> Self {
        let socket_path = socket_path.into();
        self.channel_tag = std::format!("unix:{socket_path}").into_bytes();
        self.status = TokioInterfaceStatus::new(self.id(), ConnectionState::Disconnected);
        self.bind_addr = None;
        self.socket_path = Some(socket_path);
        self
    }

    /// This supervisor's id — `LocalServer`-kind, derived from its bind address. The members it
    /// stands up are `LocalClient`-kind, a distinct id each.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &self.channel_tag)
    }

    pub(crate) async fn bind(self) -> Result<BoundLocalServer, LocalServerBindError> {
        let tcp_listener = match self.bind_addr {
            Some(bind_addr) => Some(
                TcpListener::bind(bind_addr.as_str())
                    .await
                    .map_err(|error| LocalServerBindError::Tcp(error.kind()))?,
            ),
            None => None,
        };
        #[cfg(target_os = "linux")]
        let abstract_unix = match self.socket_path {
            Some(socket_path) => {
                let listener = bind_abstract_unix(&socket_path)
                    .map_err(|error| LocalServerBindError::AbstractUnix(error.kind()))?;
                BoundAbstractUnix::Listening {
                    socket_path,
                    listener,
                }
            }
            None => BoundAbstractUnix::Disabled,
        };
        self.status.set_connection(ConnectionState::Connected);
        Ok(BoundLocalServer {
            channel_tag: self.channel_tag,
            tcp_listener,
            policy: self.policy,
            status: self.status,
            #[cfg(target_os = "linux")]
            abstract_unix,
        })
    }
}

impl Default for LocalServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind the abstract AF_UNIX socket `\0rns/{socket_path}` (Linux's abstract namespace, where the
/// leading null is implied by [`from_abstract_name`](std::os::linux::net::SocketAddrExt)).
#[cfg(target_os = "linux")]
fn bind_abstract_unix(socket_path: &str) -> std::io::Result<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr)?;
    listener.set_nonblocking(true)?;
    UnixListener::from_std(listener)
}

impl InterfaceSupervisor for LocalServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run(self, fleet: Fleet) {
        let Ok(server) = self.bind().await else {
            return;
        };
        server.run(fleet).await;
    }
}

impl InterfaceSupervisor for BoundLocalServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run(self, fleet: Fleet) {
        let tcp_listener = self.tcp_listener;
        let policy = self.policy;
        let tcp = async {
            let Some(tcp_listener) = tcp_listener else {
                return;
            };
            loop {
                if let Ok((stream, peer)) = tcp_listener.accept().await {
                    let _ = stream.set_nodelay(true);
                    let _ = fleet.add(LocalClientInterface::with_policy(
                        peer.to_string().into_bytes(),
                        stream,
                        policy,
                    ));
                }
            }
        };

        #[cfg(target_os = "linux")]
        {
            let unix = async {
                let BoundAbstractUnix::Listening {
                    socket_path,
                    listener,
                } = self.abstract_unix
                else {
                    return;
                };
                let mut nth: u64 = 0;
                loop {
                    if let Ok((stream, _peer)) = listener.accept().await {
                        nth = nth.wrapping_add(1);
                        let tag = std::format!("unix:{socket_path}#{nth}").into_bytes();
                        let _ = fleet.add(LocalClientInterface::with_policy(tag, stream, policy));
                    }
                }
            };
            tokio::join!(tcp, unix);
        }
        #[cfg(not(target_os = "linux"))]
        {
            tcp.await;
        }
    }
}

impl prns_core::interfaces::ReportsStatus for LocalServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl prns_core::interfaces::ReportsStatus for BoundLocalServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> prns_core::interfaces::ReportsStatus for LocalClientInterface<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(tag: &[u8]) -> LocalClientInterface<tokio::io::DuplexStream> {
        let (near, _far) = tokio::io::duplex(64);
        LocalClientInterface::new(tag.to_vec(), near)
    }

    #[test]
    fn the_listener_publishes_its_stable_supervisor_identity() {
        let server = LocalServer::with_port(0);
        let view = prns_core::interfaces::ReportsStatus::status_view(&server).unwrap();
        let vitals = view();
        assert_eq!(vitals.len(), 1);
        assert_eq!(vitals[0].id, server.id());
        assert_eq!(vitals[0].connection, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn a_bound_listener_reports_online() {
        let server = LocalServer::with_port(0).bind().await.unwrap();
        let view = prns_core::interfaces::ReportsStatus::status_view(&server).unwrap();
        assert_eq!(view()[0].connection, ConnectionState::Connected);
    }

    #[test]
    fn the_id_is_a_local_client_kind_from_the_tag() {
        let iface = member(b"127.0.0.1:54321");
        assert_eq!(iface.id().kind(), Some(InterfaceKind::LocalClient));
        let same = member(b"127.0.0.1:54321");
        assert_eq!(iface.id(), same.id());
        let other = member(b"127.0.0.1:54322");
        assert_ne!(iface.id(), other.id());
    }

    #[test]
    fn the_descriptor_is_full_participation_at_local_bitrate() {
        let iface = member(b"app-1");
        let descriptor = iface.descriptor();
        assert_eq!(descriptor.id, iface.id());
        assert_eq!(descriptor.mode, prns_core::interfaces::InterfaceMode::Full);
        assert_eq!(descriptor.bitrate, core::LOCAL_BITRATE_BPS);
        assert_eq!(descriptor.hardware_mtu, Some(524_288));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn constructors_select_exactly_one_shared_instance_transport() {
        assert!(LocalServer::new().bind_addr.is_some());
        assert_eq!(LocalServer::new().socket_path, None);
        assert_eq!(
            LocalServer::abstract_unix(core::DEFAULT_SOCKET_PATH)
                .socket_path
                .as_deref(),
            Some(core::DEFAULT_SOCKET_PATH)
        );
        assert!(LocalServer::abstract_unix(core::DEFAULT_SOCKET_PATH)
            .bind_addr
            .is_none());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_abstract_socket_binds_and_accepts_a_connection() {
        use std::os::linux::net::SocketAddrExt;
        let listener =
            bind_abstract_unix("personal-test-stage3b").expect("the abstract socket binds");
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(b"rns/personal-test-stage3b")
            .expect("the abstract name is valid");
        let std_client = std::os::unix::net::UnixStream::connect_addr(&addr)
            .expect("a client connects to the bound abstract socket");
        std_client
            .set_nonblocking(true)
            .expect("the client stream goes nonblocking");
        let _client =
            tokio::net::UnixStream::from_std(std_client).expect("tokio adopts the std stream");
        let (_accepted, _peer) = listener
            .accept()
            .await
            .expect("the server accepts the abstract-socket connection");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn binding_reserves_only_the_selected_endpoint_before_the_supervisor_runs() {
        use std::os::linux::net::SocketAddrExt;
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port binds");
        let port = probe.local_addr().expect("the probe has an address").port();
        drop(probe);
        let socket_path = std::format!("personal-test-prebound-{port}");
        let _server = LocalServer::abstract_unix(&socket_path)
            .bind()
            .await
            .expect("the server binds the selected Unix endpoint");

        assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok());
        let name = std::format!("rns/{socket_path}");
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
            .expect("the abstract name is valid");
        assert!(std::os::unix::net::UnixStream::connect_addr(&addr).is_ok());
    }
}
