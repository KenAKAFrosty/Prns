use std::string::String;

use crate::byte_stream::framing;
use crate::reconnect::ReconnectPolicy;
use crate::tcp::{connect, tune_for_tunnel, TcpConnectionSettings};
use prns_core::interfaces::tcp::{self, TcpWireFraming};
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;
#[cfg(test)]
use std::time::Duration;

pub struct TcpClientInterface {
    id: InterfaceId,
    target: String,
    channel_tag: std::vec::Vec<u8>,
    policy: EffectiveInterfacePolicy,
    connection: TcpConnectionSettings,
    framing: TcpWireFraming,
    status: TokioInterfaceStatus,
}

impl TcpClientInterface {
    #[must_use]
    pub fn new(target: String) -> Self {
        Self::new_with_bitrate(target, tcp::TCP_BITRATE_ESTIMATE, ReconnectPolicy::STANDARD)
    }

    #[must_use]
    pub fn new_with_bitrate(
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, target.as_bytes());
        Self::with_id_policy_and_framing(
            id,
            target,
            tcp::policy_for_bitrate(bitrate),
            reconnect_policy,
            TcpWireFraming::Hdlc,
        )
    }

    #[must_use]
    pub fn with_policy(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, target.as_bytes());
        Self::with_id_policy_and_framing(id, target, policy, reconnect_policy, TcpWireFraming::Hdlc)
    }

    #[must_use]
    pub fn with_policy_and_framing(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
        framing: TcpWireFraming,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, &channel_tag);
        Self::with_id_policy_and_framing(id, target, policy, reconnect_policy, framing)
    }

    #[must_use]
    pub fn with_policy_and_connection_settings(
        target: String,
        policy: EffectiveInterfacePolicy,
        framing: TcpWireFraming,
        connection: TcpConnectionSettings,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, &channel_tag);
        Self::with_id_policy_framing_and_connection_settings(
            id, target, policy, framing, connection,
        )
    }

    #[must_use]
    pub fn with_framing(
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
        framing: TcpWireFraming,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, &channel_tag);
        Self::with_id_policy_and_framing(
            id,
            target,
            tcp::policy_for_bitrate(bitrate),
            reconnect_policy,
            framing,
        )
    }

    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        Self::with_id_policy_and_framing(
            id,
            target,
            tcp::policy_for_bitrate(bitrate),
            reconnect_policy,
            TcpWireFraming::Hdlc,
        )
    }

    #[must_use]
    pub fn new_with_id_and_framing(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
        framing: TcpWireFraming,
    ) -> Self {
        Self::with_id_policy_and_framing(
            id,
            target,
            tcp::policy_for_bitrate(bitrate),
            reconnect_policy,
            framing,
        )
    }

    #[must_use]
    pub fn with_id_and_policy(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        Self::with_id_policy_and_framing(id, target, policy, reconnect_policy, TcpWireFraming::Hdlc)
    }

    #[must_use]
    pub fn with_id_policy_and_framing(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
        framing: TcpWireFraming,
    ) -> Self {
        Self::with_id_policy_framing_and_connection_settings(
            id,
            target,
            policy,
            framing,
            TcpConnectionSettings {
                reconnect_policy,
                ..TcpConnectionSettings::STOCK
            },
        )
    }

    #[must_use]
    pub fn with_id_policy_framing_and_connection_settings(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        framing: TcpWireFraming,
        connection: TcpConnectionSettings,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        Self {
            id,
            target,
            channel_tag,
            policy,
            connection,
            framing,
            status: TokioInterfaceStatus::new_accounted(id, ConnectionState::Initializing),
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

impl Interface for TcpClientInterface {
    const HW_MTU: usize = tcp::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        tcp::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let interface_origin = seam.interface_origin().as_str();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framing::FramedBuffers<
                framing::HdlcFraming,
                { tcp::READ_BUF_LEN },
                { tcp::FRAMED_LEN },
            >,
        > = None;
        let mut kiss_buffers: Option<
            framing::FramedBuffers<
                framing::KissFraming,
                { tcp::READ_BUF_LEN },
                { tcp::KISS_FRAMED_LEN },
            >,
        > = None;
        let mut reconnect_attempts = 0u32;
        let mut reconnect = self.connection.reconnect_policy.schedule();
        loop {
            #[cfg(feature = "tracing")]
            let connected = tracing::Instrument::instrument(
                connect(self.target.as_str(), self.connection),
                tracing::debug_span!(
                    target: "prns.interface",
                    "prns.interface.connect",
                    interface_kind = "tcp_client",
                    interface_origin,
                    peer = %self.target,
                ),
            )
            .await;
            #[cfg(not(feature = "tracing"))]
            let connected = connect(self.target.as_str(), self.connection).await;
            if let Ok(stream) = connected {
                let connected_at = tokio::time::Instant::now();
                tune_for_tunnel(&stream, self.connection.tunnel);
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: connected {}",
                    self.target
                );
                self.status.set_connection(ConnectionState::Connected);
                if self.framing == TcpWireFraming::Hdlc {
                    seam.request_tunnel_synthesis().await;
                }
                let mut meters = framing::WireMeters {
                    status: &self.status,
                    airtime: &mut airtime,
                    throughput: &mut throughput,
                    bitrate: self.policy.bitrate,
                    started,
                };
                match self.framing {
                    TcpWireFraming::Hdlc => {
                        framing::serve::<
                            framing::HdlcFraming,
                            { tcp::READ_BUF_LEN },
                            { tcp::FRAMED_LEN },
                            _,
                            _,
                        >(
                            stream,
                            buffers.get_or_insert_with(framing::FramedBuffers::new),
                            &mut seam,
                            &mut meters,
                        )
                        .await;
                    }
                    TcpWireFraming::Kiss => {
                        framing::serve::<
                            framing::KissFraming,
                            { tcp::READ_BUF_LEN },
                            { tcp::KISS_FRAMED_LEN },
                            _,
                            _,
                        >(
                            stream,
                            kiss_buffers.get_or_insert_with(framing::FramedBuffers::new),
                            &mut seam,
                            &mut meters,
                        )
                        .await;
                    }
                }
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: dropped {}, retrying",
                    self.target
                );
                self.status.set_connection(ConnectionState::Disconnected);
                reconnect_attempts = 0;
                reconnect.record_connection_lifetime(connected_at.elapsed());
            } else {
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: connect failed {}, retrying",
                    self.target
                );
                self.status.set_connection(ConnectionState::Disconnected);
            }
            if self
                .connection
                .reconnect_limit
                .exhausted(reconnect_attempts)
            {
                return;
            }
            let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            reconnect_attempts = reconnect_attempts.saturating_add(1);
            tokio::time::sleep(reconnect_delay).await;
        }
    }
}

fn channel_tag(target: &str, framing: TcpWireFraming) -> std::vec::Vec<u8> {
    let mut channel_tag = target.as_bytes().to_vec();
    if framing == TcpWireFraming::Kiss {
        channel_tag.extend_from_slice(b"\0kiss");
    }
    channel_tag
}

impl prns_core::interfaces::ReportsStatus for TcpClientInterface {
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
    use prns_core::interfaces::kiss_framing::{self, KissDecoder, FEND, FESC};
    use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
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
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ tcp::FRAME_CAP }>::new());
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

    async fn read_kiss_deframed(socket: &mut TcpStream) -> std::vec::Vec<u8> {
        let mut decoder = std::boxed::Box::new(KissDecoder::<{ tcp::FRAME_CAP }>::new());
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

    #[tokio::test]
    async fn the_client_frames_deframes_and_reconnects_across_real_sockets() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let interface = TcpClientInterface::new_with_bitrate(
            addr.to_string(),
            tcp::TCP_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
        );
        tokio::spawn(interface.run(seam));

        let (mut first, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client connects within the window")
            .expect("the listener accepts");

        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut first, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload);

        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), read_deframed(&mut first))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);

        drop(first);
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client reconnects within the window")
            .expect("the listener accepts again");
        write_framed(&mut second, b"after the reconnect").await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the reconnected interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, b"after the reconnect");
    }

    #[tokio::test]
    async fn the_client_honors_kiss_framing_in_both_directions() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        let interface = TcpClientInterface::with_framing(
            addr.to_string(),
            tcp::TCP_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
            TcpWireFraming::Kiss,
        );
        tokio::spawn(interface.run(seam));
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client connects within the window")
            .expect("the listener accepts");

        let inbound = [0x01u8, FEND, FESC, 0x02];
        let mut framed = [0u8; 32];
        let framed_len =
            kiss_framing::encode(&inbound, &mut framed).expect("encodes the inbound payload");
        socket
            .write_all(&framed[..framed_len])
            .await
            .expect("writes the inbound frame");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, inbound);

        let outbound = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&outbound);
        out_tx.commit();
        let received =
            tokio::time::timeout(Duration::from_secs(2), read_kiss_deframed(&mut socket))
                .await
                .expect("the frame leaves within the window");
        assert_eq!(received, outbound);
    }

    #[test]
    fn framing_is_part_of_the_effective_tcp_channel() {
        let hdlc = TcpClientInterface::with_framing(
            "peer.example:4242".to_string(),
            tcp::TCP_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
            TcpWireFraming::Hdlc,
        );
        let kiss = TcpClientInterface::with_framing(
            "peer.example:4242".to_string(),
            tcp::TCP_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
            TcpWireFraming::Kiss,
        );
        assert_ne!(hdlc.id(), kiss.id());
        assert_ne!(hdlc.channel_tag(), kiss.channel_tag());
    }
}
