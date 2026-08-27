use crate::reconnect::ReconnectPolicy;
use std::string::String;

use crate::byte_stream::framing;
use crate::tcp::{connect, tune_for_tunnel, TcpConnectionSettings};
use prns_core::interfaces::backbone;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub struct BackboneClientInterface {
    id: InterfaceId,
    target: String,
    policy: EffectiveInterfacePolicy,
    connection: TcpConnectionSettings,
    status: TokioInterfaceStatus,
}

impl BackboneClientInterface {
    #[must_use]
    pub fn new(target: String, bitrate: BitrateBps, reconnect_policy: ReconnectPolicy) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneClient, target.as_bytes());
        Self::with_id_and_policy(
            id,
            target,
            backbone::policy_for_bitrate(bitrate),
            reconnect_policy,
        )
    }

    #[must_use]
    pub fn with_policy(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneClient, target.as_bytes());
        Self::with_id_and_policy(id, target, policy, reconnect_policy)
    }

    #[must_use]
    pub fn with_policy_and_connection_settings(
        target: String,
        policy: EffectiveInterfacePolicy,
        connection: TcpConnectionSettings,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneClient, target.as_bytes());
        Self::with_id_policy_and_connection_settings(id, target, policy, connection)
    }

    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        Self::with_id_and_policy(
            id,
            target,
            backbone::policy_for_bitrate(bitrate),
            reconnect_policy,
        )
    }

    #[must_use]
    pub fn with_id_and_policy(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Self {
        Self::with_id_policy_and_connection_settings(
            id,
            target,
            policy,
            TcpConnectionSettings {
                reconnect_policy,
                ..TcpConnectionSettings::STOCK
            },
        )
    }

    #[must_use]
    pub fn with_id_policy_and_connection_settings(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        connection: TcpConnectionSettings,
    ) -> Self {
        Self {
            id,
            target,
            policy,
            connection,
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

impl Interface for BackboneClientInterface {
    const HW_MTU: usize = backbone::HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::BackboneClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        backbone::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        self.target.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let interface_origin = seam.interface_origin().as_str();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framing::FramedBuffers<
                framing::HdlcFraming,
                { backbone::READ_BUF_LEN },
                { backbone::FRAMED_LEN },
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
                    interface_kind = "backbone_client",
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
                    "backbone-client [{interface_origin}]: connected {}",
                    self.target
                );
                self.status.set_connection(ConnectionState::Connected);
                seam.request_tunnel_synthesis().await;
                framing::serve::<
                    framing::HdlcFraming,
                    { backbone::READ_BUF_LEN },
                    { backbone::FRAMED_LEN },
                    _,
                    _,
                >(
                    stream,
                    buffers.get_or_insert_with(framing::FramedBuffers::new),
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
                crate::diagnostic_log::debug!(
                    "backbone-client [{interface_origin}]: dropped {}, retrying",
                    self.target
                );
                self.status.set_connection(ConnectionState::Disconnected);
                reconnect_attempts = 0;
                reconnect.record_connection_lifetime(connected_at.elapsed());
            } else {
                crate::diagnostic_log::debug!(
                    "backbone-client [{interface_origin}]: connect failed {}, retrying",
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

impl prns_core::interfaces::ReportsStatus for BackboneClientInterface {
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

    #[test]
    fn the_client_id_is_a_backbone_client_kind_from_the_target() {
        let iface = BackboneClientInterface::new(
            "hub.example.com:4965".to_string(),
            backbone::BACKBONE_CLIENT_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
        );
        assert_eq!(iface.id().kind(), Some(InterfaceKind::BackboneClient));
    }

    #[tokio::test]
    async fn the_client_frames_deframes_and_reconnects_across_real_sockets() {
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

        let interface = BackboneClientInterface::new(
            addr.to_string(),
            backbone::BACKBONE_CLIENT_BITRATE_ESTIMATE,
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
}
