use super::*;

fn classify_card(
    id: InterfaceId,
    usb_id: InterfaceId,
    wifi_id: Option<InterfaceId>,
    tcp_id: Option<InterfaceId>,
    tcp_client: Option<&HopspotTcpClientConfig>,
    wifi_kind: screen::CardKind,
    lora_id: Option<InterfaceId>,
    espnow_id: Option<InterfaceId>,
) -> Option<(screen::CardKind, screen::CardLabel)> {
    if id == usb_id {
        Some((screen::CardKind::Usb, screen::card_label("USB")))
    } else if Some(id) == lora_id {
        Some((screen::CardKind::LoRa, screen::card_label("LoRa")))
    } else if Some(id) == wifi_id {
        Some((wifi_kind, screen::card_label("LAN")))
    } else if Some(id) == espnow_id {
        Some((screen::CardKind::EspNow, screen::card_label("ESP-NOW")))
    } else if Some(id) == tcp_id {
        let tcp_client = tcp_client?;
        let label = match &tcp_client.host {
            HopspotTcpClientHost::Ipv4(address) => {
                let mut target = heapless::String::<22>::new();
                let octets = address.octets();
                let _ = write!(
                    target,
                    "{}.{}.{}.{}:{}",
                    octets[0], octets[1], octets[2], octets[3], tcp_client.port
                );
                screen::tcp_card_label(target.as_str())
            }
            HopspotTcpClientHost::Hostname(hostname) => screen::tcp_card_label(hostname),
        };
        Some((screen::CardKind::Tcp, label))
    } else {
        #[cfg(feature = "bluetooth-auto")]
        if id == BLE_SUPERVISOR_ID {
            return Some((screen::CardKind::Ble, screen::card_label("BLE")));
        }
        let bytes = id.as_bytes();
        let mut label = screen::CardLabel::new();
        let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
        Some((screen::CardKind::Peer, label))
    }
}

#[embassy_executor::task]
pub(super) async fn button_task(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        match embassy_futures::select::select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            embassy_futures::select::Either::First(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::ShortPress).await
            }
            embassy_futures::select::Either::Second(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

pub(super) fn build_snapshots(
    usb: &EmbassyInterfaceStatus,
    wifi: Option<&AutoWifiStatus<MEMBERS>>,
    tcp: Option<&EmbassyInterfaceStatus>,
    lora: Option<&EmbassyInterfaceStatus>,
    espnow: Option<&EmbassyInterfaceStatus>,
) -> HVec<InterfaceSnapshot, 8> {
    use personal_rns::interfaces::InterfaceStatus;
    #[cfg(feature = "bluetooth-auto")]
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: HVec<(&dyn InterfaceStatus, Membership), 8> = HVec::new();
    if let Some(lora) = lora {
        let _ = entries.push((lora, Membership::Independent));
    }
    #[cfg(feature = "bluetooth-auto")]
    {
        let _ = entries.push((&ble, Membership::Independent));
    }
    if let Some(wifi) = wifi {
        let _ = entries.push((wifi, Membership::Independent));
    }
    if let Some(espnow) = espnow {
        let _ = entries.push((espnow, Membership::Independent));
    }
    if let Some(tcp) = tcp {
        let _ = entries.push((tcp, Membership::Independent));
    }
    let _ = entries.push((usb, Membership::Independent));

    if let Some(wifi) = wifi {
        let supervisor_id = wifi.id();
        for member in wifi.members() {
            let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
        }
    }
    #[cfg(feature = "bluetooth-auto")]
    {
        let supervisor_id = ble.id();
        for member in ble.members() {
            let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
        }
    }
    let mut snapshots: HVec<InterfaceSnapshot, 8> = HVec::new();
    let wifi_id = wifi.map(InterfaceStatus::id);
    for (status, membership) in &entries {
        let id = status.id();
        let counts = INTERFACE_STORE.counts(id);
        let connection =
            if Some(id) == wifi_id && WIFI_STATION_DATA_PATH_DEGRADED.load(Ordering::Acquire) {
                if WIFI_STATION_JOINED.load(Ordering::Relaxed) {
                    ConnectionState::Degraded
                } else {
                    ConnectionState::Reconnecting
                }
            } else {
                status.connection()
            };
        let _ = snapshots.push(InterfaceSnapshot {
            id,
            mode: personal_rns::interfaces::InterfaceMode::Full,
            gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
            connection,
            failure_reason: status.failure_reason(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            destinations: counts.destinations,
            links: counts.links,
            transported_links: counts.transported_links,
            membership: *membership,
        });
    }
    snapshots
}

pub(super) fn build_cards(
    snapshots: &[InterfaceSnapshot],
    usb_id: InterfaceId,
    wifi_id: Option<InterfaceId>,
    tcp_id: Option<InterfaceId>,
    tcp_client: Option<&HopspotTcpClientConfig>,
    wifi: Option<&AutoWifiStatus<MEMBERS>>,
    wifi_config: &HopspotWifiConfig,
    lora_id: Option<InterfaceId>,
    espnow_id: Option<InterfaceId>,
) -> HVec<screen::Card, 8> {
    let wifi_kind = if !wifi_config.has_station() {
        screen::CardKind::Wifi
    } else if wifi.is_some_and(|status| status.is_station_uplink_enabled()) {
        screen::CardKind::WifiStation
    } else {
        screen::CardKind::WifiStationDisabled
    };
    screen::snapshots_to_cards(snapshots, |id| {
        classify_card(
            id, usb_id, wifi_id, tcp_id, tcp_client, wifi_kind, lora_id, espnow_id,
        )
    })
}

fn egress_pressure_events(id: InterfaceId) -> u32 {
    match id.kind() {
        Some(InterfaceKind::UsbAutoDevice) => USB_MANIFOLD_LANE.egress_pressure_events(),
        Some(InterfaceKind::TcpClient) => TCP_MANIFOLD_LANE.egress_pressure_events(),
        Some(InterfaceKind::AutoWifi | InterfaceKind::WifiPeer | InterfaceKind::TcpServerPeer) => {
            WIFI_MANIFOLD_LANE.egress_pressure_events()
        }
        #[cfg(feature = "lora")]
        Some(InterfaceKind::LoRa) => LORA_MANIFOLD_LANE.egress_pressure_events(),
        #[cfg(feature = "bluetooth-auto")]
        Some(InterfaceKind::BluetoothAuto | InterfaceKind::BluetoothPeer) => {
            BLE_MANIFOLD_LANE.egress_pressure_events()
        }
        #[cfg(feature = "esp-now")]
        Some(InterfaceKind::EspNow) => ESPNOW_MANIFOLD_LANE.egress_pressure_events(),
        _ => 0,
    }
}

fn ingress_pressure_events(id: InterfaceId) -> u32 {
    match id.kind() {
        Some(InterfaceKind::UsbAutoDevice) => USB_MANIFOLD_LANE.ingress_pressure_events(),
        Some(InterfaceKind::TcpClient) => TCP_MANIFOLD_LANE.ingress_pressure_events(),
        Some(InterfaceKind::AutoWifi | InterfaceKind::WifiPeer | InterfaceKind::TcpServerPeer) => {
            WIFI_MANIFOLD_LANE.ingress_pressure_events()
        }
        #[cfg(feature = "lora")]
        Some(InterfaceKind::LoRa) => LORA_MANIFOLD_LANE.ingress_pressure_events(),
        #[cfg(feature = "bluetooth-auto")]
        Some(InterfaceKind::BluetoothAuto | InterfaceKind::BluetoothPeer) => BLE_MANIFOLD_LANE
            .ingress_pressure_events()
            .saturating_add(BluetoothAutoStatus::new(&BLE_SHARED).ingress_pressure_events()),
        #[cfg(feature = "esp-now")]
        Some(InterfaceKind::EspNow) => ESPNOW_MANIFOLD_LANE.ingress_pressure_events(),
        _ => 0,
    }
}

pub(super) fn add_manifold_pressure(
    details: &mut screen::InterfaceMenuDetails,
    selected_card: Option<&screen::Card>,
) {
    if let Some(card) = selected_card {
        #[cfg(feature = "bluetooth-auto")]
        if matches!(
            card.id().kind(),
            Some(InterfaceKind::BluetoothAuto | InterfaceKind::BluetoothPeer)
        ) {
            let recovery = BluetoothAutoStatus::new(&BLE_SHARED).recovery_counters();
            details.push_bluetooth_recovery(screen::BluetoothRecoveryMenuDetails {
                receive_pressure: recovery.ingress_pressure,
                setup_failures: recovery.setup_failures,
                transport_closures: recovery.transport_closures,
            });
        } else {
            details.push_ingress_pressure(ingress_pressure_events(card.id()));
        }
        #[cfg(not(feature = "bluetooth-auto"))]
        details.push_ingress_pressure(ingress_pressure_events(card.id()));
        details.push_egress_pressure(egress_pressure_events(card.id()));
    }
}

#[cfg(feature = "lora")]
pub(super) fn add_lora_spectrum(
    details: &mut screen::InterfaceMenuDetails,
    selected_card: Option<&screen::Card>,
    spectrum: &LoRaSpectrumStatus,
) {
    if !selected_card.is_some_and(|card| card.kind() == screen::CardKind::LoRa) {
        return;
    }
    let snapshot = spectrum.snapshot();
    details.push_lora_spectrum(screen::LoRaSpectrumMenuDetails {
        channel_busy_per_mille: snapshot.channel_busy_per_mille,
        noise_floor_dbm: snapshot.noise_floor_dbm,
        cca_threshold_dbm: snapshot.cca_threshold_dbm,
        deferrals: snapshot.deferrals,
        false_preambles: snapshot.false_preambles,
        contention_timeouts: snapshot.contention_timeouts,
        duty_holds: snapshot.duty_holds,
        duty_timeouts: snapshot.duty_timeouts,
        radio_recoveries: snapshot.radio_recoveries,
    });
}

#[cfg(feature = "wifi-auto")]
pub(super) fn build_interface_menu_details(
    selected_card: Option<&screen::Card>,
    snapshots: &[InterfaceSnapshot],
    usb: &EmbassyInterfaceStatus,
    #[cfg(feature = "lora")] lora_spectrum: &LoRaSpectrumStatus,
    wifi: Option<&AutoWifiStatus<MEMBERS>>,
    wifi_config: &HopspotWifiConfig,
    ap_ssid: Option<&str>,
) -> screen::InterfaceMenuDetails {
    let mut details = match selected_card.map(|card| card.kind()) {
        Some(
            screen::CardKind::Wifi
            | screen::CardKind::WifiStation
            | screen::CardKind::WifiStationDisabled,
        ) => {
            let station = if !wifi_config.has_station() {
                screen::WifiStationStatus::Unconfigured
            } else if wifi.is_some_and(|status| !status.is_station_uplink_enabled()) {
                screen::WifiStationStatus::Disabled
            } else if WIFI_STATION_JOINED.load(Ordering::Relaxed) {
                screen::WifiStationStatus::Connected(wifi_config.ssid.as_str())
            } else {
                screen::WifiStationStatus::Joining
            };
            screen::wifi_interface_menu_details(
                screen::WifiNetworkStatus {
                    station,
                    access_point_ssid: ap_ssid,
                },
                selected_card,
                snapshots,
            )
        }
        Some(screen::CardKind::Usb) => screen::usb_interface_menu_details(usb.connection()),
        Some(screen::CardKind::Ble) => {
            screen::snapshots_to_interface_menu_details(selected_card, snapshots)
        }
        Some(screen::CardKind::Tcp) => {
            let mut details = screen::InterfaceMenuDetails::empty();
            if let Some(tcp) = wifi_config.tcp_client.as_ref() {
                match &tcp.host {
                    HopspotTcpClientHost::Ipv4(address) => {
                        let mut target = heapless::String::<15>::new();
                        let octets = address.octets();
                        let _ = write!(
                            target,
                            "{}.{}.{}.{}",
                            octets[0], octets[1], octets[2], octets[3]
                        );
                        details.push_tcp_target(target.as_str(), tcp.port);
                    }
                    HopspotTcpClientHost::Hostname(hostname) => {
                        details.push_tcp_target(hostname, tcp.port);
                    }
                }
            }
            details
        }
        _ => screen::InterfaceMenuDetails::empty(),
    };
    #[cfg(feature = "lora")]
    add_lora_spectrum(&mut details, selected_card, lora_spectrum);
    add_manifold_pressure(&mut details, selected_card);
    details
}
