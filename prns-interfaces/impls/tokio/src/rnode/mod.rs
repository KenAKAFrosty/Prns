mod ble;
#[cfg(feature = "config")]
pub(crate) mod host;
pub mod multi;

use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::byte_stream::deadline::{elapsed_millis, instant_for, wait_for_deadline};
use crate::byte_stream::framing::WireMeters;
use crate::reconnect::ReconnectPolicy;
use prns_core::engine::InstantMillis;
use prns_core::interfaces::kiss::{
    KissTransmissionControl, ReadyCommandFlowControl, StationIdentification, Transmission,
};
use prns_core::interfaces::rnode::bring_up::{
    BringUp as BringUpProtocol, BringUpAction, BringUpError,
};
use prns_core::interfaces::rnode::live::{KeepaliveSchedule, LiveCommand, LiveProtocol};
use prns_core::interfaces::rnode::policy;
use prns_core::interfaces::rnode::protocol::{self, RadioConfig};
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

/// A freshly opened RNode needs the RNS two-second settle window before it will accept configuration bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RNodeResetDelay(Duration);

impl RNodeResetDelay {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

pub const DEFAULT_RNODE_RESET_DELAY: RNodeResetDelay = RNodeResetDelay::new(Duration::from_secs(2));

/// A detect timeout drops the connection and reconnects; the default is slightly longer than RNS's fixed 200 ms serial window.
pub use prns_core::interfaces::rnode::bring_up::DetectTimeout as RNodeDetectTimeout;
pub use prns_core::interfaces::rnode::live::{
    Keepalive as RNodeKeepalive, KeepaliveInterval as RNodeKeepaliveInterval,
};

pub const DEFAULT_RNODE_DETECT_TIMEOUT: RNodeDetectTimeout =
    prns_core::interfaces::rnode::bring_up::DEFAULT_DETECT_TIMEOUT;
pub const TCP_RNODE_DETECT_TIMEOUT: RNodeDetectTimeout =
    prns_core::interfaces::rnode::bring_up::REMOTE_DETECT_TIMEOUT;
pub const BLE_RNODE_DETECT_TIMEOUT: RNodeDetectTimeout =
    prns_core::interfaces::rnode::bring_up::REMOTE_DETECT_TIMEOUT;
pub const TCP_RNODE_KEEPALIVE: RNodeKeepalive = prns_core::interfaces::rnode::live::TCP_KEEPALIVE;

struct RNodeBuffers {
    decoder: Box<protocol::CommandDecoder>,
    read: Box<[u8]>,
    frame: Box<[u8]>,
}

impl RNodeBuffers {
    fn new() -> Self {
        Self {
            decoder: Box::new(protocol::CommandDecoder::new()),
            read: vec![0u8; protocol::READ_BUF_LEN].into_boxed_slice(),
            frame: vec![0u8; protocol::FRAMED_LEN].into_boxed_slice(),
        }
    }
}

pub struct RNodeInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect_policy: ReconnectPolicy,
    reset_delay: RNodeResetDelay,
    detect_timeout: RNodeDetectTimeout,
    keepalive: RNodeKeepalive,
    radio: RadioConfig,
    flow_control: ReadyCommandFlowControl,
    station_identification: Option<StationIdentification>,
    policy: EffectiveInterfacePolicy,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

pub struct RNodeSettings<'a> {
    pub reset_delay: RNodeResetDelay,
    pub detect_timeout: RNodeDetectTimeout,
    pub keepalive: RNodeKeepalive,
    pub radio: RadioConfig,
    pub flow_control: ReadyCommandFlowControl,
    pub station_identification: Option<StationIdentification>,
    pub policy: EffectiveInterfacePolicy,
    pub channel_tag: &'a [u8],
}

impl<Open> RNodeInterface<Open> {
    /// A reopened endpoint must retain its `channel_tag`, and concurrent endpoints must have distinct tags.
    #[must_use]
    pub fn new(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        radio: RadioConfig,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_settings(
            open,
            reconnect_policy,
            DEFAULT_RNODE_RESET_DELAY,
            radio,
            channel_tag,
        )
    }

    #[must_use]
    pub fn new_with_policy(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        radio: RadioConfig,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_settings_and_policy(
            open,
            reconnect_policy,
            DEFAULT_RNODE_RESET_DELAY,
            radio,
            policy,
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_settings(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        reset_delay: RNodeResetDelay,
        radio: RadioConfig,
        channel_tag: &[u8],
    ) -> Self {
        let bitrate = BitrateBps::guess(u64::from(radio.nominal_bitrate_bps()));
        Self::with_settings_and_policy(
            open,
            reconnect_policy,
            reset_delay,
            radio,
            policy::policy_for_bitrate(bitrate),
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_settings_and_policy(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        reset_delay: RNodeResetDelay,
        radio: RadioConfig,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_runtime_settings(
            open,
            reconnect_policy,
            RNodeSettings {
                reset_delay,
                detect_timeout: DEFAULT_RNODE_DETECT_TIMEOUT,
                keepalive: RNodeKeepalive::Disabled,
                radio,
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
        settings: RNodeSettings<'_>,
    ) -> Self {
        let channel_tag = settings.channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Rnode, &channel_tag);
        Self {
            id,
            open,
            reconnect_policy,
            reset_delay: settings.reset_delay,
            detect_timeout: settings.detect_timeout,
            keepalive: settings.keepalive,
            radio: settings.radio,
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

async fn bring_up<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    radio: &RadioConfig,
    decoder: &mut protocol::CommandDecoder,
    read_buf: &mut [u8],
    detect_timeout: RNodeDetectTimeout,
) -> io::Result<()> {
    decoder.reset();
    let started = tokio::time::Instant::now();
    let mut protocol = BringUpProtocol::new(*radio, detect_timeout);
    loop {
        match protocol.next_action(elapsed_millis(started)) {
            BringUpAction::WriteDetect(bytes) => stream.write_all(&bytes).await?,
            BringUpAction::WriteRadioConfiguration {
                bytes,
                outdated_firmware,
            } => {
                if let Some(firmware) = outdated_firmware {
                    crate::diagnostic_log::warn!(
                        "RNODE_FIRMWARE_OUTDATED reported={}.{} required={}.{} (continuing anyway)",
                        firmware.major,
                        firmware.minor,
                        protocol::REQUIRED_FW_VER_MAJ,
                        protocol::REQUIRED_FW_VER_MIN,
                    );
                }
                stream.write_all(&bytes).await?;
            }
            BringUpAction::ReadUntil(deadline) => {
                let Some(deadline) = instant_for(started, deadline) else {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "RNode bring-up deadline exceeds the host clock range",
                    ));
                };
                match tokio::time::timeout_at(deadline, stream.read(read_buf)).await {
                    Err(_) => protocol.deadline_elapsed(elapsed_millis(started)),
                    Ok(Ok(0)) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                    Ok(Ok(read)) => {
                        decoder.feed_slice(&read_buf[..read], |command, payload| {
                            protocol.apply_command(command, payload);
                        });
                    }
                    Ok(Err(error)) => return Err(error),
                }
            }
            BringUpAction::Complete => return Ok(()),
            BringUpAction::Failed(error) => return Err(bring_up_error(error)),
        }
    }
}

fn bring_up_error(error: BringUpError) -> io::Error {
    match error {
        BringUpError::DetectTimedOut => io::Error::new(
            io::ErrorKind::TimedOut,
            "RNode did not answer the detect query",
        ),
        BringUpError::RadioMismatch => io::Error::new(
            io::ErrorKind::InvalidData,
            "RNode reported radio parameters that do not match the configuration",
        ),
    }
}

async fn serve_rnode<S, Seam>(
    stream: &mut S,
    radio: &RadioConfig,
    buffers: &mut RNodeBuffers,
    seam: &mut Seam,
    control: &mut KissTransmissionControl,
    keepalive: RNodeKeepalive,
    meters: &mut WireMeters<'_>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let mut protocol = LiveProtocol::default();
    buffers.decoder.reset();
    control.connection_opened();
    let mut keepalive = KeepaliveSchedule::new(keepalive, elapsed_millis(meters.started));
    loop {
        if let Some(transmission) = control.next_queued(elapsed_millis(meters.started)) {
            if !write_rnode_transmission(
                stream,
                &mut buffers.frame,
                control,
                &mut keepalive,
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
            read = stream.read(&mut buffers.read) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                record_rnode_rx(read, meters);
                let mut offset = 0;
                let chunk = &buffers.read[..read];
                while offset < chunk.len() {
                    if let Some((command, payload)) =
                        buffers.decoder.feed_slice_next(chunk, &mut offset).ok().flatten()
                    {
                        match protocol.apply(command, payload, radio) {
                            LiveCommand::Data { payload, phy } => {
                                seam.next_inbound_with_phy(payload, phy).await;
                            }
                            LiveCommand::Ready => {
                                if let Some(transmission) =
                                    control.ready_received(elapsed_millis(meters.started))
                                {
                                    if !write_rnode_transmission(
                                        stream,
                                        &mut buffers.frame,
                                        control,
                                        &mut keepalive,
                                        transmission,
                                        meters,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                }
                            }
                            LiveCommand::Consumed => {}
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Some(transmission) =
                    control.accept_packet(outbound, elapsed_millis(meters.started))
                {
                    if !write_rnode_transmission(
                        stream,
                        &mut buffers.frame,
                        control,
                        &mut keepalive,
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
                    if !write_rnode_transmission(
                        stream,
                        &mut buffers.frame,
                        control,
                        &mut keepalive,
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
                    if !write_rnode_transmission(
                        stream,
                        &mut buffers.frame,
                        control,
                        &mut keepalive,
                        transmission,
                        meters,
                    )
                    .await
                    {
                        return;
                    }
                }
            }
            () = wait_for_deadline(meters.started, keepalive.deadline()) => {
                let now = elapsed_millis(meters.started);
                let Some(transmission) = keepalive.due(now) else {
                    continue;
                };
                if stream.write_all(transmission.wire_bytes()).await.is_err() {
                    return;
                }
                keepalive.wrote(elapsed_millis(meters.started));
            }
        }
    }
}

async fn write_rnode_transmission<S: AsyncWrite + Unpin>(
    stream: &mut S,
    frame_buf: &mut [u8],
    control: &mut KissTransmissionControl,
    keepalive: &mut KeepaliveSchedule,
    transmission: Transmission,
    meters: &mut WireMeters<'_>,
) -> bool {
    let Ok(framed) = protocol::encode_data_frame(transmission.payload(), frame_buf) else {
        return true;
    };
    if stream.write_all(&frame_buf[..framed]).await.is_err() {
        return false;
    }
    let now = elapsed_millis(meters.started);
    keepalive.wrote(now);
    control.transmitted(&transmission, now);
    meters.status.add_tx(framed as u64);
    let elapsed = InstantMillis(meters.started.elapsed().as_millis() as u64);
    meters.throughput.record_tx(elapsed, framed as u64);
    meters.status.set_transfer_rates(meters.throughput.rates());
    meters.status.set_airtime(
        meters
            .airtime
            .record_tx(elapsed, frame_airtime_us(framed, meters.bitrate)),
    );
    true
}

fn record_rnode_rx(read: usize, meters: &mut WireMeters<'_>) {
    meters.status.add_rx(read as u64);
    let now = InstantMillis(meters.started.elapsed().as_millis() as u64);
    meters.throughput.record_rx(now, read as u64);
    meters.status.set_transfer_rates(meters.throughput.rates());
}

impl<Open, Fut, S> Interface for RNodeInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = policy::RNODE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Rnode;

    fn descriptor(&self) -> InterfaceDescriptor {
        policy::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut control =
            KissTransmissionControl::new(self.flow_control, self.station_identification);
        // Decoder buffers are heap-held and reused across reconnects so they do not inflate the stack.
        let mut buffers = RNodeBuffers::new();
        let mut reconnect = self.reconnect_policy.schedule();
        loop {
            self.status.set_connection(ConnectionState::Reconnecting);
            let mut stream = match (self.open)().await {
                Ok(stream) => stream,
                Err(error) => {
                    let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                    crate::diagnostic_log::warn!(
                        "RNode interface {:?} could not open: {error}; retrying in {} seconds",
                        self.id.as_bytes(),
                        reconnect_delay.as_secs_f64(),
                    );
                    tokio::time::sleep(reconnect_delay).await;
                    continue;
                }
            };
            if !self.reset_delay.duration().is_zero() {
                tokio::time::sleep(self.reset_delay.duration()).await;
            }
            if let Err(error) = bring_up(
                &mut stream,
                &self.radio,
                &mut buffers.decoder,
                &mut buffers.read,
                self.detect_timeout,
            )
            .await
            {
                let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                crate::diagnostic_log::warn!(
                    "RNode interface {:?} bring-up failed: {error}; retrying in {} seconds",
                    self.id.as_bytes(),
                    reconnect_delay.as_secs_f64(),
                );
                tokio::time::sleep(reconnect_delay).await;
                continue;
            }
            let connected_at = tokio::time::Instant::now();
            self.status.set_connection(ConnectionState::Connected);
            serve_rnode(
                &mut stream,
                &self.radio,
                &mut buffers,
                &mut seam,
                &mut control,
                self.keepalive,
                &mut WireMeters {
                    status: &self.status,
                    airtime: &mut airtime,
                    throughput: &mut throughput,
                    bitrate: self.policy.bitrate,
                    started,
                },
            )
            .await;
            self.status.set_connection(ConnectionState::Reconnecting);
            reconnect.record_connection_lifetime(connected_at.elapsed());
            let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            crate::diagnostic_log::warn!(
                "RNode interface {:?} connection closed; retrying in {} seconds",
                self.id.as_bytes(),
                reconnect_delay.as_secs_f64(),
            );
            tokio::time::sleep(reconnect_delay).await;
        }
    }
}

impl<Open> prns_core::interfaces::ReportsStatus for RNodeInterface<Open> {
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
    use prns_core::interfaces::kiss::{StationIdInterval, StationIdWireFormat};
    use prns_core::interfaces::kiss_framing::{self, FEND};
    use prns_core::interfaces::{
        InterfaceStatus, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
    };
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::sync::mpsc::{self, UnboundedSender};

    struct MockSeam {
        inbound: UnboundedSender<(std::vec::Vec<u8>, PacketPhyStats)>,
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
                let _ = self
                    .inbound
                    .send((std::mem::take(&mut self.sink), PacketPhyStats::default()));
            }
        }

        async fn next_inbound_with_phy(&mut self, frame: &[u8], phy: PacketPhyStats) {
            let _ = self.inbound.send((frame.to_vec(), phy));
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
    }

    fn sample_radio() -> RadioConfig {
        RadioConfig::new(protocol::RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            tx_power_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        })
        .expect("a valid radio config")
    }

    async fn read_commands<R: AsyncRead + Unpin>(
        wire: &mut R,
        wanted: usize,
    ) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
        let mut decoder: protocol::CommandDecoder = protocol::CommandDecoder::new();
        let mut buf = [0u8; 256];
        let mut frames = std::vec::Vec::new();
        while frames.len() < wanted {
            let n = wire.read(&mut buf).await.expect("reads from the wire");
            assert_ne!(n, 0, "the wire closed before {wanted} frames arrived");
            let mut offset = 0;
            while offset < n {
                if let Some((command, payload)) = decoder
                    .feed_slice_next(&buf[..n], &mut offset)
                    .ok()
                    .flatten()
                {
                    frames.push((command, payload.to_vec()));
                }
            }
        }
        frames
    }

    async fn write_command<W: AsyncWrite + Unpin>(wire: &mut W, command: u8, payload: &[u8]) {
        let mut framed = [0u8; 64];
        let n = kiss_framing::encode_with_command(command, payload, &mut framed)
            .expect("encodes the device frame");
        wire.write_all(&framed[..n]).await.expect("writes the echo");
    }

    async fn answer_bringup<RW: AsyncRead + AsyncWrite + Unpin>(
        wire: &mut RW,
        radio: &RadioConfig,
    ) {
        let detect = read_commands(wire, 4).await;
        assert_eq!(
            detect[0],
            (protocol::CMD_DETECT, std::vec![protocol::DETECT_REQ])
        );
        write_command(wire, protocol::CMD_DETECT, &[protocol::DETECT_RESP]).await;
        write_command(wire, protocol::CMD_FW_VERSION, &[1, 80]).await;

        let config = read_commands(wire, 6).await;
        assert_eq!(
            config[0],
            (
                protocol::CMD_FREQUENCY,
                radio.frequency_hz().to_be_bytes().to_vec()
            )
        );
        assert_eq!(
            config[5],
            (
                protocol::CMD_RADIO_STATE,
                std::vec![protocol::RADIO_STATE_ON]
            )
        );
        write_command(
            wire,
            protocol::CMD_FREQUENCY,
            &radio.frequency_hz().to_be_bytes(),
        )
        .await;
        write_command(
            wire,
            protocol::CMD_BANDWIDTH,
            &radio.bandwidth_hz().to_be_bytes(),
        )
        .await;
        write_command(wire, protocol::CMD_TXPOWER, &[radio.tx_power_dbm()]).await;
        write_command(wire, protocol::CMD_SF, &[radio.spreading_factor()]).await;
        write_command(wire, protocol::CMD_CR, &[radio.coding_rate()]).await;
        write_command(wire, protocol::CMD_RADIO_STATE, &[protocol::RADIO_STATE_ON]).await;
    }

    #[tokio::test]
    async fn brings_up_the_radio_then_frames_and_deframes_data() {
        let (interface_wire, mut device) = tokio::io::duplex(4096);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<(std::vec::Vec<u8>, PacketPhyStats)>();
        let (mut out_tx, out_rx) = tokio_grant_lane(protocol::RNODE_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let radio = sample_radio();
        let interface = RNodeInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            RNodeResetDelay::new(Duration::ZERO),
            radio,
            b"test-rnode",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        tokio::time::timeout(Duration::from_secs(2), answer_bringup(&mut device, &radio))
            .await
            .expect("the bring-up handshake completes within the window");
        tokio::time::timeout(Duration::from_secs(2), async {
            while status.connection() != ConnectionState::Connected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the interface comes online after a valid bring-up");

        let payload = [0x01u8, 0x02, FEND, kiss_framing::FESC, 0x03];
        write_command(&mut device, protocol::CMD_STAT_RSSI, &[74]).await;
        write_command(&mut device, protocol::CMD_STAT_SNR, &[0xf7]).await;
        write_command(&mut device, protocol::CMD_DATA, &payload).await;
        let (received, packet_phy) = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(
            received, payload,
            "the interface deframes inbound CMD_DATA frames"
        );
        assert_eq!(
            packet_phy,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-83)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(515),
            }
        );

        write_command(&mut device, protocol::CMD_DATA, b"next").await;
        let (_, packet_phy) = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the next frame arrives within the window")
            .expect("the interface task is alive");
        assert_eq!(packet_phy, PacketPhyStats::default());

        let out_payload = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let framed = tokio::time::timeout(Duration::from_secs(2), read_commands(&mut device, 1))
            .await
            .expect("the interface frames outbound within the window");
        assert_eq!(
            framed[0],
            (protocol::CMD_DATA, out_payload.to_vec()),
            "the interface frames outbound packets as CMD_DATA"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.rx_bytes() > 0
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

        drop(device);
        tokio::time::timeout(Duration::from_secs(2), async {
            while status.connection() != ConnectionState::Reconnecting {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a closed RNode wire enters the reconnecting state");
    }

    #[tokio::test]
    async fn ready_flow_control_and_station_identification_share_the_rnode_queue() {
        let (interface_wire, mut device) = tokio::io::duplex(4096);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<(Vec<u8>, PacketPhyStats)>();
        let (mut out_tx, out_rx) = tokio_grant_lane(protocol::RNODE_FRAME_LEN, 4);
        let seam = MockSeam {
            inbound: in_tx,
            sink: Vec::new(),
            outbound: out_rx,
        };
        let radio = sample_radio();
        let station_identification = StationIdentification::new(
            b"N0CALL",
            StationIdInterval::new(Duration::from_millis(100)),
            StationIdWireFormat::Exact,
        )
        .expect("valid station identification");
        let interface = RNodeInterface::with_runtime_settings(
            open,
            ReconnectPolicy::STANDARD,
            RNodeSettings {
                reset_delay: RNodeResetDelay::new(Duration::ZERO),
                detect_timeout: DEFAULT_RNODE_DETECT_TIMEOUT,
                keepalive: RNodeKeepalive::Disabled,
                radio,
                flow_control: ReadyCommandFlowControl::WaitForReady,
                station_identification: Some(station_identification),
                policy: policy::policy_for_bitrate(BitrateBps::guess(u64::from(
                    radio.nominal_bitrate_bps(),
                ))),
                channel_tag: b"controlled-rnode",
            },
        );
        tokio::spawn(interface.run(seam));
        tokio::time::timeout(Duration::from_secs(2), answer_bringup(&mut device, &radio))
            .await
            .expect("bring-up completes");

        out_tx.try_grant().expect("first slot").fill(b"first");
        out_tx.commit();
        assert_eq!(
            read_commands(&mut device, 1).await[0],
            (protocol::CMD_DATA, b"first".to_vec())
        );

        out_tx.try_grant().expect("second slot").fill(b"second");
        out_tx.commit();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), read_commands(&mut device, 1))
                .await
                .is_err()
        );
        write_command(&mut device, kiss_framing::CMD_READY, &[1]).await;
        assert_eq!(
            read_commands(&mut device, 1).await[0],
            (protocol::CMD_DATA, b"second".to_vec())
        );
        write_command(&mut device, kiss_framing::CMD_READY, &[1]).await;
        let station = tokio::time::timeout(Duration::from_secs(1), read_commands(&mut device, 1))
            .await
            .expect("station identification arrives");
        assert_eq!(station[0], (protocol::CMD_DATA, b"N0CALL".to_vec()));
    }

    #[tokio::test]
    async fn refuses_to_come_online_when_the_radio_reports_a_mismatch() {
        let (interface_wire, mut device) = tokio::io::duplex(4096);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, _in_rx) = mpsc::unbounded_channel::<(std::vec::Vec<u8>, PacketPhyStats)>();
        let (_out_tx, out_rx) = tokio_grant_lane(protocol::RNODE_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let radio = sample_radio();
        let interface = RNodeInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            RNodeResetDelay::new(Duration::ZERO),
            radio,
            b"test-rnode",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        let _detect = read_commands(&mut device, 4).await;
        write_command(&mut device, protocol::CMD_DETECT, &[protocol::DETECT_RESP]).await;
        write_command(&mut device, protocol::CMD_FW_VERSION, &[1, 80]).await;
        let _config = read_commands(&mut device, 6).await;
        write_command(
            &mut device,
            protocol::CMD_FREQUENCY,
            &radio.frequency_hz().to_be_bytes(),
        )
        .await;
        write_command(
            &mut device,
            protocol::CMD_BANDWIDTH,
            &radio.bandwidth_hz().to_be_bytes(),
        )
        .await;
        write_command(&mut device, protocol::CMD_TXPOWER, &[radio.tx_power_dbm()]).await;
        write_command(
            &mut device,
            protocol::CMD_SF,
            &[radio.spreading_factor() + 1],
        )
        .await;
        write_command(&mut device, protocol::CMD_CR, &[radio.coding_rate()]).await;
        write_command(
            &mut device,
            protocol::CMD_RADIO_STATE,
            &[protocol::RADIO_STATE_ON],
        )
        .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_ne!(
            status.connection(),
            ConnectionState::Connected,
            "a parameter mismatch must abort bring-up, not bring the link online"
        );
    }

    #[tokio::test]
    async fn open_failures_are_visible_as_reconnecting() {
        let open = || async {
            Err::<tokio::io::DuplexStream, _>(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Bluetooth access denied",
            ))
        };
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<(Vec<u8>, PacketPhyStats)>();
        let (_out_tx, out_rx) = tokio_grant_lane(protocol::RNODE_FRAME_LEN, 1);
        let seam = MockSeam {
            inbound: in_tx,
            sink: Vec::new(),
            outbound: out_rx,
        };
        let interface = RNodeInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            RNodeResetDelay::new(Duration::ZERO),
            sample_radio(),
            b"failing-rnode",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));
        tokio::time::timeout(Duration::from_secs(1), async {
            while status.connection() != ConnectionState::Reconnecting {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the failed interface enters its reconnecting state");
    }
}
