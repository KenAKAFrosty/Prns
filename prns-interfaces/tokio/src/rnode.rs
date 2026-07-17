use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::framed_stream::WireMeters;
use prns_core::engine::InstantMillis;
use prns_core::interfaces::kiss_framing;
use prns_core::interfaces::rnode::core::{self, RadioConfig};
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
pub const RESET_SETTLE: Duration = Duration::from_secs(2);

/// How long to wait for the device to answer the detect query before giving up on this connection
/// and reconnecting. RNS's serial path waits a fixed `0.2s`; a slightly longer window is more
/// forgiving of a device still settling without changing the requirement that it *must* answer.
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait for the device to echo its radio parameters back after the configuration is
/// written, before validating whatever has arrived. RNS sleeps `0.25s` then checks; reading until
/// every parameter is reported (or this elapses) is the same check, just not wall-clock-bound.
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(2);

/// A host RNode interface (RNS `RNodeInterface` parity): Reticulum packets carried by a LoRa
/// RNode over USB serial. Like serial and KISS it owns its medium's whole lifecycle, but each
/// connection first runs the RNode bring-up handshake (detect, write the radio configuration,
/// validate the device's echoes) and only then pumps `CMD_DATA` frames; a bring-up that fails
/// drops the link and retries, exactly as RNS closes the port and reconnects.
pub struct RNodeInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
    settle: Duration,
    radio: RadioConfig,
    policy: EffectiveInterfacePolicy,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> RNodeInterface<Open> {
    /// Build with RNS's default reset-settle delay. `channel_tag` names *which* serial device
    /// this is, exactly as for serial and KISS: the same device across a reopen passes the same
    /// bytes so its routes survive. The radio determines the bitrate and bring-up configuration.
    #[must_use]
    pub fn new(open: Open, reconnect: Duration, radio: RadioConfig, channel_tag: &[u8]) -> Self {
        Self::with_settings(open, reconnect, RESET_SETTLE, radio, channel_tag)
    }

    #[must_use]
    pub fn new_with_policy(
        open: Open,
        reconnect: Duration,
        radio: RadioConfig,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        Self::with_settings_and_policy(open, reconnect, RESET_SETTLE, radio, policy, channel_tag)
    }

    /// Build with an explicit reset-settle delay — the daemon supplies the configured device, and
    /// tests pass `Duration::ZERO` to skip the real two-second RNode settle.
    #[must_use]
    pub fn with_settings(
        open: Open,
        reconnect: Duration,
        settle: Duration,
        radio: RadioConfig,
        channel_tag: &[u8],
    ) -> Self {
        let bitrate = BitrateBps::guess(u64::from(radio.nominal_bitrate_bps()));
        Self::with_settings_and_policy(
            open,
            reconnect,
            settle,
            radio,
            core::policy_for_bitrate(bitrate),
            channel_tag,
        )
    }

    #[must_use]
    pub fn with_settings_and_policy(
        open: Open,
        reconnect: Duration,
        settle: Duration,
        radio: RadioConfig,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Rnode, &channel_tag);
        Self {
            id,
            open,
            reconnect,
            settle,
            radio,
            policy,
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
) -> io::Result<()> {
    decoder.reset();
    let mut report = core::DeviceReport::default();

    // detect(): write the batched detect/firmware/platform/MCU query, then read until the device
    // answers the detect request (or we time out waiting for a device that is not an RNode).
    stream.write_all(&core::detect_frames()).await?;
    if !pump(
        stream,
        decoder,
        read_buf,
        &mut report,
        DETECT_TIMEOUT,
        |r| r.detected,
    )
    .await?
    {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "RNode did not answer the detect query",
        ));
    }
    if report.firmware_ok() == Some(false) {
        eprintln!(
            "RNODE_FIRMWARE_OUTDATED reported={}.{} required={}.{} (continuing anyway)",
            report.fw_maj.unwrap_or(0),
            report.fw_min.unwrap_or(0),
            core::REQUIRED_FW_VER_MAJ,
            core::REQUIRED_FW_VER_MIN,
        );
    }

    // initRadio(): write the radio configuration, then read the device's parameter echoes until all
    // have arrived (or the validation window elapses), and check them against the configuration.
    stream.write_all(&radio.init_command_bytes()).await?;
    pump(
        stream,
        decoder,
        read_buf,
        &mut report,
        VALIDATE_TIMEOUT,
        |r| r.all_radio_params_present(),
    )
    .await?;

    if report.radio_validated(radio) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RNode reported radio parameters that do not match the configuration",
        ))
    }
}

/// Read device frames into `report` until `done` is satisfied or `timeout` elapses. An IO
/// error or unexpected EOF propagates so bring-up treats the device as gone; a timeout returns
/// `Ok(false)` so the caller decides what an incomplete picture means (a missed detect aborts;
/// an incomplete validation simply fails the match).
async fn pump<S, Done>(
    stream: &mut S,
    decoder: &mut core::CommandDecoder,
    read_buf: &mut [u8],
    report: &mut core::DeviceReport,
    timeout: Duration,
    mut done: Done,
) -> io::Result<bool>
where
    S: AsyncRead + Unpin,
    Done: FnMut(&core::DeviceReport) -> bool,
{
    if done(report) {
        return Ok(true);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let read = match tokio::time::timeout(remaining, stream.read(read_buf)).await {
            Err(_elapsed) => return Ok(false),
            Ok(Ok(0)) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(Ok(read)) => read,
            Ok(Err(error)) => return Err(error),
        };
        decoder.feed_slice(&read_buf[..read], |command, payload| {
            report.apply(command, payload);
        });
        if done(report) {
            return Ok(true);
        }
    }
}

/// Serve one configured connection until the stream drops: deliver `CMD_DATA` bodies to the
/// seam (consuming telemetry and other commands) and frame the seam's outbound as `CMD_DATA`.
/// Distinct from the generic [`framed_stream::serve`](crate::framed_stream) because the read
/// side dispatches by command rather than treating every frame as data.
async fn serve_rnode<S, Seam>(
    stream: &mut S,
    radio: &RadioConfig,
    decoder: &mut core::CommandDecoder,
    read_buf: &mut [u8],
    frame_buf: &mut [u8],
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let WireMeters {
        status,
        airtime,
        throughput,
        bitrate,
        started,
    } = meters;
    let started = *started;
    let mut packet_phy = core::PacketPhyState::default();
    decoder.reset();
    loop {
        tokio::select! {
            read = stream.read(&mut *read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    if let Some((command, payload)) =
                        decoder.feed_slice_next(chunk, &mut offset).ok().flatten()
                    {
                        if command == core::CMD_DATA {
                            let stats = packet_phy.take_for_data();
                            if !payload.is_empty() {
                                seam.next_inbound_with_phy(payload, stats).await;
                            }
                        } else {
                            packet_phy.apply(command, payload, radio);
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Ok(framed) =
                    kiss_framing::encode_with_command(core::CMD_DATA, outbound, &mut *frame_buf)
                {
                    if stream.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(framed, *bitrate);
                    status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
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
        // The decoder and buffers are heap-held and reused across reconnects — no megabyte of buffer
        // rides the stack, and a device that never answers allocates these exactly once.
        let mut decoder = std::boxed::Box::new(core::CommandDecoder::new());
        let mut read_buf = std::vec![0u8; core::READ_BUF_LEN].into_boxed_slice();
        let mut frame_buf = std::vec![0u8; core::FRAMED_LEN].into_boxed_slice();
        loop {
            if let Ok(mut stream) = (self.open)().await {
                if !self.settle.is_zero() {
                    tokio::time::sleep(self.settle).await;
                }
                // A device that fails to detect or validate is treated like a dropped connection:
                // skip serving and fall through to the reconnect wait, as RNS closes the port.
                if bring_up(&mut stream, &self.radio, &mut decoder, &mut read_buf)
                    .await
                    .is_ok()
                {
                    self.status.set_connection(ConnectionState::Connected);
                    serve_rnode(
                        &mut stream,
                        &self.radio,
                        &mut decoder,
                        &mut read_buf,
                        &mut frame_buf,
                        &mut seam,
                        &mut WireMeters {
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
            tokio::time::sleep(self.reconnect).await;
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
        RadioConfig::new(868_000_000, 125_000, 7, 8, 5, None, None).expect("a valid radio config")
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
                radio.frequency_hz.to_be_bytes().to_vec()
            )
        );
        assert_eq!(
            config[5],
            (core::CMD_RADIO_STATE, std::vec![core::RADIO_STATE_ON])
        );
        write_command(wire, core::CMD_FREQUENCY, &radio.frequency_hz.to_be_bytes()).await;
        write_command(wire, core::CMD_BANDWIDTH, &radio.bandwidth_hz.to_be_bytes()).await;
        write_command(wire, core::CMD_TXPOWER, &[radio.txpower_dbm]).await;
        write_command(wire, core::CMD_SF, &[radio.spreading_factor]).await;
        write_command(wire, core::CMD_CR, &[radio.coding_rate]).await;
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
            Duration::from_millis(10),
            Duration::ZERO,
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
            Duration::from_millis(10),
            Duration::ZERO,
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
            &radio.frequency_hz.to_be_bytes(),
        )
        .await;
        write_command(
            &mut device,
            core::CMD_BANDWIDTH,
            &radio.bandwidth_hz.to_be_bytes(),
        )
        .await;
        write_command(&mut device, core::CMD_TXPOWER, &[radio.txpower_dbm]).await;
        write_command(&mut device, core::CMD_SF, &[radio.spreading_factor + 1]).await;
        write_command(&mut device, core::CMD_CR, &[radio.coding_rate]).await;
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
}
