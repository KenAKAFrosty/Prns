use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::byte_stream::framing;
use prns_core::interfaces::tcp;
use prns_core::interfaces::wifi_direct as contract;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub struct WifiDirectMember<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    closed: Option<mpsc::UnboundedSender<InterfaceId>>,
}

impl<S> WifiDirectMember<S> {
    #[must_use]
    pub fn with_policy(channel_tag: Vec<u8>, stream: S, policy: EffectiveInterfacePolicy) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiDirectPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            policy,
            status: TokioInterfaceStatus::new_accounted(id, ConnectionState::Connected),
            closed: None,
        }
    }

    #[must_use]
    pub fn report_close_to(mut self, sink: mpsc::UnboundedSender<InterfaceId>) -> Self {
        self.closed = Some(sink);
        self
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

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for WifiDirectMember<S> {
    const HW_MTU: usize = contract::WIFI_DIRECT_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::WifiDirectPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.policy.descriptor(self.id)
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
            { tcp::READ_BUF_LEN },
            { tcp::FRAMED_LEN },
        >::new();
        framing::serve::<framing::HdlcFraming, { tcp::READ_BUF_LEN }, { tcp::FRAMED_LEN }, _, _>(
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
        if let Some(sink) = self.closed.take() {
            let _ = sink.send(self.id);
            std::future::pending::<()>().await;
        }
    }
}

impl<S> prns_core::interfaces::ReportsStatus for WifiDirectMember<S> {
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
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

    fn duplex_member(tag: &[u8]) -> (WifiDirectMember<DuplexStream>, DuplexStream) {
        let (near, far) = tokio::io::duplex(1024);
        (
            WifiDirectMember::with_policy(
                tag.to_vec(),
                near,
                contract::defaults_for_bitrate(contract::WIFI_DIRECT_BITRATE_GUESS_BPS)
                    .configured(Default::default()),
            ),
            far,
        )
    }

    #[test]
    fn the_member_id_is_a_wifi_direct_peer_kind_from_the_tag() {
        let (member, _far) = duplex_member(b"192.168.49.10:42717");
        assert_eq!(member.id().kind(), Some(InterfaceKind::WifiDirectPeer));
        let (same, _far) = duplex_member(b"192.168.49.10:42717");
        assert_eq!(member.id(), same.id());
        let (other, _far) = duplex_member(b"192.168.49.11:42717");
        assert_ne!(member.id(), other.id());
    }

    #[test]
    fn the_member_descriptor_carries_the_declared_bitrate() {
        let (member, _far) = duplex_member(b"peer");
        let descriptor = member.descriptor();
        assert_eq!(descriptor.id, member.id());
        assert_eq!(descriptor.bitrate, contract::WIFI_DIRECT_BITRATE_GUESS_BPS);
    }

    #[test]
    fn the_member_descriptor_inherits_the_complete_interface_policy() {
        let (near, _far) = tokio::io::duplex(1024);
        let policy = contract::defaults_for_bitrate(contract::WIFI_DIRECT_BITRATE_GUESS_BPS)
            .configured(prns_core::interfaces::ConfiguredInterfacePolicy {
                mode: Some(prns_core::interfaces::InterfaceMode::Gateway),
                gravity: Some(prns_core::interfaces::InterfaceGravity::new(13)),
                ..Default::default()
            });
        let member = WifiDirectMember::with_policy(b"peer".to_vec(), near, policy);

        assert_eq!(member.descriptor().mode, policy.mode);
        assert_eq!(member.descriptor().gravity, policy.gravity);
    }

    #[tokio::test]
    async fn frames_cross_hdlc_framed_in_both_directions() {
        let (member, mut far) = duplex_member(b"peer");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        tokio::spawn(member.run(seam));

        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 64];
        let n = rns_serial_framing::encode(&payload, &mut framed).expect("encodes the payload");
        far.write_all(&framed[..n])
            .await
            .expect("writes onto the pipe");
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
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ tcp::FRAME_CAP }>::new());
        let mut buf = [0u8; 256];
        'outer: loop {
            let n = far.read(&mut buf).await.expect("reads from the pipe");
            assert_ne!(n, 0, "the pipe stays up while a frame is owed");
            for &byte in &buf[..n] {
                if let Ok(Some(frame)) = decoder.feed(byte) {
                    if !frame.is_empty() {
                        assert_eq!(frame.to_vec(), out_payload);
                        break 'outer;
                    }
                }
            }
        }
    }

    fn idle_seam() -> MockSeam {
        let (discard, _discard_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (_idle_producer, idle_consumer) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        MockSeam {
            inbound: discard,
            sink: std::vec::Vec::new(),
            outbound: idle_consumer,
        }
    }

    #[tokio::test]
    async fn a_dying_member_fires_its_close_signal() {
        let (member, far) = duplex_member(b"peer");
        let id = member.id();
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let member = member.report_close_to(closed_tx);
        tokio::spawn(member.run(idle_seam()));

        drop(far);
        let reported = tokio::time::timeout(Duration::from_secs(2), closed_rx.recv())
            .await
            .expect("the close signal fires within the window")
            .expect("the sender side is alive");
        assert_eq!(reported, id);
    }

    #[tokio::test]
    async fn a_torn_down_member_does_not_fire_its_close_signal() {
        let (member, far) = duplex_member(b"peer");
        let _keep_peer_alive = far;
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let member = member.report_close_to(closed_tx);
        let handle = tokio::spawn(member.run(idle_seam()));

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(closed_rx.try_recv().is_err());
    }
}
