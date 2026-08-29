mod host;

pub use host::{
    open_host_serial, open_host_serial_with_settings, scan_usb_serial_ports, HostSerial,
    HostSerialDataBits, HostSerialLineSettings, HostSerialParity, HostSerialStopBits,
    UsbSerialPort,
};

use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::byte_stream::framing;
use crate::reconnect::ReconnectPolicy;
use prns_core::interfaces::serial as contract;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub struct SerialInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect_policy: ReconnectPolicy,
    policy: EffectiveInterfacePolicy,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> SerialInterface<Open> {
    /// A reopened device must retain its `channel_tag`, and concurrent devices must have distinct tags.
    #[must_use]
    pub fn new(open: Open, reconnect_policy: ReconnectPolicy, channel_tag: &[u8]) -> Self {
        Self::with_policy(
            open,
            reconnect_policy,
            contract::policy_for_bitrate(contract::SERIAL_BITRATE_BPS),
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_policy(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Serial, &channel_tag);
        Self {
            id,
            open,
            reconnect_policy,
            policy,
            channel_tag,
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

impl<Open, Fut, S> Interface for SerialInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = prns_core::interfaces::serial::SERIAL_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Serial;

    fn descriptor(&self) -> InterfaceDescriptor {
        contract::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let bitrate = self.policy.bitrate;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framing::FramedBuffers<
                framing::HdlcFraming,
                { contract::READ_BUF_LEN },
                { contract::FRAMED_LEN },
            >,
        > = None;
        let mut reconnect = self.reconnect_policy.schedule();
        loop {
            if let Ok(stream) = (self.open)().await {
                let connected_at = tokio::time::Instant::now();
                self.status.set_connection(ConnectionState::Connected);
                framing::serve::<
                    framing::HdlcFraming,
                    { contract::READ_BUF_LEN },
                    { contract::FRAMED_LEN },
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
                        bitrate,
                        started,
                    },
                )
                .await;
                self.status.set_connection(ConnectionState::Disconnected);
                reconnect.record_connection_lifetime(connected_at.elapsed());
            }
            let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            tokio::time::sleep(reconnect_delay).await;
        }
    }
}

impl<Open> prns_core::interfaces::ReportsStatus for SerialInterface<Open> {
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
    use prns_core::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use prns_core::interfaces::InterfaceStatus;
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    #[tokio::test]
    async fn frames_outbound_and_deframes_inbound_across_a_real_async_stream() {
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::SERIAL_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let interface = SerialInterface::new(open, ReconnectPolicy::STANDARD, b"test-serial");
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).expect("encodes the payload");
        test_wire
            .write_all(&framed[..n])
            .await
            .expect("writes onto the wire");

        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(
            received, payload,
            "the interface deframes inbound bytes for the seam"
        );

        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();

        let mut decoder = contract::Decoder::new();
        let mut buf = [0u8; 64];
        let decoded = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let n = test_wire.read(&mut buf).await.expect("reads from the wire");
                for &byte in &buf[..n] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            return frame.to_vec();
                        }
                    }
                }
            }
        })
        .await
        .expect("the interface frames outbound within the window");
        assert_eq!(
            decoded, out_payload,
            "the interface frames outbound packets onto the wire"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.connection() == ConnectionState::Connected
                    && status.rx_bytes() > 0
                    && status.tx_bytes() > 0
                    && status.airtime().is_some()
                    && status.transfer_rates().is_some()
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the live status reflects the connection + bytes both ways within the window");
    }
}
