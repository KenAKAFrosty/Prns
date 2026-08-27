use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::byte_stream::framing::WireMeters;
use crate::kiss::{
    configure_tnc, serve_controlled_kiss, ControlledKissBuffers, TncConfigureDelay,
    DEFAULT_TNC_CONFIGURE_DELAY,
};
use crate::reconnect::ReconnectPolicy;
use prns_core::interfaces::ax25_kiss::{self as contract, Ax25AddressError, AX25_HEADER_SIZE};
use prns_core::interfaces::kiss::TncConfig;
use prns_core::interfaces::kiss::{KissTransmissionControl, ReadyCommandFlowControl};
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, FrameSink, InterfaceDescriptor, InterfaceId,
    InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

struct Ax25Seam<S> {
    inner: S,
    header: [u8; AX25_HEADER_SIZE],
    inbound: std::vec::Vec<u8>,
    outbound: std::vec::Vec<u8>,
}

impl<S: InterfaceSeam> InterfaceSeam for Ax25Seam<S> {
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        self.inner.fill_entropy(bytes);
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.inbound
    }

    async fn commit_inbound(&mut self) {
        // Match RNS's strict `len(data) > HEADER_SIZE` delivery condition.
        if self.inbound.len() > AX25_HEADER_SIZE {
            self.inner
                .next_inbound(&self.inbound[AX25_HEADER_SIZE..])
                .await;
        }
        self.inbound.clear();
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.clear();
        self.outbound.extend_from_slice(&self.header);
        let packet = self.inner.next_outbound().await;
        self.outbound.extend_from_slice(packet);
        &self.outbound
    }

    fn try_next_outbound(&mut self) -> Option<&[u8]> {
        let packet = self.inner.try_next_outbound()?;
        self.outbound.clear();
        self.outbound.extend_from_slice(&self.header);
        self.outbound.extend_from_slice(packet);
        Some(&self.outbound)
    }
}

pub struct Ax25KissInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect_policy: ReconnectPolicy,
    configure_delay: TncConfigureDelay,
    tnc: TncConfig,
    flow_control: ReadyCommandFlowControl,
    policy: EffectiveInterfacePolicy,
    header: [u8; AX25_HEADER_SIZE],
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

pub struct Ax25KissSettings<'a> {
    pub configure_delay: TncConfigureDelay,
    pub tnc: TncConfig,
    pub flow_control: ReadyCommandFlowControl,
    pub callsign: &'a str,
    pub ssid: u8,
    pub policy: EffectiveInterfacePolicy,
    pub channel_tag: &'a [u8],
}

impl<Open> Ax25KissInterface<Open> {
    pub fn new(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        callsign: &str,
        ssid: u8,
        channel_tag: &[u8],
    ) -> Result<Self, Ax25AddressError> {
        Self::with_settings(
            open,
            reconnect_policy,
            DEFAULT_TNC_CONFIGURE_DELAY,
            TncConfig::default(),
            callsign,
            ssid,
            channel_tag,
        )
    }

    pub fn with_settings(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        configure_delay: TncConfigureDelay,
        tnc: TncConfig,
        callsign: &str,
        ssid: u8,
        channel_tag: &[u8],
    ) -> Result<Self, Ax25AddressError> {
        Self::with_policy(
            open,
            reconnect_policy,
            Ax25KissSettings {
                configure_delay,
                tnc,
                flow_control: ReadyCommandFlowControl::Disabled,
                callsign,
                ssid,
                policy: contract::configured_policy(Default::default()),
                channel_tag,
            },
        )
    }

    pub fn with_policy(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        settings: Ax25KissSettings<'_>,
    ) -> Result<Self, Ax25AddressError> {
        let header = contract::build_header(settings.callsign, settings.ssid)?;
        let channel_tag = settings.channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Ax25Kiss, &channel_tag);
        Ok(Self {
            id,
            open,
            reconnect_policy,
            configure_delay: settings.configure_delay,
            tnc: settings.tnc,
            flow_control: settings.flow_control,
            policy: settings.policy,
            header,
            channel_tag,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Initializing),
        })
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

impl<Open, Fut, S> Interface for Ax25KissInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = contract::AX25_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Ax25Kiss;

    fn descriptor(&self) -> InterfaceDescriptor {
        contract::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, seam: Seam) {
        let bitrate = self.policy.bitrate;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut control = KissTransmissionControl::new(self.flow_control, None);
        let mut seam = Ax25Seam {
            inner: seam,
            header: self.header,
            inbound: std::vec::Vec::with_capacity(contract::AX25_FRAME_LEN),
            outbound: std::vec::Vec::with_capacity(contract::AX25_FRAME_LEN),
        };
        let mut buffers: Option<
            ControlledKissBuffers<
                { contract::AX25_FRAME_LEN },
                { contract::READ_BUF_LEN },
                { contract::FRAMED_LEN },
            >,
        > = None;
        let mut reconnect = self.reconnect_policy.schedule();
        loop {
            if let Ok(mut stream) = (self.open)().await {
                if !self.configure_delay.duration().is_zero() {
                    tokio::time::sleep(self.configure_delay.duration()).await;
                }
                if configure_tnc(&mut stream, &self.tnc).await.is_ok() {
                    let connected_at = tokio::time::Instant::now();
                    self.status.set_connection(ConnectionState::Connected);
                    serve_controlled_kiss(
                        &mut stream,
                        buffers.get_or_insert_with(ControlledKissBuffers::new),
                        &mut seam,
                        &mut control,
                        &mut WireMeters {
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
            }
            let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            tokio::time::sleep(reconnect_delay).await;
        }
    }
}

impl<Open> prns_core::interfaces::ReportsStatus for Ax25KissInterface<Open> {
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
    use prns_core::interfaces::kiss::ReadyTimeout;
    use prns_core::interfaces::kiss_framing::{self, FEND, FESC};
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

    async fn write_kiss(wire: &mut tokio::io::DuplexStream, body: &[u8]) {
        let mut framed = std::vec![0u8; kiss_framing::max_encoded_len(body.len())];
        let n = kiss_framing::encode(body, &mut framed).expect("encodes the body");
        wire.write_all(&framed[..n])
            .await
            .expect("writes the frame");
    }

    async fn read_kiss(wire: &mut tokio::io::DuplexStream) -> Vec<u8> {
        let mut decoder = contract::Decoder::new();
        let mut buf = [0u8; 128];
        loop {
            let read = wire.read(&mut buf).await.expect("reads from the wire");
            for &byte in &buf[..read] {
                if let Ok(Some(frame)) = decoder.feed(byte) {
                    if !frame.is_empty() {
                        return frame.to_vec();
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn wraps_outbound_in_ax25_and_strips_the_header_inbound_over_a_real_stream() {
        let callsign = "N0CALL";
        let ssid = 3;
        let header = contract::build_header(callsign, ssid).unwrap();

        let (interface_wire, mut test_wire) = tokio::io::duplex(2048);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::AX25_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let interface = Ax25KissInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            TncConfigureDelay::new(Duration::ZERO),
            TncConfig::default(),
            callsign,
            ssid,
            b"test-ax25",
        )
        .expect("a valid AX.25 address");
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        let mut config = [0u8; 20];
        tokio::time::timeout(Duration::from_secs(2), test_wire.read_exact(&mut config))
            .await
            .expect("the config frames arrive")
            .expect("the wire stays up through config");
        assert_eq!(
            config,
            [
                FEND,
                kiss_framing::CMD_TXDELAY,
                35,
                FEND,
                FEND,
                kiss_framing::CMD_TXTAIL,
                2,
                FEND,
                FEND,
                kiss_framing::CMD_P,
                64,
                FEND,
                FEND,
                kiss_framing::CMD_SLOTTIME,
                2,
                FEND,
                FEND,
                kiss_framing::CMD_READY,
                1,
                FEND,
            ]
        );

        let payload = [0x01u8, 0x02, FEND, FESC, 0x03];
        let mut body = header.to_vec();
        body.extend_from_slice(&payload);
        write_kiss(&mut test_wire, &body).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload, "the AX.25 header is stripped inbound");

        let out_payload = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();

        let mut decoder = contract::Decoder::new();
        let mut buf = [0u8; 128];
        let body = tokio::time::timeout(Duration::from_secs(2), async {
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
            &body[..AX25_HEADER_SIZE],
            &header,
            "outbound is AX.25-wrapped"
        );
        assert_eq!(
            &body[AX25_HEADER_SIZE..],
            &out_payload,
            "with the packet after the header"
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

    #[tokio::test]
    async fn ready_flow_control_gates_ax25_frames() {
        let callsign = "N0CALL";
        let ssid = 3;
        let header = contract::build_header(callsign, ssid).expect("valid address");
        let (interface_wire, mut test_wire) = tokio::io::duplex(2048);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::AX25_FRAME_LEN, 3);
        let seam = MockSeam {
            inbound: in_tx,
            sink: Vec::new(),
            outbound: out_rx,
        };
        let interface = Ax25KissInterface::with_policy(
            open,
            ReconnectPolicy::STANDARD,
            Ax25KissSettings {
                configure_delay: TncConfigureDelay::new(Duration::ZERO),
                tnc: TncConfig::default(),
                flow_control: ReadyCommandFlowControl::WaitForReadyOrTimeout(ReadyTimeout::new(
                    Duration::from_millis(100),
                )),
                callsign,
                ssid,
                policy: contract::configured_policy(Default::default()),
                channel_tag: b"flow-ax25",
            },
        )
        .expect("valid interface");
        tokio::spawn(interface.run(seam));
        let mut config = [0u8; 20];
        test_wire
            .read_exact(&mut config)
            .await
            .expect("reads config");

        out_tx.try_grant().expect("first slot").fill(b"first");
        out_tx.commit();
        let first = read_kiss(&mut test_wire).await;
        assert_eq!(&first[..AX25_HEADER_SIZE], &header);
        assert_eq!(&first[AX25_HEADER_SIZE..], b"first");

        out_tx.try_grant().expect("second slot").fill(b"second");
        out_tx.commit();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), read_kiss(&mut test_wire))
                .await
                .is_err()
        );
        test_wire
            .write_all(&kiss_framing::command_frame(kiss_framing::CMD_READY, 1))
            .await
            .expect("writes ready");
        let second = read_kiss(&mut test_wire).await;
        assert_eq!(&second[..AX25_HEADER_SIZE], &header);
        assert_eq!(&second[AX25_HEADER_SIZE..], b"second");
    }
}
