//! The listening end of a Backbone link (`BackboneInterface` parity): bind a port and stand up
//! a distinct engine interface per client, the way the reference spawns a child
//! `BackboneClientInterface` per accepted socket. Structurally identical to the TCP server,
//! since Backbone is TCP on the wire; only the interface kinds and the bitrate guess are Backbone's own.

use std::io;
use std::net::SocketAddr;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::framed_stream;
use crate::tcp::tokio_socket::{tune, RECONNECT_WAIT};
use prns_core::interfaces::backbone::core;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind};
use prns_core::reactor::airtime::AirtimeLedger;
use prns_core::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

/// One client connected to our Backbone listener: the server-spawned side of a Backbone pair,
/// a distinct engine interface over an already-accepted stream speaking the same HDLC framing.
/// The supervisor stands one up per connection and drops it when the stream closes; this end
/// never reconnects (parity: the reference's spawned interface tears down on close).
pub struct BackboneServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    bitrate: BitrateBps,
    status: TokioInterfaceStatus,
}

impl<S> BackboneServerConnection<S> {
    /// Wrap an accepted connection. `channel_tag` uniquely tags this connection within the
    /// server-peer medium — the peer's `ip:port`. The supervisor owes its uniqueness across
    /// concurrent clients (distinct source ports give distinct tags); the attach path rejects a live
    /// collision loudly.
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S, bitrate: BitrateBps) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            bitrate,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
        }
    }

    /// This interface's id, minted from the channel tag. The supervisor lets the reactor cull it when
    /// the connection drops (the run future ends), so it never holds the id itself.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this member's live-status handle, for a face to render the connected peer beside the
    /// listener. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for BackboneServerConnection<S> {
    const HW_MTU: usize = core::HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::BackboneServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.bitrate)
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
                bitrate: self.bitrate,
                started,
            },
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

/// The listening end of a Backbone pair (`BackboneInterface` parity): a supervisor over
/// per-connection members, each a distinct engine interface. It owns no wire of its own (the
/// reference's `process_outgoing` is a no-op; the members carry the traffic); teardown
/// cascades. `bitrate` sets each member's declared MTU through the reference's tier table;
/// claim honestly ([`core::BACKBONE_BITRATE_GUESS_BPS`] when genuinely unknown).
pub struct BackboneServer {
    listener: TcpListener,
    bitrate: BitrateBps,
    channel_tag: Vec<u8>,
}

impl BackboneServer {
    pub async fn bind(
        addr: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = listener.local_addr()?.to_string().into_bytes();
        Ok(Self {
            listener,
            bitrate,
            channel_tag,
        })
    }

    /// This supervisor's id — `BackboneServer`-kind, derived from its bound address. The members it
    /// stands up are `BackboneServerPeer`-kind, a distinct id each. For the app that names the
    /// listener card.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::BackboneServer, &self.channel_tag)
    }

    /// The address the listener bound — with the OS-assigned port resolved, for a face to show and a
    /// connector to dial.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl InterfaceSupervisor for BackboneServer {
    const KIND: InterfaceKind = InterfaceKind::BackboneServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run(self, fleet: Fleet) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    tune(&stream);
                    let _ = fleet.add(BackboneServerConnection::new(
                        peer.to_string().into_bytes(),
                        stream,
                        self.bitrate,
                    ));
                }
                Err(_) => tokio::time::sleep(RECONNECT_WAIT).await,
            }
        }
    }
}

impl prns_core::interfaces::ReportsStatus for BackboneServer {}

impl<S> prns_core::interfaces::ReportsStatus for BackboneServerConnection<S> {
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
    use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a grant
    /// lane the test fills — so a member's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if !self.sink.is_empty() {
                let _ = self.inbound.send(std::mem::take(&mut self.sink));
            }
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
    }

    async fn write_framed(socket: &mut TcpStream, payload: &[u8]) {
        let mut framed = [0u8; 64];
        let n = rns_serial_framing::encode(payload, &mut framed).expect("encodes the payload");
        socket
            .write_all(&framed[..n])
            .await
            .expect("writes onto the wire");
    }

    async fn read_deframed(socket: &mut TcpStream) -> std::vec::Vec<u8> {
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ core::FRAME_CAP }>::new());
        let mut buf = [0u8; 256];
        loop {
            let n = socket.read(&mut buf).await.expect("reads from the wire");
            assert_ne!(n, 0, "the wire stays up while a frame is owed");
            for &byte in &buf[..n] {
                if let Ok(Some(frame)) = decoder.feed(byte) {
                    if !frame.is_empty() {
                        return frame.to_vec();
                    }
                }
            }
        }
    }

    fn duplex_member(
        tag: &[u8],
        bitrate: BitrateBps,
    ) -> BackboneServerConnection<tokio::io::DuplexStream> {
        let (near, _far) = tokio::io::duplex(64);
        BackboneServerConnection::new(tag.to_vec(), near, bitrate)
    }

    #[test]
    fn the_member_id_is_a_backbone_server_peer_kind_from_the_tag() {
        let iface = duplex_member(b"127.0.0.1:54321", core::BACKBONE_BITRATE_GUESS_BPS);
        assert_eq!(iface.id().kind(), Some(InterfaceKind::BackboneServerPeer));
        let same = duplex_member(b"127.0.0.1:54321", core::BACKBONE_BITRATE_GUESS_BPS);
        assert_eq!(iface.id(), same.id(), "the same peer addr is the same id");
        let other = duplex_member(b"127.0.0.1:54322", core::BACKBONE_BITRATE_GUESS_BPS);
        assert_ne!(iface.id(), other.id(), "a different peer is a different id");
    }

    #[test]
    fn the_member_descriptor_declares_the_listeners_bitrate() {
        let iface = duplex_member(b"peer", BitrateBps::guess(12_345_678));
        let descriptor = iface.descriptor();
        assert_eq!(descriptor.id, iface.id());
        assert_eq!(
            descriptor.bitrate,
            BitrateBps::guess(12_345_678),
            "the listener's pipe claim rides into the member's descriptor",
        );
    }

    #[tokio::test]
    async fn a_member_frames_and_deframes_across_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        // The server side: accept one connection and serve it as a member.
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            BackboneServerConnection::new(
                peer.to_string().into_bytes(),
                stream,
                core::BACKBONE_BITRATE_GUESS_BPS,
            )
            .run(seam)
            .await;
        });

        let mut client = TcpStream::connect(addr).await.expect("the client connects");

        // Inbound: a framed payload (FLAG/ESC exercise the escaping) crosses the real socket and lands
        // deframed at the seam.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut client, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member deframes within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload);

        // Outbound: the seam yields a frame; it leaves the socket framed.
        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), read_deframed(&mut client))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);
    }
}
