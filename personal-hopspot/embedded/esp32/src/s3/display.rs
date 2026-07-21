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
        Some((screen::CardKind::Wifi, screen::card_label("WiFi/LAN")))
    } else if Some(id) == espnow_id {
        Some((screen::CardKind::EspNow, screen::card_label("ESP-NOW")))
    } else if Some(id) == tcp_id {
        Some((
            screen::CardKind::Tcp,
            screen::tcp_card_label(HOPSPOT_TCP_TARGET),
        ))
    } else {
        #[cfg(feature = "bluetooth-auto")]
        if id == BLE_FLEET_ID {
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

#[cfg(feature = "wifi-auto")]
pub(super) fn build_interface_menu_details(
    selected_card: Option<&screen::Card>,
    snapshots: &[InterfaceSnapshot],
    usb: &EmbassyInterfaceStatus,
    wifi_config: &HopspotWifiConfig,
    ap_ssid: Option<&str>,
) -> screen::InterfaceMenuDetailRows {
    let mut rows = screen::InterfaceMenuDetailRows::new();
    match selected_card.map(|card| card.kind) {
        Some(screen::CardKind::Wifi) => {
            let station_ssid =
                if wifi_config.has_station() && WIFI_STATION_JOINED.load(Ordering::Relaxed) {
                    wifi_config.ssid.as_str()
                } else {
                    "None"
                };
            screen::push_interface_menu_info(&mut rows, "STA", station_ssid);
            screen::push_interface_menu_info(&mut rows, "AP", ap_ssid.unwrap_or("None"));
            let _ = screen::push_snapshot_supervisor_peer_rows(&mut rows, selected_card, snapshots);
        }
        Some(screen::CardKind::Usb) => {
            let liveness = screen::liveness_from_connection(usb.connection());
            let peer = (liveness == screen::Liveness::Live).then_some(liveness);
            let _ = screen::push_named_peer_row(&mut rows, "USB", peer);
        }
        Some(screen::CardKind::Ble) => {
            let _ = screen::push_snapshot_supervisor_peer_rows(&mut rows, selected_card, snapshots);
        }
        _ => {}
    }
    rows
}
