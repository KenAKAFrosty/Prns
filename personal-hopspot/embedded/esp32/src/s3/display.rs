use super::*;

fn classify_card(
    id: InterfaceId,
    usb_id: InterfaceId,
    wifi_id: Option<InterfaceId>,
    tcp_id: Option<InterfaceId>,
    lora_id: InterfaceId,
    espnow_id: Option<InterfaceId>,
) -> Option<(screen::CardKind, screen::CardLabel)> {
    if id == usb_id {
        Some((screen::CardKind::Usb, screen::card_label("USB")))
    } else if id == lora_id {
        Some((screen::CardKind::LoRa, screen::card_label("LoRa")))
    } else if Some(id) == wifi_id {
        Some((screen::CardKind::Wifi, screen::card_label("Wi-Fi/LAN")))
    } else if Some(id) == espnow_id {
        Some((screen::CardKind::EspNow, screen::card_label("ESP-NOW")))
    } else if Some(id) == tcp_id {
        Some((
            screen::CardKind::Tcp,
            screen::tcp_card_label(HOPSPOT_TCP_TARGET),
        ))
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
    lora: &EmbassyInterfaceStatus,
    espnow: Option<&EmbassyInterfaceStatus>,
) -> HVec<InterfaceSnapshot, 8> {
    use personal_rns::interfaces::InterfaceStatus;
    #[cfg(feature = "bluetooth-auto")]
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: HVec<(&dyn InterfaceStatus, Membership), 8> = HVec::new();
    let _ = entries.push((lora, Membership::Independent));
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
    for (status, membership) in &entries {
        let id = status.id();
        let counts = INTERFACE_STORE.counts(id);
        let _ = snapshots.push(InterfaceSnapshot {
            id,
            connection: status.connection(),
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
    lora_id: InterfaceId,
    espnow_id: Option<InterfaceId>,
) -> HVec<screen::Card, 8> {
    screen::snapshots_to_cards(snapshots, |id| {
        classify_card(id, usb_id, wifi_id, tcp_id, lora_id, espnow_id)
    })
}

fn egress_pressure_events(id: InterfaceId) -> u32 {
    match id.kind() {
        Some(InterfaceKind::UsbAutoDevice) => USB_MANIFOLD_LANE.egress_pressure_events(),
        Some(InterfaceKind::TcpClient) => TCP_MANIFOLD_LANE.egress_pressure_events(),
        Some(InterfaceKind::AutoWifi | InterfaceKind::WifiPeer) => {
            WIFI_MANIFOLD_LANE.egress_pressure_events()
        }
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
        Some(InterfaceKind::AutoWifi | InterfaceKind::WifiPeer) => {
            WIFI_MANIFOLD_LANE.ingress_pressure_events()
        }
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
        details.push_ingress_pressure(ingress_pressure_events(card.id()));
        details.push_egress_pressure(egress_pressure_events(card.id()));
    }
}

#[cfg(feature = "wifi-auto")]
pub(super) fn build_interface_menu_details(
    selected_card: Option<&screen::Card>,
    snapshots: &[InterfaceSnapshot],
    usb: &EmbassyInterfaceStatus,
    wifi_config: &HopspotWifiConfig,
    ap_ssid: Option<&str>,
) -> screen::InterfaceMenuDetails {
    let mut details = match selected_card.map(|card| card.kind()) {
        Some(screen::CardKind::Wifi) => {
            let station_ssid = (wifi_config.has_station()
                && WIFI_STATION_JOINED.load(Ordering::Relaxed))
            .then_some(wifi_config.ssid.as_str());
            screen::wifi_interface_menu_details(
                screen::WifiNetworkStatus {
                    station_ssid,
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
        _ => screen::InterfaceMenuDetails::empty(),
    };
    add_manifold_pressure(&mut details, selected_card);
    details
}
