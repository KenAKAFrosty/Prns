use std::string::String;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
#[cfg(any(target_os = "linux", target_os = "android"))]
use tokio::net::UnixListener;

use crate::byte_stream::framing;
use prns_core::interfaces::shared_instance;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

pub struct SharedInstanceClient<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
}

impl<S> SharedInstanceClient<S> {
    /// `channel_tag` uniquely tags this connection within the local-client medium — the peer's `ip:port` for loopback TCP, or a per-connection counter for AF_UNIX.
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S) -> Self {
        Self::with_policy(
            channel_tag,
            stream,
            shared_instance::configured_policy(Default::default()),
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

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for SharedInstanceClient<S> {
    const HW_MTU: usize = shared_instance::HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::LocalClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        shared_instance::descriptor(self.id, self.policy)
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
        let mut buffers = framing::FramedBuffers::<
            framing::HdlcFraming,
            { shared_instance::READ_BUF_LEN },
            { shared_instance::FRAMED_LEN },
        >::new();
        framing::serve::<
            framing::HdlcFraming,
            { shared_instance::READ_BUF_LEN },
            { shared_instance::FRAMED_LEN },
            _,
            _,
        >(
            stream,
            &mut buffers,
            &mut seam,
            &mut framing::WireMeters {
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

pub struct SharedInstanceServer {
    channel_tag: Vec<u8>,
    bind_addr: Option<String>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    socket_path: Option<String>,
}

pub struct BoundSharedInstanceServer {
    channel_tag: Vec<u8>,
    tcp_listener: Option<TcpListener>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    abstract_unix: BoundAbstractUnix,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
enum BoundAbstractUnix {
    Disabled,
    Listening {
        socket_path: String,
        listener: UnixListener,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceServerBindError {
    Tcp(std::io::ErrorKind),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix(std::io::ErrorKind),
}

impl SharedInstanceServer {
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(shared_instance::DEFAULT_LOCAL_PORT)
    }

    #[must_use]
    pub fn with_port(port: u16) -> Self {
        let bind_addr = std::format!("127.0.0.1:{port}");
        let channel_tag = bind_addr.clone().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &channel_tag);
        Self {
            channel_tag,
            bind_addr: Some(bind_addr),
            policy: shared_instance::configured_policy(Default::default()),
            status: TokioInterfaceStatus::new(id, ConnectionState::Disconnected),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            socket_path: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn abstract_unix(socket_path: impl Into<String>) -> Self {
        let socket_path = socket_path.into();
        let channel_tag = std::format!("unix:{socket_path}").into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &channel_tag);
        Self {
            channel_tag,
            bind_addr: None,
            policy: shared_instance::configured_policy(Default::default()),
            status: TokioInterfaceStatus::new(id, ConnectionState::Disconnected),
            socket_path: Some(socket_path),
        }
    }

    #[must_use]
    pub fn with_policy(mut self, policy: EffectiveInterfacePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set the AF_UNIX `socket_path` to match an app configured with a non-default `local_socket_path` (the abstract socket bound becomes `\0rns/{socket_path}`). Linux only. Replaces TCP; use [`Self::also_listen_on_abstract_unix`] to keep loopback TCP.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn with_socket_path(mut self, socket_path: impl Into<String>) -> Self {
        let socket_path = socket_path.into();
        self.channel_tag = std::format!("unix:{socket_path}").into_bytes();
        self.status = TokioInterfaceStatus::new(self.id(), ConnectionState::Disconnected);
        self.bind_addr = None;
        self.socket_path = Some(socket_path);
        self
    }

    /// Also bind `\0rns/{socket_path}` (stock RNS `instance_name`, default `default`) without dropping the TCP loopback listener.
    ///
    /// Android Sideband's baked config uses AF_UNIX, not `shared_instance_type = tcp`. If the Unix name is already taken, [`Self::bind`] still keeps TCP.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn also_listen_on_abstract_unix(mut self, socket_path: impl Into<String>) -> Self {
        self.socket_path = Some(socket_path.into());
        self
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &self.channel_tag)
    }

    pub async fn bind(self) -> Result<BoundSharedInstanceServer, SharedInstanceServerBindError> {
        let tcp_listener = match self.bind_addr {
            Some(bind_addr) => Some(
                TcpListener::bind(bind_addr.as_str())
                    .await
                    .map_err(|error| SharedInstanceServerBindError::Tcp(error.kind()))?,
            ),
            None => None,
        };
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let tcp_also_bound = tcp_listener.is_some();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let abstract_unix = match self.socket_path {
            Some(socket_path) => match bind_abstract_unix(&socket_path) {
                Ok(listener) => BoundAbstractUnix::Listening {
                    socket_path,
                    listener,
                },
                Err(_) if tcp_also_bound => BoundAbstractUnix::Disabled,
                Err(error) => {
                    return Err(SharedInstanceServerBindError::AbstractUnix(error.kind()));
                }
            },
            None => BoundAbstractUnix::Disabled,
        };
        self.status.set_connection(ConnectionState::Connected);
        Ok(BoundSharedInstanceServer {
            channel_tag: self.channel_tag,
            tcp_listener,
            policy: self.policy,
            status: self.status,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            abstract_unix,
        })
    }
}

impl Default for SharedInstanceServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind the abstract AF_UNIX socket `\0rns/{socket_path}` (Linux's abstract namespace, where the leading null is implied by [`from_abstract_name`](std::os::linux::net::SocketAddrExt)).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_abstract_unix(socket_path: &str) -> std::io::Result<UnixListener> {
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr)?;
    listener.set_nonblocking(true)?;
    UnixListener::from_std(listener)
}

impl InterfaceSupervisor for SharedInstanceServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let Ok(server) = self.bind().await else {
            return;
        };
        server.run(fleet).await;
    }
}

impl BoundSharedInstanceServer {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[must_use]
    pub fn listens_on_abstract_unix(&self) -> bool {
        matches!(self.abstract_unix, BoundAbstractUnix::Listening { .. })
    }
}

impl InterfaceSupervisor for BoundSharedInstanceServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
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
                    let _ = fleet.add(SharedInstanceClient::with_policy(
                        peer.to_string().into_bytes(),
                        stream,
                        policy,
                    ));
                }
            }
        };

        #[cfg(any(target_os = "linux", target_os = "android"))]
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
                        let _ = fleet.add(SharedInstanceClient::with_policy(tag, stream, policy));
                    }
                }
            };
            tokio::join!(tcp, unix);
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            tcp.await;
        }
    }
}

impl prns_core::interfaces::ReportsStatus for SharedInstanceServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl prns_core::interfaces::ReportsStatus for BoundSharedInstanceServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> prns_core::interfaces::ReportsStatus for SharedInstanceClient<S> {
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

    fn member(tag: &[u8]) -> SharedInstanceClient<tokio::io::DuplexStream> {
        let (near, _far) = tokio::io::duplex(64);
        SharedInstanceClient::new(tag.to_vec(), near)
    }

    #[test]
    fn the_listener_publishes_its_stable_supervisor_identity() {
        let server = SharedInstanceServer::with_port(0);
        let view = prns_core::interfaces::ReportsStatus::status_view(&server).unwrap();
        let vitals = view();
        assert_eq!(vitals.len(), 1);
        assert_eq!(vitals[0].id, server.id());
        assert_eq!(vitals[0].connection, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn a_bound_listener_reports_online() {
        let server = SharedInstanceServer::with_port(0).bind().await.unwrap();
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
        assert_eq!(descriptor.bitrate, shared_instance::LOCAL_BITRATE_BPS);
        assert_eq!(descriptor.hardware_mtu, Some(524_288));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn constructors_select_exactly_one_shared_instance_transport() {
        assert!(SharedInstanceServer::new().bind_addr.is_some());
        assert_eq!(SharedInstanceServer::new().socket_path, None);
        assert_eq!(
            SharedInstanceServer::abstract_unix(shared_instance::DEFAULT_SOCKET_PATH)
                .socket_path
                .as_deref(),
            Some(shared_instance::DEFAULT_SOCKET_PATH)
        );
        assert!(
            SharedInstanceServer::abstract_unix(shared_instance::DEFAULT_SOCKET_PATH)
                .bind_addr
                .is_none()
        );
        let both = SharedInstanceServer::with_port(37_428)
            .also_listen_on_abstract_unix(shared_instance::DEFAULT_SOCKET_PATH);
        assert!(both.bind_addr.is_some());
        assert_eq!(
            both.socket_path.as_deref(),
            Some(shared_instance::DEFAULT_SOCKET_PATH)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn tcp_and_abstract_unix_can_bind_together() {
        #[cfg(target_os = "android")]
        use std::os::android::net::SocketAddrExt;
        #[cfg(target_os = "linux")]
        use std::os::linux::net::SocketAddrExt;
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port binds");
        let port = probe.local_addr().expect("the probe has an address").port();
        drop(probe);
        let socket_path = std::format!("personal-test-dual-{port}");
        let server = SharedInstanceServer::with_port(port)
            .also_listen_on_abstract_unix(&socket_path)
            .bind()
            .await
            .expect("TCP and Unix bind together");
        assert!(server.listens_on_abstract_unix());
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok());
        let name = std::format!("rns/{socket_path}");
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
            .expect("the abstract name is valid");
        assert!(std::os::unix::net::UnixStream::connect_addr(&addr).is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn a_taken_unix_name_does_not_drop_tcp_when_both_were_requested() {
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port binds");
        let port = probe.local_addr().expect("the probe has an address").port();
        drop(probe);
        let socket_path = std::format!("personal-test-unix-taken-{port}");
        let _holder = bind_abstract_unix(&socket_path).expect("the abstract socket binds");
        let server = SharedInstanceServer::with_port(port)
            .also_listen_on_abstract_unix(&socket_path)
            .bind()
            .await
            .expect("TCP still binds when Unix is taken");
        assert!(!server.listens_on_abstract_unix());
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn the_abstract_socket_binds_and_accepts_a_connection() {
        #[cfg(target_os = "android")]
        use std::os::android::net::SocketAddrExt;
        #[cfg(target_os = "linux")]
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn binding_reserves_only_the_selected_endpoint_before_the_supervisor_runs() {
        #[cfg(target_os = "android")]
        use std::os::android::net::SocketAddrExt;
        #[cfg(target_os = "linux")]
        use std::os::linux::net::SocketAddrExt;
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port binds");
        let port = probe.local_addr().expect("the probe has an address").port();
        drop(probe);
        let socket_path = std::format!("personal-test-prebound-{port}");
        let _server = SharedInstanceServer::abstract_unix(&socket_path)
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
