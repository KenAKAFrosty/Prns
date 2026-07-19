use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::framed_stream::WireMeters;
use crate::kiss_deadline::{elapsed_millis, instant_for, wait_for_deadline};
use crate::reconnect::ReconnectPolicy;
use prns_core::engine::InstantMillis;
use prns_core::interfaces::kiss::transmission_control::{
    KissTransmissionControl, ReadyCommandFlowControl, StationIdentification, Transmission,
};
use prns_core::interfaces::rnode::bring_up::{
    BringUp as BringUpProtocol, BringUpAction, BringUpError,
};
use prns_core::interfaces::rnode::core::{self, RadioConfig};
use prns_core::interfaces::rnode::live::{KeepaliveSchedule, LiveCommand, LiveProtocol};
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::reactor::driver::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::reactor::throughput::ThroughputLedger;

/// How long to wait after opening the port before driving the device, mirroring RNS
/// `configure_device`'s `reset_radio_state()` + `sleep(2.0)`: a freshly opened RNode needs a moment
/// to settle, and bytes written into that window are lost. Tests pass `Duration::ZERO`.
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

/// How long to wait for the device to answer the detect query before giving up on this connection
/// and reconnecting. RNS's serial path waits a fixed `0.2s`; a slightly longer window is more
/// forgiving of a device still settling without changing the requirement that it *must* answer.
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
    decoder: Box<core::CommandDecoder>,
    read: Box<[u8]>,
    frame: Box<[u8]>,
}

impl RNodeBuffers {
    fn new() -> Self {
        Self {
            decoder: Box::new(core::CommandDecoder::new()),
            read: vec![0u8; core::READ_BUF_LEN].into_boxed_slice(),
            frame: vec![0u8; core::FRAMED_LEN].into_boxed_slice(),
        }
    }
}

/// A host RNode interface (RNS `RNodeInterface` parity): Reticulum packets carried by a LoRa
/// RNode over a serial, TCP, or BLE byte stream. It owns its medium's whole lifecycle, but each
/// connection first runs the RNode bring-up handshake (detect, write the radio configuration,
/// validate the device's echoes) and only then pumps `CMD_DATA` frames; a bring-up that fails
/// drops the link and retries, exactly as RNS closes the port and reconnects.
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
    /// Build with RNS's default reset-settle delay. `channel_tag` names *which* host transport
    /// this is: the same endpoint across a reopen passes the same
    /// bytes so its routes survive. The radio determines the bitrate and bring-up configuration.
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

    /// Build with an explicit reset-settle delay — the daemon supplies the configured device, and
    /// tests pass `Duration::ZERO` to skip the real two-second RNode settle.
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
            core::policy_for_bitrate(bitrate),
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
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id, derived from its device `channel_tag`, for the app that wants to name it.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this interface's live-status handle for the app to read on its own render cadence.
    /// Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

/// Run the RNode bring-up handshake on a freshly opened link, returning once the radio is
/// configured and validated. Any IO error, detect timeout, or parameter mismatch is reported
/// so the run loop drops the link and reconnects, mirroring RNS `configure_device`. The
/// `decoder` and `read_buf` are the run loop's, reused across reconnects.
async fn bring_up<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    radio: &RadioConfig,
    decoder: &mut core::CommandDecoder,
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
                    eprintln!(
                        "RNODE_FIRMWARE_OUTDATED reported={}.{} required={}.{} (continuing anyway)",
                        firmware.major,
                        firmware.minor,
                        core::REQUIRED_FW_VER_MAJ,
                        core::REQUIRED_FW_VER_MIN,
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

/// Serve one configured connection until the stream drops: deliver `CMD_DATA` bodies to the
/// seam (consuming telemetry and other commands) and frame the seam's outbound as `CMD_DATA`.
/// Distinct from the generic [`framed_stream::serve`](crate::framed_stream) because the read
/// side dispatches by command rather than treating every frame as data.
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
    let Ok(framed) = core::encode_data_frame(transmission.payload(), frame_buf) else {
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
    const HW_MTU: usize = core::RNODE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Rnode;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.policy)
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
        // The decoder and buffers are heap-held and reused across reconnects — no megabyte of buffer
        // rides the stack, and a device that never answers allocates these exactly once.
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
    use prns_core::interfaces::kiss::transmission_control::{
        StationIdInterval, StationIdWireFormat,
    };
    use prns_core::interfaces::kiss_framing::{self, FEND};
    use prns_core::interfaces::{
        InterfaceStatus, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
    };
    use prns_runtime::reactor::driver::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a grant
    /// lane the test fills — so the interface's framing and data path can be exercised in isolation.
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
        RadioConfig::new(core::RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            txpower_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        })
        .expect("a valid radio config")
    }

    /// Read decoded device-bound `(command, payload)` frames off the wire until `wanted` of them have
    /// arrived, so the test can assert what the interface wrote to the "device".
    async fn read_commands<R: AsyncRead + Unpin>(
        wire: &mut R,
        wanted: usize,
    ) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
        let mut decoder: core::CommandDecoder = core::CommandDecoder::new();
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

    /// Write one device-bound command frame onto the wire (the device's echo back to the host).
    async fn write_command<W: AsyncWrite + Unpin>(wire: &mut W, command: u8, payload: &[u8]) {
        let mut framed = [0u8; 64];
        let n = kiss_framing::encode_with_command(command, payload, &mut framed)
            .expect("encodes the device frame");
        wire.write_all(&framed[..n]).await.expect("writes the echo");
    }

    /// Play a cooperative RNode: answer detect, then echo back exactly the radio parameters the host
    /// configured, so the host's bring-up validates and the link comes online.
    async fn answer_bringup<RW: AsyncRead + AsyncWrite + Unpin>(
        wire: &mut RW,
        radio: &RadioConfig,
    ) {
        // The host writes the four detect frames first; consume them, then answer detect + firmware.
        let detect = read_commands(wire, 4).await;
        assert_eq!(detect[0], (core::CMD_DETECT, std::vec![core::DETECT_REQ]));
        write_command(wire, core::CMD_DETECT, &[core::DETECT_RESP]).await;
        write_command(wire, core::CMD_FW_VERSION, &[1, 80]).await;

        // Then the host writes the radio configuration; consume the six config frames and echo each
        // back as the device would report it, ending with the radio powered on.
        let config = read_commands(wire, 6).await;
        assert_eq!(
            config[0],
            (
                core::CMD_FREQUENCY,
                radio.frequency_hz().to_be_bytes().to_vec()
            )
        );
        assert_eq!(
            config[5],
            (core::CMD_RADIO_STATE, std::vec![core::RADIO_STATE_ON])
        );
        write_command(
            wire,
            core::CMD_FREQUENCY,
            &radio.frequency_hz().to_be_bytes(),
        )
        .await;
        write_command(
            wire,
            core::CMD_BANDWIDTH,
            &radio.bandwidth_hz().to_be_bytes(),
        )
        .await;
        write_command(wire, core::CMD_TXPOWER, &[radio.txpower_dbm()]).await;
        write_command(wire, core::CMD_SF, &[radio.spreading_factor()]).await;
        write_command(wire, core::CMD_CR, &[radio.coding_rate()]).await;
        write_command(wire, core::CMD_RADIO_STATE, &[core::RADIO_STATE_ON]).await;
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
        let (mut out_tx, out_rx) = tokio_grant_lane(core::RNODE_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let radio = sample_radio();
        // settle = ZERO so the test does not wait the real two-second RNode reset delay.
        let interface = RNodeInterface::with_settings(
            open,
            ReconnectPolicy::STANDARD,
            RNodeResetDelay::new(Duration::ZERO),
            radio,
            b"test-rnode",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        // The device plays its side of the bring-up handshake; the link should come online.
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

        // Inbound: a CMD_DATA frame (FEND/FESC in the payload exercise the escaping) crosses the wire
        // and lands deframed at the seam; the firmware/telemetry commands around it are consumed.
        let payload = [0x01u8, 0x02, FEND, kiss_framing::FESC, 0x03];
        write_command(&mut device, core::CMD_STAT_RSSI, &[74]).await;
        write_command(&mut device, core::CMD_STAT_SNR, &[0xf7]).await;
        write_command(&mut device, core::CMD_DATA, &payload).await;
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

        write_command(&mut device, core::CMD_DATA, b"next").await;
        let (_, packet_phy) = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the next frame arrives within the window")
            .expect("the interface task is alive");
        assert_eq!(packet_phy, PacketPhyStats::default());

        // Outbound: the seam yields a frame; the interface frames it as CMD_DATA onto the wire.
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
            (core::CMD_DATA, out_payload.to_vec()),
            "the interface frames outbound packets as CMD_DATA"
        );

        // The live status reflects the connection and bytes both ways.
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
        let (mut out_tx, out_rx) = tokio_grant_lane(core::RNODE_FRAME_LEN, 4);
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
                policy: core::policy_for_bitrate(BitrateBps::guess(u64::from(
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
            (core::CMD_DATA, b"first".to_vec())
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
            (core::CMD_DATA, b"second".to_vec())
        );
        write_command(&mut device, kiss_framing::CMD_READY, &[1]).await;
        let station = tokio::time::timeout(Duration::from_secs(1), read_commands(&mut device, 1))
            .await
            .expect("station identification arrives");
        assert_eq!(station[0], (core::CMD_DATA, b"N0CALL".to_vec()));
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
        let (_out_tx, out_rx) = tokio_grant_lane(core::RNODE_FRAME_LEN, 2);
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

        // Answer detect, but echo a spreading factor that does not match the configuration.
        let _detect = read_commands(&mut device, 4).await;
        write_command(&mut device, core::CMD_DETECT, &[core::DETECT_RESP]).await;
        write_command(&mut device, core::CMD_FW_VERSION, &[1, 80]).await;
        let _config = read_commands(&mut device, 6).await;
        write_command(
            &mut device,
            core::CMD_FREQUENCY,
            &radio.frequency_hz().to_be_bytes(),
        )
        .await;
        write_command(
            &mut device,
            core::CMD_BANDWIDTH,
            &radio.bandwidth_hz().to_be_bytes(),
        )
        .await;
        write_command(&mut device, core::CMD_TXPOWER, &[radio.txpower_dbm()]).await;
        write_command(&mut device, core::CMD_SF, &[radio.spreading_factor() + 1]).await;
        write_command(&mut device, core::CMD_CR, &[radio.coding_rate()]).await;
        write_command(&mut device, core::CMD_RADIO_STATE, &[core::RADIO_STATE_ON]).await;

        // The interface must never report Connected on a mismatched bring-up; give it room to try and
        // confirm it stayed out of the online state.
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
        let (_out_tx, out_rx) = tokio_grant_lane(core::RNODE_FRAME_LEN, 1);
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
