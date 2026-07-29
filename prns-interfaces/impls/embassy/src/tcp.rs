use ::core::time::Duration as CoreDuration;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_io_async_07::Write;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::interfaces::{
    tcp, BitrateBps, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::EmbassyInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam, EMBEDDED_MAX_LINK_MTU};
use prns_runtime::manifold::reconnect::ReconnectPolicy;
use prns_runtime::manifold::throughput::ThroughputLedger;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A connection idle past [`SOCKET_TIMEOUT`] is dropped for reconnect, while [`KEEP_ALIVE`] prevents a quiet live link from reaching that timeout.
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(24);
pub const KEEP_ALIVE: Duration = Duration::from_secs(5);
pub const TCP_DNS_HOSTNAME_MAX_BYTES: usize = 253;

pub struct TcpSocketBuffers<'a> {
    pub rx: &'a mut [u8],
    pub tx: &'a mut [u8],
}

pub struct TcpClientInput<'a> {
    pub stack: Stack<'a>,
    pub target: TcpClientTarget,
    pub channel_tag: &'a [u8],
    pub bitrate: BitrateBps,
    pub reconnect_policy: ReconnectPolicy,
    pub socket_buffers: TcpSocketBuffers<'a>,
    pub status: &'a EmbassyInterfaceStatus,
}

pub struct TcpClientTarget {
    endpoint: Option<IpEndpoint>,
    #[cfg(feature = "tcp-dns")]
    hostname: heapless::String<TCP_DNS_HOSTNAME_MAX_BYTES>,
    #[cfg(feature = "tcp-dns")]
    port: u16,
}

impl TcpClientTarget {
    #[must_use]
    pub fn endpoint(endpoint: IpEndpoint) -> Self {
        Self {
            endpoint: Some(endpoint),
            #[cfg(feature = "tcp-dns")]
            hostname: heapless::String::new(),
            #[cfg(feature = "tcp-dns")]
            port: endpoint.port,
        }
    }

    #[cfg(feature = "tcp-dns")]
    #[must_use]
    pub fn dns(hostname: heapless::String<TCP_DNS_HOSTNAME_MAX_BYTES>, port: u16) -> Self {
        Self {
            endpoint: None,
            hostname,
            port,
        }
    }
}

pub struct TcpClient<'a> {
    id: InterfaceId,
    stack: Stack<'a>,
    target: TcpClientTarget,
    tag: &'a [u8],
    bitrate: BitrateBps,
    reconnect_policy: ReconnectPolicy,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    status: &'a EmbassyInterfaceStatus,
}

impl<'a> TcpClient<'a> {
    #[must_use]
    pub fn interface_id(tag: &[u8]) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::TcpClient, tag)
    }

    #[must_use]
    pub fn new(input: TcpClientInput<'a>) -> Self {
        let TcpClientInput {
            stack,
            target,
            channel_tag,
            bitrate,
            reconnect_policy,
            socket_buffers,
            status,
        } = input;
        Self {
            id: Self::interface_id(channel_tag),
            stack,
            target,
            tag: channel_tag,
            bitrate,
            reconnect_policy,
            rx_buffer: socket_buffers.rx,
            tx_buffer: socket_buffers.tx,
            status,
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }
}

impl Interface for TcpClient<'_> {
    const HW_MTU: usize = EMBEDDED_MAX_LINK_MTU;
    const KIND: InterfaceKind = InterfaceKind::TcpClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        tcp::descriptor(self.id, tcp::policy_for_bitrate(self.bitrate))
    }

    fn channel_tag(&self) -> &[u8] {
        self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let TcpClient {
            id: _,
            stack,
            target,
            tag: _,
            bitrate,
            reconnect_policy,
            rx_buffer,
            tx_buffer,
            status,
        } = self;
        let mut decoder = RnsSerialDecoder::<{ tcp::EMBEDDED_FRAME_CAP }>::new();
        let mut read_buf = [0u8; tcp::EMBEDDED_READ_BUF_LEN];
        let mut frame_buf = [0u8; tcp::EMBEDDED_FRAMED_LEN];
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = Instant::now();
        let mut reconnect = reconnect_policy.schedule();

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                status.wait_until_enabled().await;
                continue;
            }
            let resolved_target = select(
                with_timeout(CONNECT_TIMEOUT, resolve_target(stack, &target)),
                status.wait_until_disabled(),
            )
            .await;
            let Either::First(Ok(Some(resolved_target))) = resolved_target else {
                if status.is_enabled() {
                    status.set_connection(ConnectionState::Disconnected);
                    let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                    let _ = select(
                        Timer::after(Duration::from_millis(reconnect_delay.as_millis() as u64)),
                        status.wait_until_disabled(),
                    )
                    .await;
                }
                continue;
            };
            let mut socket = TcpSocket::new(stack, &mut *rx_buffer, &mut *tx_buffer);
            socket.set_timeout(Some(SOCKET_TIMEOUT));
            socket.set_keep_alive(Some(KEEP_ALIVE));
            let connected = select(
                with_timeout(CONNECT_TIMEOUT, socket.connect(resolved_target)),
                status.wait_until_disabled(),
            )
            .await;
            if let Either::First(Ok(Ok(()))) = connected {
                let connected_at = Instant::now();
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
                    bitrate,
                    started,
                )
                .await;
                reconnect.record_connection_lifetime(CoreDuration::from_millis(
                    connected_at.elapsed().as_millis(),
                ));
            }
            socket.abort();
            // Skip reconnect delay after disable so status changes immediately.
            if status.is_enabled() {
                status.set_connection(ConnectionState::Disconnected);
                let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                let _ = select(
                    Timer::after(Duration::from_millis(reconnect_delay.as_millis() as u64)),
                    status.wait_until_disabled(),
                )
                .await;
            }
        }
    }
}

async fn resolve_target(_stack: Stack<'_>, target: &TcpClientTarget) -> Option<IpEndpoint> {
    if let Some(endpoint) = target.endpoint {
        return Some(endpoint);
    }
    #[cfg(feature = "tcp-dns")]
    {
        use embassy_net::dns::DnsQueryType;
        use embassy_net::IpAddress;

        return _stack
            .dns_query(target.hostname.as_str(), DnsQueryType::A)
            .await
            .ok()?
            .into_iter()
            .find_map(|address| match address {
                IpAddress::Ipv4(address) => {
                    Some(IpEndpoint::new(IpAddress::Ipv4(address), target.port))
                }
                IpAddress::Ipv6(_) => None,
            });
    }
    #[cfg(not(feature = "tcp-dns"))]
    None
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn serve<Seam: InterfaceSeam>(
    socket: &mut TcpSocket<'_>,
    seam: &mut Seam,
    status: &EmbassyInterfaceStatus,
    decoder: &mut RnsSerialDecoder<{ tcp::EMBEDDED_FRAME_CAP }>,
    read_buf: &mut [u8],
    frame_buf: &mut [u8],
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    bitrate: BitrateBps,
    started: Instant,
) {
    let (mut reader, mut writer) = socket.split();
    loop {
        match select3(
            reader.read(read_buf),
            seam.next_outbound(),
            status.wait_until_disabled(),
        )
        .await
        {
            Either3::Third(()) => return,
            Either3::First(read) => {
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
            Either3::Second(outbound) => {
                if let Ok(framed) = rns_serial_framing::encode(outbound, frame_buf) {
                    if writer.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis());
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(framed, bitrate);
                    status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}
