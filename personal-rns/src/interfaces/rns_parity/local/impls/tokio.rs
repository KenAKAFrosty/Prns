use std::string::String;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::interfaces::framed_stream;
use crate::interfaces::rns_parity::local::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;
use crate::runtime::{Fleet, InterfaceSupervisor};

/// One app connected to our shared instance — the server-spawned side of RNS
/// `LocalClientInterface`. A distinct engine interface over an already-accepted loopback or AF_UNIX
/// stream, speaking the same HDLC framing as TCP. The [`LocalServer`] supervisor stands one up per
/// connection and drops it when the stream closes; unlike the TCP client this end
/// never reconnects (a vanished app is just gone). Generic over the stream so one body serves both a
/// `TcpStream` and a `UnixStream`.
pub struct LocalClientInterface<S> {
    id: InterfaceId,
    reachability_tag: Vec<u8>,
    stream: Option<S>,
    status: TokioInterfaceStatus,
}

impl<S> LocalClientInterface<S> {
    /// Wrap an accepted connection. `reachability_tag` uniquely tags this connection within the
    /// local-client medium — the peer's `ip:port` for loopback TCP, or a per-connection counter for
    /// AF_UNIX. The supervisor owes its uniqueness across concurrent clients; the attach path
    /// rejects a live collision loudly.
    #[must_use]
    pub fn new(reachability_tag: Vec<u8>, stream: S) -> Self {
        let id = InterfaceId::from_reachability_tag(InterfaceKind::LocalClient, &reachability_tag);
        Self {
            id,
            reachability_tag,
            stream: Some(stream),
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
        }
    }

    /// This interface's id, minted from the reachability tag. The supervisor holds it to deregister
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

    fn reachability_tag(&self) -> &[u8] {
        &self.reachability_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        framed_stream::serve::<
            { core::READ_BUF_LEN },
            { core::FRAME_CAP },
            { core::FRAMED_LEN },
            _,
            _,
        >(
            stream,
            &mut seam,
            &self.status,
            &mut airtime,
            &mut throughput,
            Some(core::LOCAL_BITRATE_BPS),
            started,
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

/// The shared-instance listener — RNS `LocalServerInterface`. The node that owns the local bus (the
/// Hopspot daemon) binds a loopback port; every other RNS app on the host that fails to bind it
/// connects here instead, and this supervisor stands up one [`LocalClientInterface`] per connection.
/// It owns no wire of its own (RNS's `LocalServerInterface.process_outgoing` is a no-op); its members
/// carry the traffic, each a distinct engine interface. A member whose connection drops deregisters
/// itself — its run future ends and the driver culls it — so the supervisor accepts and forgets, and
/// a supervisor teardown cascades to every member. (AF_UNIX, which RNS uses only on Linux, follows.)
pub struct LocalServer {
    reachability_tag: Vec<u8>,
    bind_addr: String,
}

impl LocalServer {
    /// Bind RNS's default shared-instance port on loopback (`127.0.0.1:37428`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_port(core::DEFAULT_LOCAL_PORT)
    }

    /// Bind a chosen loopback port — a second instance, or a test on an ephemeral port.
    #[must_use]
    pub fn with_port(port: u16) -> Self {
        let bind_addr = std::format!("127.0.0.1:{port}");
        Self {
            reachability_tag: bind_addr.clone().into_bytes(),
            bind_addr,
        }
    }

    /// This supervisor's id — `LocalServer`-kind, derived from its bind address. The members it
    /// stands up are `LocalClient`-kind, a distinct id each.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_reachability_tag(InterfaceKind::LocalServer, &self.reachability_tag)
    }
}

impl Default for LocalServer {
    fn default() -> Self {
        Self::new()
    }
}

impl InterfaceSupervisor for LocalServer {
    const KIND: InterfaceKind = InterfaceKind::LocalServer;

    fn reachability_tag(&self) -> &[u8] {
        &self.reachability_tag
    }

    async fn run(self, fleet: Fleet) {
        let Ok(listener) = TcpListener::bind(self.bind_addr.as_str()).await else {
            return;
        };
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let _ = fleet.add(LocalClientInterface::new(
                        peer.to_string().into_bytes(),
                        stream,
                    ));
                }
                Err(_) => continue,
            }
        }
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
        assert_eq!(descriptor.mode, crate::interfaces::InterfaceMode::Full);
        assert_eq!(descriptor.bitrate_bps, Some(core::LOCAL_BITRATE_BPS));
    }
}
