#[cfg(feature = "wifi-auto")]
use super::captive_portal::station_wifi_mode;
#[cfg(feature = "wifi-auto")]
use super::captive_portal::{build_ap_netif, dhcp_server_task, dns_server_task, http_server_task};
use super::*;
#[cfg(feature = "wifi-auto")]
use static_cell::ConstStaticCell;

pub(super) fn build_tcp(
    stack: Stack<'static>,
) -> Option<(
    TcpClient<'static>,
    &'static EmbassyInterfaceStatus,
    InterfaceId,
)> {
    let addr = HOPSPOT_TCP_TARGET.parse::<::core::net::SocketAddr>().ok()?;
    let target = IpEndpoint::new(addr.ip().into(), addr.port());
    let channel_tag = HOPSPOT_TCP_TARGET.as_bytes();
    let id = TcpClient::interface_id(channel_tag);
    let status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(id, ConnectionState::Initializing)
    );
    let rx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
    let tx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
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
    // Trim WiFi RX buffering from the defaults (static_rx 10, rx_ba_win 6) so the full radio stack +
    // SoftAP fits in internal DMA SRAM: each static RX buffer is ~1.6 KiB, internal and never freed,
    // and Reticulum's small frames don't need deep buffering. The captive portal's DNS socket needs
    // AP join-time margin too, so this stays one notch tighter than the earlier 4/3 floor. (The
    // 16 KiB D-cache lever is unusable here — the S3 BT controller ROM requires a 32 KiB cache,
    // ESP-IDF #10268.)
    let wifi_config = ControllerConfig::default()
        .with_static_rx_buf_num(3)
        .with_rx_ba_win(2);
    let Ok((mut controller, interfaces)) = esp_radio::wifi::new(wifi, wifi_config) else {
        return (None, None, None);
    };
    let esp_now = interfaces.esp_now;

    // In SoftAP mode, APSTA brings the AP up whether or not a station uplink is configured;
    // set_config calls esp_wifi_start, so the AP is live here on core 0.
    let _ = controller.set_config(&station_wifi_mode(StationConfig::default(), ap_enabled));

    // Opportunistic station uplink: only a configured SSID stands a station netif up and runs
    // the connect loop; otherwise the keepalive task just owns the controller, no scanning.
    let station_segment: Option<AutoWifiSegment<'static>> = if config.has_station() {
        let link_local = wifi_auto_contract::link_local_from_mac(MacAddress::new(mac));
        // Dual-stack: the v6 link-local carries WiFi-auto's discovery/data UDP; v4 over DHCP gives
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
        let discovery = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
            UdpSocket::new(
                stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let data = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
            UdpSocket::new(
                stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let wifi_status = AutoWifiStatus::new(&WIFI_SHARED);
        spawner.spawn(net_task(runner).expect("net task fits"));
        spawner.spawn(
            wifi_connect_task(controller, wifi_status, config.clone(), ap_enabled)
                .expect("wifi connect task fits"),
        );
        Some(AutoWifiSegment {
            stack,
            discovery,
            data,
            mac,
        })
    } else {
        spawner
            .spawn(wifi_radio_keepalive_task(controller).expect("wifi radio keepalive task fits"));
        None
    };
    let tcp_stack = station_segment.as_ref().map(|segment| segment.stack);

    // In explicit SoftAP mode, the AP is the primary WiFi-auto segment and the station (if any) folds
    // in as the opportunistic secondary. The AP link-local is the station MAC + 1 (build_ap_netif
    // derives it from `mac`), and the supervisor hashes its peering token over that AP link-local, so
    // it takes `ap_mac`.
    #[cfg(feature = "wifi-auto")]
    if ap_enabled {
        let mut ap_mac = mac;
        ap_mac[5] = ap_mac[5].wrapping_add(1);
        let ap_stack = build_ap_netif(spawner, interfaces.access_point, mac);
        // Hand joiners a 192.168.4.x lease with the SoftAP as their default gateway, so their WiFi-auto
        // client auto-dials the TCP rendezvous on the gateway (multicast can't cross the SoftAP).
        spawner.spawn(dhcp_server_task(ap_stack).expect("dhcp server task fits"));
        spawner.spawn(dns_server_task(ap_stack).expect("dns server task fits"));
        for _ in 0..4 {
            spawner.spawn(http_server_task(ap_stack).expect("http server task fits"));
        }
        let ap_discovery = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
            UdpSocket::new(
                ap_stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let ap_data = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
            UdpSocket::new(
                ap_stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let wifi = AutoWifi::new(
            AutoWifiTopology {
                primary: AutoWifiSegment {
                    stack: ap_stack,
                    discovery: ap_discovery,
                    data: ap_data,
                    mac: ap_mac,
                },
                secondary: station_segment,
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
                },
                &WIFI_SHARED,
            );
            (Some(wifi), tcp_stack, Some(esp_now))
        }
        None => (None, None, Some(esp_now)),
    }
}

#[cfg(feature = "wifi-auto")]
/// Hold the WiFi controller alive with no AP association — dropping it would stop the radio — so
/// ESP-NOW keeps the WiFi MAC up on a fixed channel when no SSID is configured. The radio was started
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
/// The pinned ESP-NOW PHY rate: 802.11g 12 Mbps, QPSK rate-1/2 OFDM. HT/HE *broadcast* RX is
/// hard-pinned to 1M DSSS by the closed WiFi blob (no public override) so MCS rates transmit but
/// never receive; the legacy OFDM-g family is the broadcast-compatible way to keep OFDM's good
/// multipath, and 12M is the QPSK-1/2 sweet spot (good range at ~the USB-feed budget).
///
/// Off-by-one shim: esp-radio 0.18's `set_rate` casts the sequential `WifiPhyRate` discriminant
/// straight into the C `wifi_phy_rate_t`, which reserves a gap at value 4 — so every variant past the
/// gap programs the rate one slot below its name (`Rate12m` -> C 24M). The discriminant of `Rate6m`
/// (10) equals C `WIFI_PHY_RATE_12M`, so `Rate6m` is what actually selects g-12M. This one spot
/// localizes the workaround; TODO: patch esp-radio's enum upstream and return `Rate12m`.
#[cfg(feature = "wifi-auto")]
const fn espnow_phy_rate() -> WifiPhyRate {
    WifiPhyRate::Rate6m
}

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
            let _ = self.manager.set_rate(espnow_phy_rate());
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

/// A node pinned to a WiFi access point is channel-locked to it (ESP-NOW must follow the station's
/// channel, never retune and break the association); a node with no WiFi configured is free to sit on
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
const WIFI_LINK_CHECK_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(feature = "wifi-auto")]
const WIFI_RETRY_DELAY: Duration = Duration::from_secs(2);
#[cfg(feature = "wifi-auto")]
const WIFI_SCAN_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(feature = "wifi-auto")]
const WIFI_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(feature = "wifi-auto")]
/// A mesh (e.g. eero) hands the same SSID out on many BSSIDs across its nodes and bands and bridges
/// multicast between them unreliably, so a station left to roam can land on a node that never
/// receives the discovery group. To avoid that, this scans first and pins to the strongest BSSID
/// for the SSID — landing the Heltec V4 on one node and holding it there, where the discovery
/// multicast reaches it.
#[embassy_executor::task]
async fn wifi_connect_task(
    mut controller: WifiController<'static>,
    status: AutoWifiStatus<MEMBERS>,
    config: HopspotWifiConfig,
    ap_enabled: bool,
) -> ! {
    let base = StationConfig::default()
        .with_ssid(config.ssid.clone())
        .with_password(config.password.clone());

    let _ = controller.set_config(&station_wifi_mode(base.clone(), ap_enabled));
    loop {
        while !status.is_enabled() {
            if controller.is_connected() {
                let _ = controller.disconnect_async().await;
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            status.wait_until_radio_enabled().await;
        }
        if controller.is_connected() {
            WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
            match select3(
                controller.wait_for_disconnect_async(),
                status.wait_until_radio_disabled(),
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
        let mut station = base.clone();
        let scan = embassy_futures::select::select(
            with_timeout(
                WIFI_SCAN_TIMEOUT,
                controller.scan_async(&ScanConfig::default()),
            ),
            status.wait_until_radio_disabled(),
        )
        .await;
        match scan {
            embassy_futures::select::Either::First(Ok(Ok(networks))) => {
                let mut best: Option<([u8; 6], u8, i8)> = None;
                for ap in &networks {
                    if ap.ssid.as_str() == config.ssid.as_str()
                        && best.is_none_or(|(_, _, rssi)| ap.signal_strength > rssi)
                    {
                        best = Some((ap.bssid, ap.channel, ap.signal_strength));
                    }
                }
                if let Some((bssid, channel, rssi)) = best {
                    log::info!(
                        "wifi: pinned to BSSID {:02x?} channel {} (rssi {})",
                        bssid,
                        channel,
                        rssi
                    );
                    station = base.clone().with_bssid(bssid).with_channel(channel);
                } else {
                    log::warn!("wifi: configured network absent from scan");
                }
            }
            embassy_futures::select::Either::First(Ok(Err(error))) => {
                log::warn!("wifi: scan failed: {error:?}");
                Timer::after(WIFI_RETRY_DELAY).await;
                continue;
            }
            embassy_futures::select::Either::First(Err(_)) => {
                log::warn!("wifi: scan timed out");
                Timer::after(WIFI_RETRY_DELAY).await;
                continue;
            }
            embassy_futures::select::Either::Second(()) => {
                continue;
            }
        }
        if !status.is_enabled() {
            continue;
        }
        if let Err(error) = controller.set_config(&station_wifi_mode(station, ap_enabled)) {
            log::warn!("wifi: station configuration failed: {error:?}");
            let _ = embassy_futures::select::select(
                Timer::after(WIFI_RETRY_DELAY),
                status.wait_until_radio_disabled(),
            )
            .await;
            continue;
        }
        if !status.is_enabled() {
            continue;
        }
        let connected = embassy_futures::select::select(
            with_timeout(WIFI_CONNECT_TIMEOUT, controller.connect_async()),
            status.wait_until_radio_disabled(),
        )
        .await;
        match connected {
            embassy_futures::select::Either::First(Ok(Ok(connected))) => {
                WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
                log::info!(
                    "wifi: station connected to BSSID {:02x?} channel {}",
                    connected.bssid,
                    connected.channel
                );
                if let Err(error) = controller.set_power_saving(PowerSaveMode::None) {
                    log::warn!("wifi: power-save configuration failed: {error:?}");
                }
            }
            embassy_futures::select::Either::First(Ok(Err(error))) => {
                WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                match error {
                    WifiError::Disconnected(disconnected) => log::warn!(
                        "wifi: station connection failed ({:?}, rssi {})",
                        disconnected.reason,
                        disconnected.rssi
                    ),
                    other => log::warn!("wifi: station connection failed: {other:?}"),
                }
                let _ = embassy_futures::select::select(
                    Timer::after(WIFI_RETRY_DELAY),
                    status.wait_until_radio_disabled(),
                )
                .await;
            }
            embassy_futures::select::Either::First(Err(_)) => {
                WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                log::warn!("wifi: station connection timed out");
                let _ = embassy_futures::select::select(
                    Timer::after(WIFI_RETRY_DELAY),
                    status.wait_until_radio_disabled(),
                )
                .await;
            }
            embassy_futures::select::Either::Second(()) => {
                WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            }
        }
    }
}
