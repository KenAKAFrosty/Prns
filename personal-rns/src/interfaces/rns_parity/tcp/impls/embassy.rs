//! The embassy TCP-client interface: an RNS `TCPClientInterface` over embassy-net's smoltcp
//! socket. The board dials a fixed Reticulum TCP node on the LAN (no DNS — the target arrives as
//! an already-resolved [`IpEndpoint`]), speaks the same RNS HDLC framing the serial and tokio-TCP
//! interfaces speak, and reconnects when the stream drops. Point-to-point: one engine interface,
//! one peer.
//!
//! Its decoder and frame buffers size to the board's embedded wire ceiling
//! ([`core::EMBEDDED_FRAME_CAP`]), never the host's absolute one — the host-vs-embedded split the
//! reactor lanes draw. The reactor clamps the declared MTU to those lanes regardless, so a link can
//! never negotiate past the buffers a frame must land in.

use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_io_async_07::Write;

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::tcp::core;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::embassy_reactor::EmbassyInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam, EMBEDDED_MAX_LINK_MTU};
use crate::reactor::throughput::ThroughputLedger;

/// How long one connect attempt gets (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The default wait between a dropped connection and the next attempt
/// (`TCPClientInterface.RECONNECT_WAIT`); [`TcpClient::new`] takes the live value.
pub const RECONNECT_WAIT: Duration = Duration::from_secs(5);
/// The socket discipline, embassy-net's nearest equivalent to the reference's keepalive: a
/// connection idle past [`SOCKET_TIMEOUT`] is dropped so the reconnect loop heals it, and
/// [`KEEP_ALIVE`] probes keep a quiet-but-live link from tripping that timeout.
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(24);
pub const KEEP_ALIVE: Duration = Duration::from_secs(5);

/// The initiating end of an RNS TCP pair on embassy. Owns its connection lifecycle: connect to
/// `target`, serve until the stream drops, wait `reconnect`, connect again. The socket's smoltcp
/// buffers (`rx_buffer`/`tx_buffer`) and the status handle are borrowed from the board's `static`s;
/// `tag` is the stable channel identity — the configured target's bytes — the interface id derives
/// from, so the same node reconnects under the same routing key. `bitrate_bps` is the host's claim
/// about its pipe; it sets the declared MTU through the reference's tier table, so claim honestly
/// ([`core::TCP_BITRATE_GUESS_BPS`] when genuinely unknown).
pub struct TcpClient<'a> {
    id: InterfaceId,
    stack: Stack<'a>,
    target: IpEndpoint,
    tag: &'a [u8],
    bitrate_bps: u32,
    reconnect: Duration,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    status: &'a EmbassyInterfaceStatus,
}

impl<'a> TcpClient<'a> {
    /// The id a client dialing `tag` will carry — for the caller that must stand its
    /// [`EmbassyInterfaceStatus`] up under the same key before it builds the interface.
    #[must_use]
    pub fn interface_id(tag: &[u8]) -> InterfaceId {
        InterfaceId::from_reachability_tag(InterfaceKind::TcpClient, tag)
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stack: Stack<'a>,
        target: IpEndpoint,
        tag: &'a [u8],
        bitrate_bps: u32,
        reconnect: Duration,
        rx_buffer: &'a mut [u8],
        tx_buffer: &'a mut [u8],
        status: &'a EmbassyInterfaceStatus,
    ) -> Self {
        Self {
            id: Self::interface_id(tag),
            stack,
            target,
            tag,
            bitrate_bps,
            reconnect,
            rx_buffer,
            tx_buffer,
            status,
        }
    }

    /// This interface's id, derived from the dial target — for the app that names it (an
    /// [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }
}

impl Interface for TcpClient<'_> {
    const HW_MTU: usize = EMBEDDED_MAX_LINK_MTU;
    const KIND: InterfaceKind = InterfaceKind::TcpClient;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn reachability_tag(&self) -> &[u8] {
        self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let TcpClient {
            id: _,
            stack,
            target,
            tag: _,
            bitrate_bps,
            reconnect,
            rx_buffer,
            tx_buffer,
            status,
        } = self;
        let mut decoder = RnsSerialDecoder::<{ core::EMBEDDED_FRAME_CAP }>::new();
        let mut read_buf = [0u8; core::EMBEDDED_READ_BUF_LEN];
        let mut frame_buf = [0u8; core::EMBEDDED_FRAMED_LEN];
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = Instant::now();

        loop {
            let mut socket = TcpSocket::new(stack, &mut *rx_buffer, &mut *tx_buffer);
            socket.set_timeout(Some(SOCKET_TIMEOUT));
            socket.set_keep_alive(Some(KEEP_ALIVE));
            if let Ok(Ok(())) = with_timeout(CONNECT_TIMEOUT, socket.connect(target)).await {
                status.set_connection(ConnectionState::Connected);
                serve(
                    &mut socket,
                    &mut seam,
                    status,
                    &mut decoder,
                    &mut read_buf,
                    &mut frame_buf,
                    &mut airtime,
                    &mut throughput,
                    bitrate_bps,
                    started,
                )
                .await;
                status.set_connection(ConnectionState::Disconnected);
            }
            socket.abort();
            Timer::after(reconnect).await;
        }
    }
}

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam, drain
/// the seam and frame outbound onto the wire. Returns on any IO error so [`run`](TcpClient::run)
/// reconnects. The socket is split so a read in flight and a write never contend for it.
#[allow(clippy::too_many_arguments)]
async fn serve<Seam: InterfaceSeam>(
    socket: &mut TcpSocket<'_>,
    seam: &mut Seam,
    status: &EmbassyInterfaceStatus,
    decoder: &mut RnsSerialDecoder<{ core::EMBEDDED_FRAME_CAP }>,
    read_buf: &mut [u8],
    frame_buf: &mut [u8],
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    bitrate_bps: u32,
    started: Instant,
) {
    let (mut reader, mut writer) = socket.split();
    loop {
        match select(reader.read(read_buf), seam.next_outbound()).await {
            Either::First(read) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis());
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    if let Ok(Some(frame)) = decoder.feed_slice_next(chunk, &mut offset) {
                        if !frame.is_empty() {
                            seam.next_inbound(frame).await;
                        }
                    }
                }
            }
            Either::Second(outbound) => {
                if let Ok(framed) = rns_serial_framing::encode(outbound, frame_buf) {
                    if writer.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis());
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(framed, bitrate_bps);
                    status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}
