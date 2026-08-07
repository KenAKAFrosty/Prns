#[cfg(feature = "wifi-auto")]
use super::captive_portal::station_wifi_mode;
#[cfg(feature = "wifi-auto")]
use super::captive_portal::{
    build_ap_netif, dhcp_server_task, dns_server_task, http_server_task, HTTP_SERVER_WORKERS,
};
use super::*;
#[cfg(feature = "wifi-auto")]
use crate::wifi_rx_recovery::{
    StationReceptionAction, StationReceptionRecovery, StationReceptionWindow,
};
#[cfg(feature = "wifi-auto")]
use alloc::boxed::Box;

#[cfg(feature = "wifi-auto")]
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
        crate::storage::allocate_psram([PacketMetadata::EMPTY; RX_META]),
        crate::storage::allocate_psram([0u8; RX_BYTES]),
        crate::storage::allocate_psram([PacketMetadata::EMPTY; TX_META]),
        crate::storage::allocate_psram([0u8; TX_BYTES]),
    )
}

#[cfg(feature = "wifi-auto")]
const WIFI_STATIC_RX_BUFFERS: u8 = 6;
#[cfg(feature = "wifi-auto")]
const WIFI_DYNAMIC_RX_BUFFERS: u16 = 12;
#[cfg(feature = "wifi-auto")]
const WIFI_RX_BA_WINDOW: u8 = 6;
#[cfg(feature = "wifi-auto")]
const WIFI_RX_QUEUE_FRAMES: usize = 8;
#[cfg(feature = "wifi-auto")]
const WIFI_TX_QUEUE_FRAMES: usize = 3;
#[cfg(feature = "wifi-auto")]
const WIFI_STATIC_TX_BUFFERS: u8 = 6;
#[cfg(feature = "wifi-auto")]
const WIFI_DYNAMIC_TX_BUFFERS: u16 = 0;
#[cfg(feature = "wifi-auto")]
const WIFI_DATA_SOCKET_BUFFER_BYTES: usize = 4 * 1_024;
#[cfg(feature = "wifi-auto")]
const WIFI_HEALTH_SAMPLES_BETWEEN_REPORTS: u8 = 4;
#[cfg(feature = "wifi-auto")]
const _: () = assert!(WIFI_STATIC_RX_BUFFERS >= WIFI_RX_BA_WINDOW);
#[cfg(feature = "wifi-auto")]
const _: () = assert!(WIFI_DYNAMIC_RX_BUFFERS > WIFI_RX_BA_WINDOW as u16);
#[cfg(feature = "wifi-auto")]
const _: () = assert!(WIFI_STATIC_TX_BUFFERS >= WIFI_TX_QUEUE_FRAMES as u8);

pub(super) fn build_tcp(
    stack: Stack<'static>,
    config: &HopspotTcpClientConfig,
) -> Option<(
    TcpClient<'static>,
    &'static EmbassyInterfaceStatus,
    InterfaceId,
)> {
    let channel_tag = crate::storage::allocate_psram([0u8; 256]);
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
        EmbassyInterfaceStatus::new(id, ConnectionState::Initializing)
    );
    let rx_buffer: &'static mut [u8] =
        crate::storage::allocate_psram([0u8; TCP_SOCKET_BUFFER_BYTES]);
    let tx_buffer: &'static mut [u8] =
        crate::storage::allocate_psram([0u8; TCP_SOCKET_BUFFER_BYTES]);
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

#[cfg(feature = "wifi-auto")]
pub(super) fn build_wifi(
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
    let _ = controller.set_config(&station_wifi_mode(StationConfig::default(), ap_enabled));

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
        let resources = mk_static!(StackResources<6>, StackResources::new());
        let seed = {
            let mut bytes = [0u8; 8];
            Rng::new().read(&mut bytes);
            u64::from_le_bytes(bytes)
        };
        let (stack, runner) = embassy_net::new(interfaces.station, net_config, resources, seed);
        let discovery = psram_udp_socket::<8, 128, 8, 128>(stack);
        let unicast_discovery = psram_udp_socket::<8, 128, 1, 1>(stack);
        let data =
            psram_udp_socket::<8, WIFI_DATA_SOCKET_BUFFER_BYTES, 8, WIFI_DATA_SOCKET_BUFFER_BYTES>(
                stack,
            );
        let wifi_status = AutoWifiStatus::new(&WIFI_SHARED);
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
    #[cfg(feature = "wifi-auto")]
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
        let rendezvous_events = Box::leak(Box::new([TcpRendezvousWireSlot::empty()]));
        let rendezvous_commands = Box::leak(Box::new([TcpRendezvousWireSlot::empty()]));
        let rendezvous_storage = Box::leak(Box::new(TcpRendezvousStorage::new(
            rendezvous_events,
            rendezvous_commands,
        )));
        let rendezvous_rx = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES]));
        let rendezvous_tx = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES]));
        let rendezvous_read = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_READ_BUFFER_BYTES]));
        let rendezvous_framed = Box::leak(Box::new([0u8; TCP_RENDEZVOUS_FRAMED_LEN]));
        let rendezvous_decoder = Box::leak(Box::new(
            personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder::<
                TCP_RENDEZVOUS_FRAME_CAP,
            >::new(),
        ));
        let (rendezvous_server, rendezvous_client) = tcp_rendezvous(
            ap_stack,
            TcpRendezvousBuffers {
                rx: rendezvous_rx,
                tx: rendezvous_tx,
                read: rendezvous_read,
                framed: rendezvous_framed,
                decoder: rendezvous_decoder,
            },
            rendezvous_storage,
        );
        spawner.spawn(tcp_rendezvous_task(rendezvous_server).expect("TCP rendezvous task fits"));
        let ap_discovery = psram_udp_socket::<8, 512, 8, 512>(ap_stack);
        let ap_unicast_discovery = psram_udp_socket::<8, 128, 1, 1>(ap_stack);
        let ap_data =
            psram_udp_socket::<8, WIFI_DATA_SOCKET_BUFFER_BYTES, 8, WIFI_DATA_SOCKET_BUFFER_BYTES>(
                ap_stack,
            );
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
                rendezvous: Some(rendezvous_client),
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

#[cfg(feature = "wifi-auto")]
#[embassy_executor::task]
async fn tcp_rendezvous_task(server: TcpRendezvousServer<'static>) -> ! {
    server.run().await
}

#[cfg(feature = "wifi-auto")]
/// Hold the Wi-Fi controller alive with no AP association — dropping it would stop the radio — so
/// ESP-NOW keeps the Wi-Fi MAC up on a fixed channel when no SSID is configured. The radio was started
/// synchronously by [`build_wifi`] before this task takes the controller.
#[embassy_executor::task]
async fn wifi_radio_keepalive_task(_controller: WifiController<'static>) -> ! {
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

/// Adapts esp-radio's `EspNow` handle to the engine's [`EspNowRadio`] seam — the unsafe-free board
/// side of the boundary, the way the SX1262 driver sits behind `SpiDevice`. Broadcast-only; a
/// transient `NO_MEM` while the radio is off serving a BLE connection event is retried a few times
/// before the frame is dropped for the engine to resend.
#[cfg(feature = "wifi-auto")]
pub(super) struct EspNowAdapter {
    manager: EspNowManager<'static>,
    sender: EspNowSender<'static>,
    receiver: EspNowReceiver<'static>,
    rate_applied: bool,
}

#[cfg(feature = "wifi-auto")]
const ESPNOW_SEND_RETRIES: u8 = 8;
#[cfg(feature = "wifi-auto")]
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);
#[cfg(feature = "wifi-auto")]
pub(super) struct EspNowPhySettings {
    pub(super) driver_rate: WifiPhyRate,
    pub(super) bitrate: BitrateBps,
}
/// The pinned ESP-NOW PHY rate: 802.11g 12 Mbps, QPSK rate-1/2 OFDM. HT/HE *broadcast* RX is
/// hard-pinned to 1 Mbps DSSS by the closed Wi-Fi blob (no public override) so MCS rates transmit but
/// never receive; the legacy OFDM-g family is the broadcast-compatible way to keep OFDM's good
/// multipath, and 12M is the QPSK-1/2 sweet spot (good range at ~the USB-feed budget).
///
/// Off-by-one shim: esp-radio 0.18's `set_rate` casts the sequential `WifiPhyRate` discriminant
/// straight into the C `wifi_phy_rate_t`, which reserves a gap at value 4 — so every variant past the
/// gap programs the rate one slot below its name (`Rate12m` -> C 24M). The discriminant of `Rate6m`
/// (10) equals C `WIFI_PHY_RATE_12M`, so `Rate6m` is what actually selects g-12M. This one spot
/// localizes the workaround; TODO: patch esp-radio's enum upstream and return `Rate12m`.
#[cfg(feature = "wifi-auto")]
pub(super) const ESPNOW_PHY: EspNowPhySettings = EspNowPhySettings {
    driver_rate: WifiPhyRate::Rate6m,
    bitrate: BitrateBps::guess(12_000_000),
};

#[cfg(feature = "wifi-auto")]
impl EspNowAdapter {
    pub(super) fn new(esp_now: EspNow<'static>) -> Self {
        let (manager, sender, receiver) = esp_now.split();
        Self {
            manager,
            sender,
            receiver,
            rate_applied: false,
        }
    }

    /// Pin the PHY rate once, lazily on first transmit — by then the radio is started (set_config runs
    /// before the interface loop in both the associated and off-grid paths), which
    /// `esp_wifi_config_espnow_rate` requires.
    fn ensure_rate(&mut self) {
        if !self.rate_applied {
            let _ = self.manager.set_rate(ESPNOW_PHY.driver_rate);
            self.rate_applied = true;
        }
    }
}

#[cfg(feature = "wifi-auto")]
impl espnow_core::EspNowRadio for EspNowAdapter {
    fn set_channel(&mut self, channel: EspNowChannel) {
        let _ = self.manager.set_channel(channel.as_u8());
    }

    async fn broadcast(&mut self, frame: &[u8]) -> bool {
        self.ensure_rate();
        for _ in 0..ESPNOW_SEND_RETRIES {
            if self
                .sender
                .send_async(&BROADCAST_ADDRESS, frame)
                .await
                .is_ok()
            {
                return true;
            }
            Timer::after(ESPNOW_SEND_RETRY_DELAY).await;
        }
        false
    }

    async fn receive(&mut self, buf: &mut [u8]) -> usize {
        let frame = self.receiver.receive_async().await;
        let data = frame.data();
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        len
    }
}

/// A node pinned to a Wi-Fi access point is channel-locked to it (ESP-NOW must follow the station's
/// channel, never retune and break the association); a node with no Wi-Fi configured is free to sit on
/// the default rendezvous channel. The locked/free seam a future scan-and-follow layer extends.
#[cfg(feature = "wifi-auto")]
pub(super) fn espnow_channel_policy(station_configured: bool) -> ChannelPolicy {
    if station_configured {
        ChannelPolicy::FollowStation
    } else {
        ChannelPolicy::Fixed(EspNowChannel::DEFAULT)
    }
}

#[cfg(feature = "wifi-auto")]
#[embassy_executor::task(pool_size = 2)]
pub(super) async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

#[cfg(feature = "wifi-auto")]
#[embassy_executor::task]
async fn network_ready_task(stack: Stack<'static>) -> ! {
    let mut previous_state = None;
    let mut previous_data_path = None;
    let mut station_reception_recovery = StationReceptionRecovery::new();
    let mut samples_until_report = 0;
    let mut internal_free_low_water = usize::MAX;
    loop {
        let associated = WIFI_STATION_JOINED.load(Ordering::Relaxed);
        let link_up = stack.is_link_up();
        let ipv4 = stack.config_v4();
        let has_ipv4 = ipv4.is_some();
        let state = (associated, link_up, has_ipv4);
        let state_changed = previous_state != Some(state);
        let was_ready = previous_state
            .map(|(_, previous_link, previous_ipv4)| previous_link && previous_ipv4)
            .unwrap_or(false);
        let ready = link_up && has_ipv4;
        let internal_free = esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into());
        let external_free = esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::External.into());
        internal_free_low_water = internal_free_low_water.min(internal_free);

        if ready && !was_ready {
            boot_stage(BootPhase::NetworkReady);
        }
        if state_changed || samples_until_report == 0 {
            let heap = esp_alloc::HEAP.stats();
            let data_path = esp_radio::wifi::data_path_diagnostics();
            let station_ready = associated && ready;
            let reception_window = previous_data_path.as_ref().map(|earlier| {
                if data_path.receive_delivery_blocked_by_transmit_capacity_since(earlier) {
                    StationReceptionWindow::TransmitCapacityBlocked
                } else if data_path.station_receive_progressed_since(earlier) {
                    StationReceptionWindow::ReceiveProgress
                } else if data_path.transmit_progressed_without_station_receive_since(earlier) {
                    StationReceptionWindow::TransmitWithoutReceive
                } else {
                    StationReceptionWindow::NoProgress
                }
            });
            log::info!(
                "wifi-health: associated={} link_up={} ipv4={:?} internal_free={} internal_low={} external_free={} heap_free={} heap_used={} heap_high={}",
                associated,
                link_up,
                ipv4,
                internal_free,
                internal_free_low_water,
                external_free,
                heap.size.saturating_sub(heap.current_usage),
                heap.current_usage,
                heap.max_usage
            );
            log::info!("wifi-data: {}", data_path);
            if station_ready {
                if let Some(reception_window) = reception_window {
                    if matches!(&reception_window, StationReceptionWindow::ReceiveProgress) {
                        WIFI_STATION_RX_DEGRADED.store(false, Ordering::Release);
                    }
                    match station_reception_recovery.observe(reception_window) {
                        StationReceptionAction::Continue => {}
                        StationReceptionAction::Reassert { count } => {
                            WIFI_STATION_RX_DEGRADED.store(true, Ordering::Release);
                            esp_phy::reassert_wifi_rx_enabled();
                            log::warn!(
                                "wifi-health: station RX stalled while TX progressed; reasserted reception count={count}"
                            );
                        }
                        StationReceptionAction::RestartDriver { count } => {
                            WIFI_STATION_RX_DEGRADED.store(true, Ordering::Release);
                            WIFI_RX_RESTART_REQUESTED.store(true, Ordering::Release);
                            log::warn!(
                                "wifi-health: station RX remained stalled after reassertion; requested driver restart count={count}"
                            );
                        }
                    }
                }
            } else {
                station_reception_recovery.station_unavailable();
            }
            previous_data_path = if station_ready { Some(data_path) } else { None };
            samples_until_report = WIFI_HEALTH_SAMPLES_BETWEEN_REPORTS;
        } else {
            samples_until_report = samples_until_report.saturating_sub(1);
        }
        previous_state = Some(state);
        Timer::after(WIFI_LINK_CHECK_INTERVAL).await;
    }
}

#[cfg(feature = "wifi-auto")]
const WIFI_LINK_CHECK_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(feature = "wifi-auto")]
const WIFI_INTER_CHANNEL_DELAY: Duration = Duration::from_millis(25);
#[cfg(feature = "wifi-auto")]
const WIFI_CHANNEL_SCAN_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(feature = "wifi-auto")]
const WIFI_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(feature = "wifi-auto")]
const WIFI_SCAN_MIN_DWELL: HalDuration = HalDuration::from_millis(5);
#[cfg(feature = "wifi-auto")]
const WIFI_SCAN_MAX_DWELL: HalDuration = HalDuration::from_millis(20);
#[cfg(feature = "wifi-auto")]
const DRIVER_STOP_RETRY_DELAY: Duration = Duration::from_millis(25);
#[cfg(feature = "wifi-auto")]
const ESP_OK: i32 = 0;
#[cfg(feature = "wifi-auto")]
const ESP_ERR_WIFI_NOT_INIT: i32 = 12_289;
#[cfg(feature = "wifi-auto")]
const ESP_ERR_WIFI_NOT_STARTED: i32 = 12_290;

#[cfg(feature = "wifi-auto")]
struct StationCredentials {
    ssid: String,
    password: String,
}

#[cfg(feature = "wifi-auto")]
extern "C" {
    fn esp_wifi_disconnect_internal() -> i32;
    fn esp_wifi_scan_stop() -> i32;
}

#[cfg(feature = "wifi-auto")]
#[allow(clippy::undocumented_unsafe_blocks)]
async fn stop_station_connection() {
    let mut reported = None;
    loop {
        let result = unsafe { esp_wifi_disconnect_internal() };
        if matches!(
            result,
            ESP_OK | ESP_ERR_WIFI_NOT_INIT | ESP_ERR_WIFI_NOT_STARTED
        ) {
            return;
        }
        if reported != Some(result) {
            log::warn!("wifi: station stop pending code={result}");
            reported = Some(result);
        }
        Timer::after(DRIVER_STOP_RETRY_DELAY).await;
    }
}

#[cfg(feature = "wifi-auto")]
#[allow(clippy::undocumented_unsafe_blocks)]
async fn stop_station_scan() {
    let mut reported = None;
    loop {
        let result = unsafe { esp_wifi_scan_stop() };
        if matches!(
            result,
            ESP_OK | ESP_ERR_WIFI_NOT_INIT | ESP_ERR_WIFI_NOT_STARTED
        ) {
            return;
        }
        if reported != Some(result) {
            log::warn!("wifi: scan stop pending code={result}");
            reported = Some(result);
        }
        Timer::after(DRIVER_STOP_RETRY_DELAY).await;
    }
}

#[cfg(feature = "wifi-auto")]
#[embassy_executor::task]
async fn wifi_connect_task(
    mut controller: WifiController<'static>,
    status: AutoWifiStatus<MEMBERS>,
    credentials: StationCredentials,
    ap_enabled: bool,
) -> ! {
    let base = StationConfig::default()
        .with_ssid(credentials.ssid.clone())
        .with_password(credentials.password.clone());
    let mut recovery = StationRecovery::new(DiscoveryScope::FullBand);

    loop {
        let mut resumed = false;
        while !status.is_station_uplink_enabled() {
            WIFI_STATION_RX_DEGRADED.store(false, Ordering::Release);
            WIFI_RX_RESTART_REQUESTED.store(false, Ordering::Release);
            if controller.is_connected() {
                let _ = controller.disconnect_async().await;
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            status.wait_until_station_uplink_enabled().await;
            resumed = true;
        }
        if resumed {
            recovery.resume_now();
        }
        if WIFI_RX_RESTART_REQUESTED.swap(false, Ordering::AcqRel) {
            log::warn!("wifi: restarting driver after RX recovery escalation");
            if let Err(error) = controller.restart() {
                WIFI_RX_RESTART_REQUESTED.store(true, Ordering::Release);
                log::warn!("wifi: RX recovery driver restart failed: {error:?}");
                Timer::after(DRIVER_STOP_RETRY_DELAY).await;
                continue;
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            recovery.resume_now();
            continue;
        }
        if controller.is_connected() {
            WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
            match select3(
                controller.wait_for_disconnect_async(),
                status.wait_until_station_uplink_disabled(),
                Timer::after(WIFI_LINK_CHECK_INTERVAL),
            )
            .await
            {
                Either3::First(Ok(disconnected)) => {
                    log::warn!(
                        "wifi: station disconnected ({:?}, rssi {})",
                        disconnected.reason,
                        disconnected.rssi
                    );
                }
                Either3::First(Err(error)) => {
                    log::warn!("wifi: disconnect monitor failed: {error:?}");
                }
                Either3::Second(()) => {
                    let _ = controller.disconnect_async().await;
                }
                Either3::Third(()) => continue,
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            continue;
        }
        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
        if ap_enabled {
            let discovery_scope = match controller.channel() {
                Ok((channel, _)) => match DiscoveryScope::protected(channel) {
                    Some(discovery_scope) => Some(discovery_scope),
                    None => {
                        log::warn!("wifi: SoftAP channel is outside 2.4 GHz channel={channel}");
                        None
                    }
                },
                Err(error) => {
                    log::warn!("wifi: SoftAP channel query failed: {error:?}");
                    None
                }
            };
            let Some(discovery_scope) = discovery_scope else {
                apply_station_yield(StationYield::Retry(RecoveryDelay::TwoSeconds), &status).await;
                continue;
            };
            recovery.set_discovery_scope(discovery_scope);
        } else {
            recovery.set_discovery_scope(DiscoveryScope::FullBand);
        }
        let Some(attempt) = recovery.begin_attempt() else {
            Timer::after(DRIVER_STOP_RETRY_DELAY).await;
            continue;
        };
        match attempt {
            StationAttempt::Connect(attempt) => {
                let access_point = attempt.access_point();
                let station = base
                    .clone()
                    .with_bssid(access_point.bssid)
                    .with_channel(access_point.channel);
                let configured = {
                    let mode = station_wifi_mode(station, ap_enabled);
                    match controller.set_config(&mode) {
                        Ok(()) => true,
                        Err(error) => {
                            log::warn!("wifi: station configuration failed: {error:?}");
                            false
                        }
                    }
                };
                if !configured {
                    let next = recovery.finish_connection(
                        attempt,
                        ConnectionOutcome::Failed(ConnectionFailure::Driver),
                    );
                    apply_station_yield(next, &status).await;
                    continue;
                }
                if !status.is_station_uplink_enabled() {
                    let next = recovery.finish_connection(attempt, ConnectionOutcome::Cancelled);
                    recovery.resume_now();
                    apply_station_yield(next, &status).await;
                    continue;
                }
                boot_stage(BootPhase::WifiConnectionBegin);
                let started_at = embassy_time::Instant::now().as_millis();
                log::info!(
                    "wifi: station connection begin channel={}",
                    access_point.channel
                );
                let connected = embassy_futures::select::select(
                    with_timeout(WIFI_CONNECT_TIMEOUT, controller.connect_async()),
                    status.wait_until_station_uplink_disabled(),
                )
                .await;
                let next = match connected {
                    embassy_futures::select::Either::First(Ok(Ok(connected))) => {
                        WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
                        WIFI_STATION_RX_DEGRADED.store(false, Ordering::Release);
                        boot_stage(BootPhase::WifiAssociated);
                        log::info!(
                            "wifi: station connected channel={} elapsed_ms={}",
                            connected.channel,
                            embassy_time::Instant::now()
                                .as_millis()
                                .saturating_sub(started_at)
                        );
                        let next = recovery.finish_connection(
                            attempt,
                            ConnectionOutcome::Connected(StationAccessPoint {
                                bssid: connected.bssid,
                                channel: connected.channel,
                            }),
                        );
                        if let Err(error) = controller.set_power_saving(PowerSaveMode::None) {
                            log::warn!("wifi: power-save configuration failed: {error:?}");
                        }
                        next
                    }
                    embassy_futures::select::Either::First(Ok(Err(error))) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        match error {
                            WifiError::Disconnected(disconnected) => log::warn!(
                                "wifi: station connection failed ({:?}, rssi {}) elapsed_ms={}",
                                disconnected.reason,
                                disconnected.rssi,
                                embassy_time::Instant::now()
                                    .as_millis()
                                    .saturating_sub(started_at)
                            ),
                            other => log::warn!(
                                "wifi: station connection failed: {other:?} elapsed_ms={}",
                                embassy_time::Instant::now()
                                    .as_millis()
                                    .saturating_sub(started_at)
                            ),
                        }
                        let failure = classify_connection_failure(error);
                        recovery.finish_connection(attempt, ConnectionOutcome::Failed(failure))
                    }
                    embassy_futures::select::Either::First(Err(_)) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        log::warn!(
                            "wifi: station connection timed out elapsed_ms={}",
                            embassy_time::Instant::now()
                                .as_millis()
                                .saturating_sub(started_at)
                        );
                        stop_station_connection().await;
                        recovery.finish_connection(
                            attempt,
                            ConnectionOutcome::Failed(ConnectionFailure::Timeout),
                        )
                    }
                    embassy_futures::select::Either::Second(()) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        stop_station_connection().await;
                        let next =
                            recovery.finish_connection(attempt, ConnectionOutcome::Cancelled);
                        recovery.resume_now();
                        next
                    }
                };
                apply_station_yield(next, &status).await;
            }
            StationAttempt::Scan(attempt) => {
                let channel = attempt.channel();
                if attempt.starts_sweep() {
                    boot_stage(BootPhase::WifiDiscoveryBegin);
                    log::info!("wifi: discovery sweep begin");
                }
                let scan_config = ScanConfig::default()
                    .with_ssid(credentials.ssid.as_str())
                    .with_channel(channel)
                    .with_scan_type(ScanTypeConfig::Active {
                        min: WIFI_SCAN_MIN_DWELL,
                        max: WIFI_SCAN_MAX_DWELL,
                    })
                    .with_max(8);
                let scan = embassy_futures::select::select(
                    with_timeout(
                        WIFI_CHANNEL_SCAN_TIMEOUT,
                        controller.scan_async(&scan_config),
                    ),
                    status.wait_until_station_uplink_disabled(),
                )
                .await;
                let next = match scan {
                    embassy_futures::select::Either::First(Ok(Ok(networks))) => {
                        let best = networks
                            .iter()
                            .max_by_key(|access_point| access_point.signal_strength)
                            .map(|access_point| StationAccessPoint {
                                bssid: access_point.bssid,
                                channel: access_point.channel,
                            });
                        if best.is_some() || attempt.ends_sweep() {
                            boot_stage(BootPhase::WifiDiscoveryComplete);
                        }
                        if best.is_some() {
                            log::info!("wifi: discovery found channel={channel}");
                        } else if attempt.ends_sweep() {
                            log::warn!("wifi: configured network absent");
                        }
                        let outcome = best.map_or(ScanOutcome::NotFound, ScanOutcome::Found);
                        recovery.finish_scan(attempt, outcome)
                    }
                    embassy_futures::select::Either::First(Ok(Err(error))) => {
                        log::warn!("wifi: discovery scan failed channel={channel}: {error:?}");
                        stop_station_scan().await;
                        recovery.finish_scan(attempt, ScanOutcome::Failed(ScanFailure::Driver))
                    }
                    embassy_futures::select::Either::First(Err(_)) => {
                        log::warn!("wifi: discovery scan timed out channel={channel}");
                        stop_station_scan().await;
                        recovery.finish_scan(attempt, ScanOutcome::Failed(ScanFailure::Timeout))
                    }
                    embassy_futures::select::Either::Second(()) => {
                        stop_station_scan().await;
                        let next = recovery.finish_scan(attempt, ScanOutcome::Cancelled);
                        recovery.resume_now();
                        next
                    }
                };
                apply_station_yield(next, &status).await;
            }
        }
    }
}

#[cfg(feature = "wifi-auto")]
fn classify_connection_failure(error: WifiError) -> ConnectionFailure {
    match error {
        WifiError::InvalidPassword => ConnectionFailure::Authentication,
        WifiError::InvalidSsid => ConnectionFailure::NetworkNotFound,
        WifiError::Disconnected(disconnected) => match disconnected.reason {
            DisconnectReason::NoAccessPointFound
            | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
            | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
            | DisconnectReason::NoAccessPointFoundInRssiThreshold => {
                ConnectionFailure::NetworkNotFound
            }
            DisconnectReason::AuthenticationExpired
            | DisconnectReason::AssociationNotAuthenticated
            | DisconnectReason::FourWayHandshakeTimeout
            | DisconnectReason::GroupKeyUpdateTimeout
            | DisconnectReason::_802_1xAuthenticationFailed
            | DisconnectReason::AuthenticationFailed
            | DisconnectReason::HandshakeTimeout => ConnectionFailure::Authentication,
            DisconnectReason::Timeout | DisconnectReason::BeaconTimeout => {
                ConnectionFailure::Timeout
            }
            _ => ConnectionFailure::Driver,
        },
        _ => ConnectionFailure::Driver,
    }
}

#[cfg(feature = "wifi-auto")]
async fn apply_station_yield(next: StationYield, status: &AutoWifiStatus<MEMBERS>) {
    match next {
        StationYield::Continue | StationYield::MonitorLink | StationYield::Disabled => {}
        StationYield::InterChannel => {
            let _ = embassy_futures::select::select(
                Timer::after(WIFI_INTER_CHANNEL_DELAY),
                status.wait_until_station_uplink_disabled(),
            )
            .await;
        }
        StationYield::Retry(delay) => {
            let delay_seconds = delay.seconds();
            log::info!("wifi: station recovery delay_secs={delay_seconds}");
            let _ = embassy_futures::select::select(
                Timer::after(Duration::from_secs(delay_seconds)),
                status.wait_until_station_uplink_disabled(),
            )
            .await;
        }
    }
}
