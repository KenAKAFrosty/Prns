use ::core::time::Duration as CoreDuration;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_net::tcp::{Error as TcpIoError, TcpSocket};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpClientExitCause {
    PeerClosed,
    ReadFailure(TcpIoError),
    WriteFailure(TcpIoError),
    Timeout,
    NetworkUnavailable,
    Disabled,
}

enum TcpConnectionAttempt {
    Initial,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TcpNetworkFamily {
    Ipv4,
    Ipv6,
}

impl TcpConnectionAttempt {
    const fn connection_state(&self) -> ConnectionState {
        match self {
            TcpConnectionAttempt::Initial => ConnectionState::Initializing,
            TcpConnectionAttempt::Retry => ConnectionState::Reconnecting,
        }
    }
}

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

#[derive(Debug)]
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

    fn network_family(&self) -> TcpNetworkFamily {
        match self.endpoint.map(|endpoint| endpoint.addr) {
            Some(embassy_net::IpAddress::Ipv4(_)) | None => TcpNetworkFamily::Ipv4,
            Some(embassy_net::IpAddress::Ipv6(_)) => TcpNetworkFamily::Ipv6,
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
        let mut connection_attempt = TcpConnectionAttempt::Initial;
        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                status.wait_until_enabled().await;
                continue;
            }
            status.set_connection(connection_attempt.connection_state());
            connection_attempt = TcpConnectionAttempt::Retry;
            let network_family = target.network_family();
            let network_ready = select(
                wait_until_network_ready(stack, network_family),
                status.wait_until_disabled(),
            )
            .await;
            if matches!(network_ready, Either::Second(())) {
                status.set_connection(ConnectionState::Disabled);
                continue;
            }
            crate::diagnostic_log::info!("tcp-client [configured]: resolving target={target:?}");
            let resolved_target = select(
                with_timeout(CONNECT_TIMEOUT, resolve_target(stack, &target)),
                status.wait_until_disabled(),
            )
            .await;
            let resolved_target = match resolved_target {
                Either::First(Ok(Some(resolved_target))) => {
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: resolved target={target:?} endpoint={resolved_target:?}"
                    );
                    resolved_target
                }
                Either::First(Ok(None)) => {
                    crate::diagnostic_log::warn!(
                        "tcp-client [configured]: resolution failed target={target:?}"
                    );
                    status.set_connection(ConnectionState::Disconnected);
                    let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: target={target:?} retry_delay_ms={}",
                        reconnect_delay.as_millis()
                    );
                    let _ = select(
                        Timer::after(Duration::from_millis(reconnect_delay.as_millis() as u64)),
                        status.wait_until_disabled(),
                    )
                    .await;
                    continue;
                }
                Either::First(Err(_)) => {
                    crate::diagnostic_log::warn!(
                        "tcp-client [configured]: resolution failed target={target:?} cause={:?}",
                        TcpClientExitCause::Timeout
                    );
                    status.set_connection(ConnectionState::Disconnected);
                    let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: target={target:?} retry_delay_ms={}",
                        reconnect_delay.as_millis()
                    );
                    let _ = select(
                        Timer::after(Duration::from_millis(reconnect_delay.as_millis() as u64)),
                        status.wait_until_disabled(),
                    )
                    .await;
                    continue;
                }
                Either::Second(()) => {
                    status.set_connection(ConnectionState::Disabled);
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: target={target:?} exit={:?}",
                        TcpClientExitCause::Disabled
                    );
                    continue;
                }
            };
            let mut socket = TcpSocket::new(stack, &mut *rx_buffer, &mut *tx_buffer);
            socket.set_timeout(Some(SOCKET_TIMEOUT));
            socket.set_keep_alive(Some(KEEP_ALIVE));
            crate::diagnostic_log::info!(
                "tcp-client [configured]: connecting target={target:?} endpoint={resolved_target:?}"
            );
            let connected = select(
                with_timeout(CONNECT_TIMEOUT, socket.connect(resolved_target)),
                status.wait_until_disabled(),
            )
            .await;
            match connected {
                Either::First(Ok(Ok(()))) => {
                    let connected_at = Instant::now();
                    reset_decoder_for_connection(&mut decoder);
                    status.set_connection(ConnectionState::Connected);
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: connected target={target:?} endpoint={resolved_target:?}"
                    );
                    let exit = serve(
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
                        stack,
                        network_family,
                    )
                    .await;
                    let lifetime_ms = connected_at.elapsed().as_millis();
                    reconnect.record_connection_lifetime(CoreDuration::from_millis(lifetime_ms));
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: target={target:?} endpoint={resolved_target:?} exit={exit:?} lifetime_ms={lifetime_ms}"
                    );
                }
                Either::First(Ok(Err(error))) => {
                    crate::diagnostic_log::warn!(
                        "tcp-client [configured]: connect failed target={target:?} endpoint={resolved_target:?} error={error:?}"
                    );
                }
                Either::First(Err(_)) => {
                    crate::diagnostic_log::warn!(
                        "tcp-client [configured]: connect failed target={target:?} endpoint={resolved_target:?} cause={:?}",
                        TcpClientExitCause::Timeout
                    );
                }
                Either::Second(()) => {
                    crate::diagnostic_log::info!(
                        "tcp-client [configured]: connect stopped target={target:?} endpoint={resolved_target:?} exit={:?}",
                        TcpClientExitCause::Disabled
                    );
                }
            }
            socket.abort();
            // Skip reconnect delay after disable so status changes immediately.
            if status.is_enabled() {
                status.set_connection(ConnectionState::Disconnected);
                let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
                crate::diagnostic_log::info!(
                    "tcp-client [configured]: target={target:?} retry_delay_ms={}",
                    reconnect_delay.as_millis()
                );
                let _ = select(
                    Timer::after(Duration::from_millis(reconnect_delay.as_millis() as u64)),
                    status.wait_until_disabled(),
                )
                .await;
            } else {
                status.set_connection(ConnectionState::Disabled);
            }
        }
    }
}

fn reset_decoder_for_connection(decoder: &mut RnsSerialDecoder<{ tcp::EMBEDDED_FRAME_CAP }>) {
    decoder.reset();
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

fn network_ready(stack: Stack<'_>, family: TcpNetworkFamily) -> bool {
    if !stack.is_link_up() {
        return false;
    }
    match family {
        TcpNetworkFamily::Ipv4 => stack.config_v4().is_some(),
        TcpNetworkFamily::Ipv6 => stack.config_v6().is_some(),
    }
}

async fn wait_until_network_ready(stack: Stack<'_>, family: TcpNetworkFamily) {
    while !network_ready(stack, family) {
        Timer::after(Duration::from_millis(100)).await;
    }
}

async fn wait_until_network_unavailable(stack: Stack<'_>, family: TcpNetworkFamily) {
    while network_ready(stack, family) {
        Timer::after(Duration::from_millis(100)).await;
    }
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
    stack: Stack<'_>,
    network_family: TcpNetworkFamily,
) -> TcpClientExitCause {
    let (mut reader, mut writer) = socket.split();
    loop {
        match select4(
            reader.read(read_buf),
            seam.next_outbound(),
            status.wait_until_disabled(),
            wait_until_network_unavailable(stack, network_family),
        )
        .await
        {
            Either4::Fourth(()) => return TcpClientExitCause::NetworkUnavailable,
            Either4::Third(()) => return TcpClientExitCause::Disabled,
            Either4::First(read) => {
                let read = match read {
                    Ok(0) => return TcpClientExitCause::PeerClosed,
                    Err(error) => return TcpClientExitCause::ReadFailure(error),
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis());
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    match decoder.feed_slice_next(chunk, &mut offset) {
                        Ok(Some(frame)) => {
                            if !frame.is_empty() {
                                status.count_frame_in();
                                seam.next_inbound(frame).await;
                                status.count_frame_delivered();
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            status.count_frame_undecodable();
                            crate::diagnostic_log::warn!(
                                "tcp-client [configured]: decode failed error={error:?}"
                            );
                        }
                    }
                }
            }
            Either4::Second(outbound) => match rns_serial_framing::encode(outbound, frame_buf) {
                Ok(framed) => {
                    if let Err(error) = writer.write_all(&frame_buf[..framed]).await {
                        return TcpClientExitCause::WriteFailure(error);
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis());
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(framed, bitrate);
                    status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
                Err(error) => crate::diagnostic_log::warn!(
                    "tcp-client [configured]: encode failed error={error:?}"
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_connection_discards_partial_decoder_state() {
        let mut decoder = RnsSerialDecoder::<{ tcp::EMBEDDED_FRAME_CAP }>::new();
        let mut encoded = [0u8; tcp::EMBEDDED_FRAMED_LEN];
        let len = rns_serial_framing::encode(b"second connection", &mut encoded).unwrap();
        let mut offset = 0;
        let _ = decoder.feed_slice_next(&encoded[..len / 2], &mut offset);

        reset_decoder_for_connection(&mut decoder);
        offset = 0;
        let decoded = decoder
            .feed_slice_next(&encoded[..len], &mut offset)
            .unwrap()
            .expect("the complete frame decodes after reset");

        assert_eq!(decoded, b"second connection");
    }

    #[test]
    fn connection_attempts_distinguish_initialization_from_retries() {
        assert_eq!(
            TcpConnectionAttempt::Initial.connection_state(),
            ConnectionState::Initializing
        );
        assert_eq!(
            TcpConnectionAttempt::Retry.connection_state(),
            ConnectionState::Reconnecting
        );
    }

    #[test]
    fn tcp_targets_select_their_required_network_family() {
        let v4 = TcpClientTarget::endpoint(IpEndpoint::new(
            embassy_net::Ipv4Address::new(192, 0, 2, 1).into(),
            4242,
        ));
        let v6 = TcpClientTarget::endpoint(IpEndpoint::new(
            embassy_net::Ipv6Address::LOCALHOST.into(),
            4242,
        ));
        assert_eq!(v4.network_family(), TcpNetworkFamily::Ipv4);
        assert_eq!(v6.network_family(), TcpNetworkFamily::Ipv6);
    }
}
