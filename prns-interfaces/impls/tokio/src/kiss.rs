use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::byte_stream::deadline::{elapsed_millis, wait_for_deadline};
use crate::byte_stream::framing::WireMeters;
use crate::reconnect::ReconnectPolicy;
use prns_core::engine::InstantMillis;
use prns_core::interfaces::kiss::{self as contract, TncConfig};
use prns_core::interfaces::kiss::{
    KissTransmissionControl, ReadyCommandFlowControl, StationIdentification, Transmission,
};
use prns_core::interfaces::kiss_framing;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

/// A real TNC needs the RNS two-second boot window before it will accept configuration bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TncConfigureDelay(Duration);

impl TncConfigureDelay {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

pub const DEFAULT_TNC_CONFIGURE_DELAY: TncConfigureDelay =
    TncConfigureDelay::new(Duration::from_secs(2));

pub struct KissInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect_policy: ReconnectPolicy,
    configure_delay: TncConfigureDelay,
    tnc: TncConfig,
    flow_control: ReadyCommandFlowControl,
    station_identification: Option<StationIdentification>,
    policy: EffectiveInterfacePolicy,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

pub struct KissSettings<'a> {
    pub configure_delay: TncConfigureDelay,
    pub tnc: TncConfig,
    pub flow_control: ReadyCommandFlowControl,
    pub station_identification: Option<StationIdentification>,
    pub policy: EffectiveInterfacePolicy,
    pub channel_tag: &'a [u8],
}

impl<Open> KissInterface<Open> {
    /// A reopened device must retain its `channel_tag`, and concurrent devices must have distinct tags.
    #[must_use]
    pub fn new(open: Open, reconnect_policy: ReconnectPolicy, channel_tag: &[u8]) -> Self {
        Self::with_settings(
            open,
            reconnect_policy,
            DEFAULT_TNC_CONFIGURE_DELAY,
            TncConfig::default(),
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_settings(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        configure_delay: TncConfigureDelay,
        tnc: TncConfig,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_settings_and_policy(
            open,
            reconnect_policy,
            configure_delay,
            tnc,
            contract::configured_policy(Default::default()),
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_settings_and_policy(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        configure_delay: TncConfigureDelay,
        tnc: TncConfig,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_runtime_settings(
            open,
            reconnect_policy,
            KissSettings {
                configure_delay,
                tnc,
                flow_control: ReadyCommandFlowControl::Disabled,
                station_identification: None,
                policy,
                channel_tag,
            },
        )
    }

    #[must_use]
    pub fn with_runtime_settings(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        settings: KissSettings<'_>,
    ) -> Self {
        let channel_tag = settings.channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Kiss, &channel_tag);
        Self {
            id,
            open,
            reconnect_policy,
            configure_delay: settings.configure_delay,
            tnc: settings.tnc,
            flow_control: settings.flow_control,
            station_identification: settings.station_identification,
            policy: settings.policy,
            channel_tag,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Initializing),
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

pub(crate) struct ControlledKissBuffers<
    const FRAME_CAP: usize,
    const READ_LEN: usize,
    const FRAMED_LEN: usize,
> {
    decoder: kiss_framing::KissCommandDecoder<FRAME_CAP>,
    read_buf: Box<[u8]>,
    frame_buf: Box<[u8]>,
}

impl<const FRAME_CAP: usize, const READ_LEN: usize, const FRAMED_LEN: usize>
    ControlledKissBuffers<FRAME_CAP, READ_LEN, FRAMED_LEN>
{
    pub(crate) fn new() -> Self {
        Self {
            decoder: kiss_framing::KissCommandDecoder::new(),
            read_buf: vec![0; READ_LEN].into_boxed_slice(),
            frame_buf: vec![0; FRAMED_LEN].into_boxed_slice(),
        }
    }
}

pub(crate) async fn configure_tnc<S: AsyncWrite + Unpin>(
    stream: &mut S,
    tnc: &TncConfig,
) -> io::Result<()> {
    for (command, value) in tnc.command_sequence() {
        stream
            .write_all(&kiss_framing::command_frame(command, value))
            .await?;
    }
    Ok(())
}

pub(crate) async fn serve_controlled_kiss<
    const FRAME_CAP: usize,
    const READ_LEN: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    stream: &mut S,
    buffers: &mut ControlledKissBuffers<FRAME_CAP, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    control: &mut KissTransmissionControl,
    meters: &mut WireMeters<'_>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    buffers.decoder.reset();
    control.connection_opened();
    loop {
        if let Some(transmission) = control.next_queued(elapsed_millis(meters.started)) {
            if !write_transmission(
                stream,
                &mut buffers.frame_buf,
                control,
                transmission,
                meters,
            )
            .await
            {
                return;
            }
            continue;
        }
        let flow_deadline = control.flow_timeout_deadline();
        let station_deadline = control.station_identification_deadline();
        tokio::select! {
            read = stream.read(&mut buffers.read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                record_rx(read, meters);
                let mut offset = 0;
                while offset < read {
                    if let Some((command, payload)) = buffers
                        .decoder
                        .feed_slice_next(&buffers.read_buf[..read], &mut offset)
                        .ok()
                        .flatten()
                    {
                        match command & 0x0f {
                            kiss_framing::CMD_DATA if !payload.is_empty() => {
                                seam.next_inbound(payload).await;
                            }
                            kiss_framing::CMD_READY => {
                                if let Some(transmission) =
                                    control.ready_received(elapsed_millis(meters.started))
                                {
                                    if !write_transmission(
                                        stream,
                                        &mut buffers.frame_buf,
                                        control,
                                        transmission,
                                        meters,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Some(transmission) =
                    control.accept_packet(outbound, elapsed_millis(meters.started))
                {
                    if !write_transmission(
                        stream,
                        &mut buffers.frame_buf,
                        control,
                        transmission,
                        meters,
                    )
                    .await
                    {
                        return;
                    }
                }
            }
            () = wait_for_deadline(meters.started, flow_deadline) => {
                if let Some(transmission) =
                    control.flow_timeout_elapsed(elapsed_millis(meters.started))
                {
                    if !write_transmission(
                        stream,
                        &mut buffers.frame_buf,
                        control,
                        transmission,
                        meters,
                    )
                    .await
                    {
                        return;
                    }
                }
            }
            () = wait_for_deadline(meters.started, station_deadline) => {
                if let Some(transmission) =
                    control.station_identification_elapsed(elapsed_millis(meters.started))
                {
                    if !write_transmission(
                        stream,
                        &mut buffers.frame_buf,
                        control,
                        transmission,
                        meters,
                    )
                    .await
                    {
                        return;
                    }
                }
            }
        }
    }
}

async fn write_transmission<S: AsyncWrite + Unpin>(
    stream: &mut S,
    frame_buf: &mut [u8],
    control: &mut KissTransmissionControl,
    transmission: Transmission,
    meters: &mut WireMeters<'_>,
) -> bool {
    let Ok(framed) = kiss_framing::encode(transmission.payload(), frame_buf) else {
        return true;
    };
    if stream.write_all(&frame_buf[..framed]).await.is_err() {
        return false;
    }
    let now = elapsed_millis(meters.started);
    control.transmitted(&transmission, now);
    record_tx(framed, meters);
    true
}

fn record_rx(read: usize, meters: &mut WireMeters<'_>) {
    meters.status.add_rx(read as u64);
    let now = InstantMillis(meters.started.elapsed().as_millis() as u64);
    meters.throughput.record_rx(now, read as u64);
    meters.status.set_transfer_rates(meters.throughput.rates());
}

fn record_tx(written: usize, meters: &mut WireMeters<'_>) {
    meters.status.add_tx(written as u64);
    let now = InstantMillis(meters.started.elapsed().as_millis() as u64);
    meters.throughput.record_tx(now, written as u64);
    meters.status.set_transfer_rates(meters.throughput.rates());
    meters.status.set_airtime(
        meters
            .airtime
            .record_tx(now, frame_airtime_us(written, meters.bitrate)),
    );
}

impl<Open, Fut, S> Interface for KissInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = contract::KISS_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Kiss;

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
        let mut control =
            KissTransmissionControl::new(self.flow_control, self.station_identification);
        let mut buffers: Option<
            ControlledKissBuffers<
                { contract::KISS_FRAME_LEN },
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
                // A configuration write failure follows the same reconnect path as a mid-serve drop.
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

impl<Open> prns_core::interfaces::ReportsStatus for KissInterface<Open> {
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
    use prns_core::interfaces::kiss::{ReadyTimeout, StationIdInterval, StationIdWireFormat};
    use prns_core::interfaces::kiss_framing::{self, FEND, FESC};
    use prns_core::interfaces::InterfaceStatus;
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
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

    async fn read_kiss_payload(wire: &mut tokio::io::DuplexStream) -> Vec<u8> {
        let mut decoder = contract::Decoder::new();
        let mut buf = [0u8; 64];
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
    async fn configures_the_tnc_then_frames_and_deframes_kiss_across_a_real_async_stream() {
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::KISS_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let interface = KissInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            TncConfigureDelay::new(Duration::ZERO),
            TncConfig::default(),
            b"test-kiss",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        let mut config = [0u8; 20];
        tokio::time::timeout(Duration::from_secs(2), test_wire.read_exact(&mut config))
            .await
            .expect("the config frames arrive within the window")
            .expect("the wire stays up through the config write");
        assert_eq!(
            config,
            [
                FEND,
                kiss_framing::CMD_TXDELAY,
                35,
                FEND, // preamble 350 ms / 10
                FEND,
                kiss_framing::CMD_TXTAIL,
                2,
                FEND, // tx-tail 20 ms / 10
                FEND,
                kiss_framing::CMD_P,
                64,
                FEND, // persistence
                FEND,
                kiss_framing::CMD_SLOTTIME,
                2,
                FEND, // slot time 20 ms / 10
                FEND,
                kiss_framing::CMD_READY,
                1,
                FEND, // flow-control ready
            ]
        );

        let payload = [0x01u8, 0x02, FEND, FESC, 0x03];
        let mut framed = [0u8; 32];
        let n = kiss_framing::encode(&payload, &mut framed).expect("encodes the payload");
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
            "the interface deframes inbound KISS frames"
        );

        let out_payload = [0xAAu8, FEND, 0xBB];
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
            "the interface frames outbound packets"
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
    async fn ready_flow_control_queues_and_recovers_from_a_missed_ready() {
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::KISS_FRAME_LEN, 4);
        let seam = MockSeam {
            inbound: in_tx,
            sink: Vec::new(),
            outbound: out_rx,
        };
        let interface = KissInterface::with_runtime_settings(
            open,
            ReconnectPolicy::STANDARD,
            KissSettings {
                configure_delay: TncConfigureDelay::new(Duration::ZERO),
                tnc: TncConfig::default(),
                flow_control: ReadyCommandFlowControl::WaitForReadyOrTimeout(ReadyTimeout::new(
                    Duration::from_millis(50),
                )),
                station_identification: None,
                policy: contract::configured_policy(Default::default()),
                channel_tag: b"flow-kiss",
            },
        );
        tokio::spawn(interface.run(seam));
        let mut config = [0u8; 20];
        test_wire
            .read_exact(&mut config)
            .await
            .expect("reads config");

        out_tx.try_grant().expect("first slot").fill(b"first");
        out_tx.commit();
        assert_eq!(read_kiss_payload(&mut test_wire).await, b"first");

        out_tx.try_grant().expect("second slot").fill(b"second");
        out_tx.commit();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), read_kiss_payload(&mut test_wire))
                .await
                .is_err()
        );
        test_wire
            .write_all(&kiss_framing::command_frame(kiss_framing::CMD_READY, 1))
            .await
            .expect("writes ready");
        assert_eq!(read_kiss_payload(&mut test_wire).await, b"second");

        out_tx.try_grant().expect("third slot").fill(b"third");
        out_tx.commit();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), read_kiss_payload(&mut test_wire))
                .await
                .expect("flow timeout releases the queue"),
            b"third"
        );
    }

    #[tokio::test]
    async fn station_identification_is_padded_and_sent_after_ordinary_traffic() {
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(contract::KISS_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: Vec::new(),
            outbound: out_rx,
        };
        let station_identification = StationIdentification::new(
            b"N0CALL",
            StationIdInterval::new(Duration::from_millis(20)),
            StationIdWireFormat::KissPadded,
        )
        .expect("valid station identification");
        let interface = KissInterface::with_runtime_settings(
            open,
            ReconnectPolicy::STANDARD,
            KissSettings {
                configure_delay: TncConfigureDelay::new(Duration::ZERO),
                tnc: TncConfig::default(),
                flow_control: ReadyCommandFlowControl::Disabled,
                station_identification: Some(station_identification),
                policy: contract::configured_policy(Default::default()),
                channel_tag: b"station-kiss",
            },
        );
        tokio::spawn(interface.run(seam));
        let mut config = [0u8; 20];
        test_wire
            .read_exact(&mut config)
            .await
            .expect("reads config");

        out_tx.try_grant().expect("packet slot").fill(b"packet");
        out_tx.commit();
        assert_eq!(read_kiss_payload(&mut test_wire).await, b"packet");
        let station =
            tokio::time::timeout(Duration::from_secs(1), read_kiss_payload(&mut test_wire))
                .await
                .expect("station identification arrives");
        assert_eq!(station.len(), 15);
        assert_eq!(&station[..6], b"N0CALL");
        assert!(station[6..].iter().all(|byte| *byte == 0));
    }
}
