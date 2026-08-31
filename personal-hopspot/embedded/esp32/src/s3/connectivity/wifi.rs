use super::super::captive_portal::{
    build_ap_netif, dhcp_server_task, dns_server_task, http_server_task, station_wifi_mode,
    HTTP_SERVER_WORKERS,
};
use super::super::*;
use super::station::{net_task, network_ready_task, wifi_connect_task, StationCredentials};
use alloc::boxed::Box;
use personal_rns::wifi_auto::MdnsMulticastFamily;

fn psram_udp_socket<
    const RX_META: usize,
    const RX_BYTES: usize,
    const TX_META: usize,
    const TX_BYTES: usize,
>(
    stack: Stack<'static>,
) -> UdpSocket<'static> {
    UdpSocket::new(
        stack,
        crate::storage::allocate_psram_slice(RX_META, PacketMetadata::EMPTY),
        crate::storage::allocate_psram_slice(RX_BYTES, 0u8),
        crate::storage::allocate_psram_slice(TX_META, PacketMetadata::EMPTY),
        crate::storage::allocate_psram_slice(TX_BYTES, 0u8),
    )
}

// Preserve the ESP32-S3 driver's established static RX headroom. Although six buffers matches the
// configured Block Ack window, on-device Wi-Fi/BLE coexistence testing showed that reducing the
// prior ten-buffer pool caused the station RX path to wedge repeatedly.
const WIFI_STATIC_RX_BUFFERS: u8 = 10;
// Match the driver-private pool to the unchanged 32-frame software queue. Each S3 dynamic RX
// buffer costs roughly 1.6 KiB of internal RAM; buffers beyond the queue's capacity cannot add
// delivery capacity and would consume the headroom BLE coexistence needs to keep recycling RX.
const WIFI_DYNAMIC_RX_BUFFERS: u16 = 32;
const WIFI_RX_BA_WINDOW: u8 = 6;
const WIFI_RX_QUEUE_FRAMES: usize = 32;
const WIFI_TX_QUEUE_FRAMES: usize = 3;
// Keep TX storage demand-driven. Sixteen static buffers permanently reserve roughly 25.6 KiB of
// internal RAM and leave an active BLE link with almost no allocation headroom. The driver copies
// each submission into a dynamic buffer, while the three-frame software queue below bounds the
// number that can be live concurrently.
const WIFI_STATIC_TX_BUFFERS: u8 = 0;
const WIFI_DYNAMIC_TX_BUFFERS: u16 = 16;
const WIFI_DATA_SOCKET_BUFFER_BYTES: usize = 4 * 1_024;
const WIFI_DATA_SOCKET_METADATA: usize = 8;
const WIFI_AUTO_DISCOVERY_SOCKET_METADATA: usize = 8;
const WIFI_AUTO_DISCOVERY_SOCKET_BYTES: usize = 128;
const WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_METADATA: usize = 8;
const WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_BYTES: usize = 512;
const WIFI_AUTO_UNICAST_DISCOVERY_TX_QUEUED_PACKETS: usize = 2;
const WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_METADATA: usize =
    WIFI_AUTO_UNICAST_DISCOVERY_TX_QUEUED_PACKETS + 1;
const WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_BYTES: usize =
    wifi_auto_contract::PEERING_TOKEN_BYTES * WIFI_AUTO_UNICAST_DISCOVERY_TX_QUEUED_PACKETS;
// `embassy_net::new` installs one DNS resolver socket whenever the firmware enables DNS. That
// socket exists even for an IPv4 TCP target, so it must be counted separately from the configured
// client's socket or DNS-SD leaves the fixed socket table full before `TcpSocket::new` runs.
const EMBASSY_DNS_RESOLVER_SOCKET_COUNT: usize = 1;
const DHCP_CLIENT_SOCKET_COUNT: usize = 1;
const WIFI_AUTO_DATAGRAM_SOCKET_COUNT: usize = 3;
const CONFIGURED_TCP_CLIENT_SOCKET_COUNT: usize = 1;
const STATION_STACK_SOCKET_CAPACITY: usize = EMBASSY_DNS_RESOLVER_SOCKET_COUNT
    + DHCP_CLIENT_SOCKET_COUNT
    + WIFI_AUTO_DATAGRAM_SOCKET_COUNT
    + CONFIGURED_TCP_CLIENT_SOCKET_COUNT
    + UDP_SERVICE_DISCOVERY_SOCKET_COUNT as usize;
fn wifi_auto_station_multicast_discovery_socket(stack: Stack<'static>) -> UdpSocket<'static> {
    psram_udp_socket::<
        WIFI_AUTO_DISCOVERY_SOCKET_METADATA,
        WIFI_AUTO_DISCOVERY_SOCKET_BYTES,
        WIFI_AUTO_DISCOVERY_SOCKET_METADATA,
        WIFI_AUTO_DISCOVERY_SOCKET_BYTES,
    >(stack)
}

fn wifi_auto_soft_ap_multicast_discovery_socket(stack: Stack<'static>) -> UdpSocket<'static> {
    psram_udp_socket::<
        WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_METADATA,
        WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_BYTES,
        WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_METADATA,
        WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_BYTES,
    >(stack)
}

fn wifi_auto_unicast_discovery_socket(stack: Stack<'static>) -> UdpSocket<'static> {
    psram_udp_socket::<
        WIFI_AUTO_DISCOVERY_SOCKET_METADATA,
        WIFI_AUTO_DISCOVERY_SOCKET_BYTES,
        WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_METADATA,
        WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_BYTES,
    >(stack)
}

fn wifi_auto_data_socket(stack: Stack<'static>) -> UdpSocket<'static> {
    psram_udp_socket::<
        WIFI_DATA_SOCKET_METADATA,
        WIFI_DATA_SOCKET_BUFFER_BYTES,
        WIFI_DATA_SOCKET_METADATA,
        WIFI_DATA_SOCKET_BUFFER_BYTES,
    >(stack)
}

fn udp_service_discovery_socket(stack: Stack<'static>) -> UdpSocket<'static> {
    psram_udp_socket::<
        UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA,
        UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES,
        UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA,
        UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES,
    >(stack)
}

const _: () = assert!(WIFI_STATIC_RX_BUFFERS >= WIFI_RX_BA_WINDOW);
const _: () = assert!(WIFI_DYNAMIC_RX_BUFFERS > WIFI_RX_BA_WINDOW as u16);
const _: () = assert!(WIFI_DYNAMIC_RX_BUFFERS as usize >= WIFI_RX_QUEUE_FRAMES);
const _: () = assert!(WIFI_DYNAMIC_TX_BUFFERS >= WIFI_TX_QUEUE_FRAMES as u16);
const _: () = assert!(STATION_STACK_SOCKET_CAPACITY == 7);
const _: () = assert!(
    WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_METADATA > WIFI_AUTO_UNICAST_DISCOVERY_TX_QUEUED_PACKETS
);
const _: () = assert!(
    WIFI_AUTO_UNICAST_DISCOVERY_TX_SOCKET_BYTES
        == wifi_auto_contract::PEERING_TOKEN_BYTES * WIFI_AUTO_UNICAST_DISCOVERY_TX_QUEUED_PACKETS
);
const _: () = assert!(WIFI_AUTO_DISCOVERY_SOCKET_BYTES >= wifi_auto_contract::PEERING_TOKEN_BYTES);
const _: () =
    assert!(WIFI_AUTO_SOFT_AP_DISCOVERY_SOCKET_BYTES >= wifi_auto_contract::PEERING_TOKEN_BYTES);

pub(in crate::s3) fn build_wifi(
    spawner: &Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    mac: [u8; 6],
    config: &HopspotWifiConfig,
    ap_enabled: bool,
) -> (
    Option<AutoWifi<'static, MEMBERS>>,
    Option<Stack<'static>>,
    Option<EspNow<'static>>,
) {
    let wifi_config = ControllerConfig::default()
        .with_static_rx_buf_num(WIFI_STATIC_RX_BUFFERS)
        .with_dynamic_rx_buf_num(WIFI_DYNAMIC_RX_BUFFERS)
        .with_rx_ba_win(WIFI_RX_BA_WINDOW)
        .with_rx_queue_size(WIFI_RX_QUEUE_FRAMES)
        .with_tx_queue_size(WIFI_TX_QUEUE_FRAMES)
        .with_static_tx_buf_num(WIFI_STATIC_TX_BUFFERS)
        .with_dynamic_tx_buf_num(WIFI_DYNAMIC_TX_BUFFERS);
    let Ok((mut controller, interfaces)) = esp_radio::wifi::new(wifi, wifi_config) else {
        return (None, None, None);
    };
    log::info!(
        "wifi: rx profile static={} dynamic={} ba={} queue={} tx_queue={} tx_static={} tx_dynamic={}",
        WIFI_STATIC_RX_BUFFERS,
        WIFI_DYNAMIC_RX_BUFFERS,
        WIFI_RX_BA_WINDOW,
        WIFI_RX_QUEUE_FRAMES,
        WIFI_TX_QUEUE_FRAMES,
        WIFI_STATIC_TX_BUFFERS,
        WIFI_DYNAMIC_TX_BUFFERS
    );
    let esp_now = interfaces.esp_now;

    // In SoftAP mode, APSTA brings the AP up whether or not a station uplink is configured;
    // set_config calls esp_wifi_start, so the AP is live here on core 0.
    let _ = controller.set_config(&station_wifi_mode(
        StationConfig::default(),
        ap_enabled,
        None,
    ));

    // Opportunistic station uplink: only a configured SSID stands a station netif up and runs
    // the connect loop; otherwise the keepalive task just owns the controller, no scanning.
    let station_segment: Option<AutoWifiSegment<'static>> = if config.has_station() {
        let link_local = wifi_auto_contract::link_local_from_mac(MacAddress::new(mac));
        // Dual-stack: the v6 link-local carries Wi-Fi Auto's discovery/data UDP; v4 over DHCP gives
        // the board a routable address to dial a Reticulum TCP node by ip:port.
        let mut net_config = NetConfig::dhcpv4(DhcpConfig::default());
        net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
            address: Ipv6Cidr::new(link_local, 64),
            gateway: None,
            dns_servers: Default::default(),
        });
        // embassy-net's socket table is normal Rust state and may live in PSRAM. Reserving
        // internal SRAM for the vendor radio allocator prevents its RX pool from stalling.
        let resources =
            crate::storage::allocate_psram(StackResources::<STATION_STACK_SOCKET_CAPACITY>::new());
        let seed = {
            let mut bytes = [0u8; 8];
            Rng::new().read(&mut bytes);
            u64::from_le_bytes(bytes)
        };
        let (stack, runner) = embassy_net::new(interfaces.station, net_config, resources, seed);
        let discovery = wifi_auto_station_multicast_discovery_socket(stack);
        let unicast_discovery = wifi_auto_unicast_discovery_socket(stack);
        let data = wifi_auto_data_socket(stack);
        let wifi_status = AutoWifiStatus::new(&WIFI_SHARED);
        start_udp_service_discovery(spawner, stack, link_local, wifi_status);
        let station_credentials = StationCredentials {
            ssid: config.ssid.clone(),
            password: config.password.clone(),
        };
        spawner.spawn(net_task(runner).expect("net task fits"));
        spawner.spawn(network_ready_task(stack).expect("network readiness task fits"));
        spawner.spawn(
            wifi_connect_task(controller, wifi_status, station_credentials, ap_enabled)
                .expect("wifi connect task fits"),
        );
        Some(AutoWifiSegment {
            stack,
            discovery,
            unicast_discovery,
            data,
            mac,
        })
    } else {
        spawner
            .spawn(wifi_radio_keepalive_task(controller).expect("wifi radio keepalive task fits"));
        None
    };
    let tcp_stack = station_segment.as_ref().map(|segment| segment.stack);

    // In explicit SoftAP mode, the AP is the primary Wi-Fi Auto segment and the station (if any) folds
    // in as the opportunistic secondary. The AP link-local is the station MAC + 1 (build_ap_netif
    // derives it from `mac`), and the supervisor hashes its peering token over that AP link-local, so
    // it takes `ap_mac`.
    if ap_enabled {
        let mut ap_mac = mac;
        ap_mac[5] = ap_mac[5].wrapping_add(1);
        let ap_stack = build_ap_netif(spawner, interfaces.access_point, mac);
        // Hand joiners a 192.168.4.x lease with the SoftAP as their default gateway, so their Wi-Fi Auto
        // client auto-dials the TCP rendezvous on the gateway (multicast can't cross the SoftAP).
        spawner.spawn(dhcp_server_task(ap_stack).expect("dhcp server task fits"));
        spawner.spawn(dns_server_task(ap_stack).expect("dns server task fits"));
        for _ in 0..HTTP_SERVER_WORKERS {
            spawner.spawn(http_server_task(ap_stack).expect("http server task fits"));
        }
        let (server0, client0) = build_tcp_rendezvous_listener(ap_stack);
        let (server1, client1) = build_tcp_rendezvous_listener(ap_stack);
        let (server2, client2) = build_tcp_rendezvous_listener(ap_stack);
        let (server3, client3) = build_tcp_rendezvous_listener(ap_stack);
        for server in [server0, server1, server2, server3] {
            spawner.spawn(tcp_rendezvous_task(server).expect("TCP rendezvous task fits"));
        }
        let rendezvous_clients = TcpRendezvousClients::new([client0, client1, client2, client3]);
        let ap_discovery = wifi_auto_soft_ap_multicast_discovery_socket(ap_stack);
        let ap_unicast_discovery = wifi_auto_unicast_discovery_socket(ap_stack);
        let ap_data = wifi_auto_data_socket(ap_stack);
        let wifi = AutoWifi::new(
            AutoWifiTopology {
                primary: AutoWifiSegment {
                    stack: ap_stack,
                    discovery: ap_discovery,
                    unicast_discovery: ap_unicast_discovery,
                    data: ap_data,
                    mac: ap_mac,
                },
                secondary: station_segment,
                rendezvous: Some(rendezvous_clients),
            },
            &WIFI_SHARED,
        );
        return (Some(wifi), tcp_stack, Some(esp_now));
    }

    match station_segment {
        Some(primary) => {
            let wifi = AutoWifi::new(
                AutoWifiTopology {
                    primary,
                    secondary: None,
                    rendezvous: None,
                },
                &WIFI_SHARED,
            );
            (Some(wifi), tcp_stack, Some(esp_now))
        }
        None => (None, None, Some(esp_now)),
    }
}

fn start_udp_service_discovery(
    spawner: &Spawner,
    stack: Stack<'static>,
    address: core::net::Ipv6Addr,
    status: AutoWifiStatus<MEMBERS>,
) {
    let socket = udp_service_discovery_socket(stack);
    let storage = crate::storage::allocate_psram(UdpServiceDiscoveryStorage::<MEMBERS>::new());
    // IPv4 mDNS (224.0.0.251) — works on APs that block IPv6 LL multicast (Android path).
    // Publication carries LL AAAA plus station IPv4 A when DHCP/static v4 is up.
    let service_discovery = match UdpServiceDiscovery::with_multicast(
        socket,
        stack,
        address,
        status,
        storage,
        hardware_entropy,
        MdnsMulticastFamily::Ipv4,
    ) {
        Ok(service_discovery) => service_discovery,
        Err(error) => {
            log::error!("wifi-auto: UDP DNS-SD construction failed: {error:?}");
            return;
        }
    };
    let task = match udp_service_discovery_task(service_discovery) {
        Ok(task) => task,
        Err(_) => {
            log::error!("wifi-auto: UDP DNS-SD task capacity exhausted");
            return;
        }
    };
    spawner.spawn(task);
    log::info!("wifi-auto: UDP DNS-SD task started (IPv4 mDNS)");
}

fn build_tcp_rendezvous_listener(
    stack: Stack<'static>,
) -> (
    TcpRendezvousServer<'static>,
    personal_rns::wifi_auto::TcpRendezvousClient<'static>,
) {
    let events = Box::leak(Box::new([TcpRendezvousWireSlot::empty()]));
    let commands = Box::leak(Box::new([TcpRendezvousWireSlot::empty()]));
    let storage = Box::leak(Box::new(TcpRendezvousStorage::new(events, commands)));
    let rx = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES]));
    let tx = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES]));
    let read = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_READ_BUFFER_BYTES]));
    let framed = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_FRAMED_LEN]));
    let decoder =
        Box::leak(Box::new(
            personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder::<
                TCP_RENDEZVOUS_FRAME_CAP,
            >::new(),
        ));
    tcp_rendezvous(
        stack,
        TcpRendezvousBuffers {
            rx,
            tx,
            read,
            framed,
            decoder,
        },
        storage,
    )
}

#[embassy_executor::task(pool_size = TCP_RENDEZVOUS_CLIENT_CAPACITY)]
async fn tcp_rendezvous_task(server: TcpRendezvousServer<'static>) -> ! {
    server.run().await
}

#[embassy_executor::task]
async fn udp_service_discovery_task(service_discovery: UdpServiceDiscovery<'static, MEMBERS>) -> ! {
    service_discovery.run().await
}

/// Hold the Wi-Fi controller alive with no AP association — dropping it would stop the radio — so
/// ESP-NOW keeps the Wi-Fi MAC up on a fixed channel when no SSID is configured. The radio was started
/// synchronously by [`build_wifi`] before this task takes the controller.
#[embassy_executor::task]
async fn wifi_radio_keepalive_task(_controller: WifiController<'static>) -> ! {
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}
