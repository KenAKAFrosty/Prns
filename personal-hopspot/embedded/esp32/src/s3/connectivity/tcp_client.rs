use super::super::*;

pub(in crate::s3) fn build_tcp(
    stack: Stack<'static>,
    config: &HopspotTcpClientConfig,
) -> Option<(
    TcpClient<'static>,
    &'static EmbassyInterfaceStatus,
    InterfaceId,
)> {
    let channel_tag = crate::storage::allocate_psram_slice(256, 0u8);
    let (target, target_len) = match &config.host {
        HopspotTcpClientHost::Ipv4(address) => {
            channel_tag[0] = 1;
            channel_tag[1..5].copy_from_slice(&address.octets());
            (
                TcpClientTarget::endpoint(IpEndpoint::new((*address).into(), config.port)),
                5,
            )
        }
        HopspotTcpClientHost::Hostname(hostname) => {
            let dns_hostname =
                heapless::String::<TCP_DNS_HOSTNAME_MAX_BYTES>::try_from(hostname.as_str()).ok()?;
            channel_tag[0] = 2;
            channel_tag[1..1 + hostname.len()].copy_from_slice(hostname.as_bytes());
            (
                TcpClientTarget::dns(dns_hostname, config.port),
                1 + hostname.len(),
            )
        }
    };
    channel_tag[target_len..target_len + 2].copy_from_slice(&config.port.to_be_bytes());
    let channel_tag: &'static [u8] = &channel_tag[..target_len + 2];
    let id = TcpClient::interface_id(channel_tag);
    let status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing)
    );
    let rx_buffer: &'static mut [u8] =
        crate::storage::allocate_psram_slice(TCP_SOCKET_BUFFER_BYTES, 0u8);
    let tx_buffer: &'static mut [u8] =
        crate::storage::allocate_psram_slice(TCP_SOCKET_BUFFER_BYTES, 0u8);
    let tcp = TcpClient::new(TcpClientInput {
        stack,
        target,
        channel_tag,
        bitrate: TCP_BITRATE_BPS,
        reconnect_policy: ReconnectPolicy::STANDARD,
        socket_buffers: TcpSocketBuffers {
            rx: rx_buffer,
            tx: tx_buffer,
        },
        status,
    });
    Some((tcp, status, id))
}
