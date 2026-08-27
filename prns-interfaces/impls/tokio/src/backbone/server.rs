use std::io;
use std::net::SocketAddr;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::byte_stream::framing;
use crate::reconnect::ReconnectPolicy;
use crate::tcp::tune;
use prns_core::interfaces::backbone;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

pub struct BackboneServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
}

impl<S> BackboneServerConnection<S> {
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S, bitrate: BitrateBps) -> Self {
        Self::with_policy(channel_tag, stream, backbone::policy_for_bitrate(bitrate))
    }

    #[must_use]
    pub fn with_policy(channel_tag: Vec<u8>, stream: S, policy: EffectiveInterfacePolicy) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            policy,
            status: TokioInterfaceStatus::new_accounted(id, ConnectionState::Connected),
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

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for BackboneServerConnection<S> {
    const HW_MTU: usize = backbone::HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::BackboneServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        backbone::descriptor(self.id, self.policy)
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
            { backbone::READ_BUF_LEN },
            { backbone::FRAMED_LEN },
        >::new();
        framing::serve::<
            framing::HdlcFraming,
            { backbone::READ_BUF_LEN },
            { backbone::FRAMED_LEN },
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

pub struct BackboneServer {
    listener: TcpListener,
    policy: EffectiveInterfacePolicy,
    channel_tag: Vec<u8>,
    status: TokioInterfaceStatus,
}

impl BackboneServer {
    pub async fn bind(
        addr: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> io::Result<Self> {
        Self::bind_with_policy(addr, backbone::policy_for_bitrate(bitrate)).await
    }

    pub async fn bind_with_policy(
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = listener.local_addr()?.to_string().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneServer, &channel_tag);
        Ok(Self {
            listener,
            policy,
            channel_tag,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::BackboneServer, &self.channel_tag)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl InterfaceSupervisor for BackboneServer {
    const KIND: InterfaceKind = InterfaceKind::BackboneServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let policy = ReconnectPolicy::STANDARD;
        let mut schedule = policy.schedule();
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    schedule = policy.schedule();
                    tune(&stream);
                    let _ = fleet.add(BackboneServerConnection::with_policy(
                        peer.to_string().into_bytes(),
                        stream,
                        self.policy,
                    ));
                }
                Err(_) => {
                    let delay = schedule.next_delay(|bytes| fleet.fill_entropy(bytes));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

impl prns_core::interfaces::ReportsStatus for BackboneServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> prns_core::interfaces::ReportsStatus for BackboneServerConnection<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }

    fn frame_accounting_recorder(&self) -> Option<prns_core::interfaces::FrameAccountingRecorder> {
        prns_core::interfaces::FrameAccountingRecorder::of(self.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc::{self, UnboundedSender};

    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

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
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ backbone::FRAME_CAP }>::new());
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
        let iface = duplex_member(b"127.0.0.1:54321", backbone::BACKBONE_BITRATE_ESTIMATE);
        assert_eq!(iface.id().kind(), Some(InterfaceKind::BackboneServerPeer));
        let same = duplex_member(b"127.0.0.1:54321", backbone::BACKBONE_BITRATE_ESTIMATE);
        assert_eq!(iface.id(), same.id(), "the same peer addr is the same id");
        let other = duplex_member(b"127.0.0.1:54322", backbone::BACKBONE_BITRATE_ESTIMATE);
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
    async fn the_listener_publishes_a_stable_unaccounted_supervisor_status() {
        let server = BackboneServer::bind("127.0.0.1:0", backbone::BACKBONE_BITRATE_ESTIMATE)
            .await
            .expect("binds an ephemeral listener");
        let view = prns_core::interfaces::ReportsStatus::status_view(&server)
            .expect("the listener publishes status");
        let vitals = view();

        assert_eq!(vitals.len(), 1);
        assert_eq!(vitals[0].id, server.id());
        assert_eq!(vitals[0].connection, ConnectionState::Connected);
        assert_eq!(vitals[0].frame_accounting, None);
    }

    #[tokio::test]
    async fn a_member_frames_and_deframes_across_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(backbone::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            BackboneServerConnection::new(
                peer.to_string().into_bytes(),
                stream,
                backbone::BACKBONE_BITRATE_ESTIMATE,
            )
            .run(seam)
            .await;
        });

        let mut client = TcpStream::connect(addr).await.expect("the client connects");

        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut client, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member deframes within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload);

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
