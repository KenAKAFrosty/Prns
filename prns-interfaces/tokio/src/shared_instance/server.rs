use std::string::String;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;

use crate::framed_stream;
use prns_core::interfaces::shared_instance::core;
use prns_core::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use prns_core::reactor::airtime::AirtimeLedger;
use prns_core::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

/// One app connected to our shared instance: the server-spawned side of RNS
/// `LocalClientInterface`, a distinct engine interface over an already-accepted loopback or
/// AF_UNIX stream speaking the same HDLC framing as TCP. Never reconnects (a vanished app is
/// just gone); generic over the stream so one body serves both a `TcpStream` and a `UnixStream`.
pub struct LocalClientInterface<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    status: TokioInterfaceStatus,
}

impl<S> LocalClientInterface<S> {
    /// Wrap an accepted connection. `channel_tag` uniquely tags this connection within the
    /// local-client medium — the peer's `ip:port` for loopback TCP, or a per-connection counter for
    /// AF_UNIX. The supervisor owes its uniqueness across concurrent clients; the attach path
    /// rejects a live collision loudly.
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
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

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
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
            { core::FRAME_CAP },
            { core::FRAMED_LEN },
        >::new();
        framed_stream::serve::<
            framed_stream::HdlcFraming,
            { core::READ_BUF_LEN },
            { core::FRAME_CAP },
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
                bitrate_bps: Some(core::LOCAL_BITRATE_BPS),
                started,
            },
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

/// The shared-instance listener, RNS `LocalServerInterface`. The node that owns the local bus
/// binds the local endpoints; every other RNS app on the host that fails to bind them connects
/// here, and this supervisor stands up one [`LocalClientInterface`] per connection. It owns no
/// wire of its own; members deregister themselves as their run futures end, and teardown cascades.
///
/// It listens on the loopback TCP port everywhere, and on Linux *also* on the abstract AF_UNIX
/// socket `\0rns/{socket_path}`, because a default-config Linux app prefers that socket while
/// one configured `shared_instance_type = tcp` uses the port; binding both serves either (RNS's own `use_af_unix` is Linux/Android only).
pub struct LocalServer {
    channel_tag: Vec<u8>,
    bind_addr: String,
    #[cfg(target_os = "linux")]
    socket_path: Option<String>,
}

impl LocalServer {
    /// Bind RNS's default shared-instance endpoints: the loopback port (`127.0.0.1:37428`) and, on
    /// Linux, the default abstract socket (`\0rns/default`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(core::DEFAULT_LOCAL_PORT)
    }

    /// Bind a chosen loopback port — a second instance, or a test on an ephemeral port. Keeps the
    /// default abstract socket on Linux.
    #[must_use]
    pub fn with_port(port: u16) -> Self {
        let bind_addr = std::format!("127.0.0.1:{port}");
        Self {
            channel_tag: bind_addr.clone().into_bytes(),
            bind_addr,
            #[cfg(target_os = "linux")]
            socket_path: Some(core::DEFAULT_SOCKET_PATH.into()),
        }
    }

    /// Set the AF_UNIX `socket_path` to match an app configured with a non-default `local_socket_path`
    /// (the abstract socket bound becomes `\0rns/{socket_path}`). Linux only.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn with_socket_path(mut self, socket_path: impl Into<String>) -> Self {
        self.socket_path = Some(socket_path.into());
        self
    }

    /// This supervisor's id — `LocalServer`-kind, derived from its bind address. The members it
    /// stands up are `LocalClient`-kind, a distinct id each.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LocalServer, &self.channel_tag)
    }
}

impl Default for LocalServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind the abstract AF_UNIX socket `\0rns/{socket_path}` (Linux's abstract namespace, where the
/// leading null is implied by [`from_abstract_name`](std::os::linux::net::SocketAddrExt)). `None` if
/// the name is taken (another shared instance already holds it) — the supervisor then serves TCP
/// only, exactly as a client that fails to bind falls through to connecting.
#[cfg(target_os = "linux")]
fn bind_abstract_unix(socket_path: &str) -> Option<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    listener.set_nonblocking(true).ok()?;
    UnixListener::from_std(listener).ok()
}

impl InterfaceSupervisor for LocalServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run(self, fleet: Fleet) {
        let tcp = async {
            let Ok(listener) = TcpListener::bind(self.bind_addr.as_str()).await else {
                return;
            };
            loop {
                if let Ok((stream, peer)) = listener.accept().await {
                    let _ = stream.set_nodelay(true);
                    let _ = fleet.add(LocalClientInterface::new(
                        peer.to_string().into_bytes(),
                        stream,
                    ));
                }
            }
        };

        #[cfg(target_os = "linux")]
        {
            let unix = async {
                let Some(socket_path) = self.socket_path.as_deref() else {
                    return;
                };
                let Some(listener) = bind_abstract_unix(socket_path) else {
                    return;
                };
                let mut nth: u64 = 0;
                loop {
                    if let Ok((stream, _peer)) = listener.accept().await {
                        nth = nth.wrapping_add(1);
                        let tag = std::format!("unix:{socket_path}#{nth}").into_bytes();
                        let _ = fleet.add(LocalClientInterface::new(tag, stream));
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

impl prns_core::interfaces::ReportsStatus for LocalServer {}

impl<S> prns_core::interfaces::ReportsStatus for LocalClientInterface<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
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
        assert_eq!(descriptor.bitrate_bps, Some(core::LOCAL_BITRATE_BPS));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_server_defaults_to_the_default_abstract_socket() {
        assert_eq!(
            LocalServer::new().socket_path.as_deref(),
            Some(core::DEFAULT_SOCKET_PATH)
        );
        assert_eq!(
            LocalServer::new()
                .with_socket_path("custom")
                .socket_path
                .as_deref(),
            Some("custom")
        );
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
}
