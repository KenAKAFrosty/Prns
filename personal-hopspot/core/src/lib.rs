#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "host")]
extern crate std;

mod battery;
mod destinations;
mod flash_identity;
mod identity;
mod mobile;
pub mod node_pages;
mod screen;

pub use battery::{BatteryGauge, BatteryPercent, BatterySource, BatteryState, NoBattery};
pub use destinations::{HopspotDestinationHashes, HopspotDestinationSet};
pub use flash_identity::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
};
#[cfg(feature = "host")]
pub use identity::{
    generate_host_ble_identity, generate_host_node_identity, load_host_ble_identity,
    load_host_node_identity,
};
pub use identity::{
    HopspotNodeIdentity, IdentityBootstrap, IdentityPersistence, IdentityStorageName,
    BLE_IDENTITY_STORAGE, NODE_IDENTITY_STORAGE,
};
pub use mobile::{
    InvalidMobileInputCode, MobileActionCode, MobileEngineFailure, MobileEngineState,
    MobileInputCode, MobileRgbaFrameBuffer, MOBILE_DARK_RGBA, MOBILE_LIT_RGBA, MOBILE_PANEL_HEIGHT,
    MOBILE_PANEL_WIDTH, MOBILE_PIXEL_COUNT, MOBILE_RGBA_BYTES,
};
pub use screen::{
    card_label, render, splash, tcp_card_label, AccessPointState, Card, CardActivityTracker,
    CardKind, CardLabel, DisplayPowerControl, InputEvent, InterfaceMenuDetails, Liveness,
    LocalDocsAccess, RenderFrame, ScreenContent, SplashContent, UiAction, UiConfiguration,
    UiNotice, UiState, WifiNetworkStatus,
};

use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceSnapshot, Membership};

/// The faces' redraw-coalescing window, in milliseconds. A burst of engine changes inside this span
/// folds into one repaint (~30 fps). It bounds how fast a face repaints when things change; it is not
/// a frame clock — a face wakes on the store's signal and stays idle when nothing moves.
pub const COALESCE_MS: u64 = 33;

fn liveness(connection: ConnectionState) -> Liveness {
    screen::liveness_from_connection(connection)
}

fn interface_kind_shows_supervisor_peers(id: InterfaceId) -> bool {
    id.kind().is_some_and(|kind| kind.member_kind().is_some())
}

/// Build the renderable [`Card`] list from one [`InterfaceSnapshot`] per interface. `classify`
/// maps an [`InterfaceId`] to its `(icon kind, label)`; returning `None` drops that interface.
/// `N` bounds the returned vector; pass the panel's card capacity.
///
/// A [`FleetMember`](Membership::FleetMember) gets no card of its own: its engine counts roll up
/// into its supervisor's card, so the root shows one card per independent interface with the
/// whole fleet's traffic summed under it. The link glyph sums terminated + carried links into one
/// count of every live link. The returned list is already in face display order.
pub fn snapshots_to_cards<const N: usize>(
    snapshots: &[InterfaceSnapshot],
    mut classify: impl FnMut(InterfaceId) -> Option<(CardKind, CardLabel)>,
) -> heapless::Vec<Card, N> {
    let mut cards = heapless::Vec::new();
    for snapshot in snapshots {
        if let Membership::FleetMember { .. } = snapshot.membership {
            continue;
        }
        let Some((kind, label)) = classify(snapshot.id) else {
            continue;
        };
        let mut destinations = snapshot.destinations;
        let mut links = snapshot.links;
        let mut transported_links = snapshot.transported_links;
        let mut has_members = false;
        for member in snapshots {
            if let Membership::FleetMember { supervisor_id } = member.membership {
                if supervisor_id == snapshot.id {
                    has_members = true;
                    destinations = destinations.saturating_add(member.destinations);
                    links = links.saturating_add(member.links);
                    transported_links = transported_links.saturating_add(member.transported_links);
                }
            }
        }
        let mut liveness = liveness(snapshot.connection);
        if has_members && liveness == Liveness::Dormant {
            liveness = Liveness::Live;
        }
        let _ = cards.push(Card {
            id: snapshot.id,
            kind,
            label,
            liveness,
            failure_reason: snapshot.failure_reason,
            tx_bytes: snapshot.tx_bytes,
            rx_bytes: snapshot.rx_bytes,
            links: links.saturating_add(transported_links),
            destinations,
            rate_bytes_per_sec: snapshot
                .transfer_rates
                .map(|rates| rates.rx_bps.saturating_add(rates.tx_bps) / 8)
                .unwrap_or(0),
            last_activity_secs: None,
        });
    }
    screen::sort_cards_for_display(&mut cards);
    cards
}

fn push_snapshot_supervisor_peer_rows(
    details: &mut InterfaceMenuDetails,
    selected_card: Option<&Card>,
    snapshots: &[InterfaceSnapshot],
) -> usize {
    let Some(card) = selected_card else {
        return 0;
    };
    let has_members = snapshots.iter().any(|snapshot| {
        matches!(
            snapshot.membership,
            Membership::FleetMember { supervisor_id } if supervisor_id == card.id
        )
    });
    if !has_members && !interface_kind_shows_supervisor_peers(card.id) {
        return 0;
    }
    let peers = snapshots.iter().filter_map(|snapshot| {
        if let Membership::FleetMember { supervisor_id } = snapshot.membership {
            (supervisor_id == card.id).then_some((snapshot.id, liveness(snapshot.connection)))
        } else {
            None
        }
    });
    details.push_supervisor_peers(peers)
}

pub fn snapshots_to_interface_menu_details(
    selected_card: Option<&Card>,
    snapshots: &[InterfaceSnapshot],
) -> InterfaceMenuDetails {
    let mut details = InterfaceMenuDetails::empty();
    let _ = push_snapshot_supervisor_peer_rows(&mut details, selected_card, snapshots);
    details
}

pub fn wifi_interface_menu_details(
    status: WifiNetworkStatus<'_>,
    selected_card: Option<&Card>,
    snapshots: &[InterfaceSnapshot],
) -> InterfaceMenuDetails {
    let mut details = InterfaceMenuDetails::empty();
    details.push_info("STA", status.station_ssid.unwrap_or("None"));
    details.push_info("AP", status.access_point_ssid.unwrap_or("None"));
    let _ = push_snapshot_supervisor_peer_rows(&mut details, selected_card, snapshots);
    details
}

pub fn usb_interface_menu_details(connection: ConnectionState) -> InterfaceMenuDetails {
    let mut details = InterfaceMenuDetails::empty();
    let liveness = liveness(connection);
    let peer = (liveness == Liveness::Live).then_some(liveness);
    let _ = details.push_named_peer("USB", peer);
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{InterfaceKind, TransferRates};

    fn snapshot(kind: InterfaceKind) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id: InterfaceId::new([kind as u8, 0, 0, 0, 0, 0, 0, 0]),
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 0,
            tx_bytes: 0,
            transfer_rates: None::<TransferRates>,
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: Membership::Independent,
        }
    }

    #[test]
    fn snapshots_to_cards_returns_face_display_order() {
        let snapshots = [
            snapshot(InterfaceKind::LoRa),
            snapshot(InterfaceKind::UsbAutoDevice),
            snapshot(InterfaceKind::BluetoothAuto),
            snapshot(InterfaceKind::AutoWifi),
        ];

        let cards: heapless::Vec<Card, 4> = snapshots_to_cards(&snapshots, |id| match id.kind() {
            Some(InterfaceKind::LoRa) => Some((CardKind::LoRa, card_label("LoRa"))),
            Some(InterfaceKind::UsbAutoDevice) => Some((CardKind::Usb, card_label("USB"))),
            Some(InterfaceKind::BluetoothAuto) => Some((CardKind::Ble, card_label("BLE"))),
            Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("Wi-Fi/LAN"))),
            _ => None,
        });

        let kinds: heapless::Vec<CardKind, 4> = cards.iter().map(|card| card.kind).collect();
        assert_eq!(
            kinds.as_slice(),
            &[CardKind::LoRa, CardKind::Wifi, CardKind::Ble, CardKind::Usb]
        );
    }

    #[test]
    fn snapshots_to_details_lists_selected_supervisor_members() {
        let supervisor_id =
            InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
        let member_id = InterfaceId::new([
            InterfaceKind::BluetoothPeer as u8,
            0xab,
            0xcd,
            0,
            0,
            0,
            0,
            0,
        ]);
        let mut supervisor = snapshot(InterfaceKind::BluetoothAuto);
        supervisor.id = supervisor_id;
        let mut member = snapshot(InterfaceKind::BluetoothPeer);
        member.id = member_id;
        member.membership = Membership::FleetMember { supervisor_id };
        let card = Card {
            id: supervisor_id,
            kind: CardKind::Ble,
            label: card_label("BLE"),
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        };

        let details = snapshots_to_interface_menu_details(Some(&card), &[supervisor, member]);
        let rows = details.as_slice();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text(), "Peers 1");
        assert_eq!(rows[1].text(), "P abcd Live");
    }

    #[test]
    fn snapshots_to_cards_marks_supervisor_live_when_any_member_exists() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let member_id =
            InterfaceId::new([InterfaceKind::WifiPeer as u8, 0x12, 0x34, 0, 0, 0, 0, 0]);
        let mut supervisor = snapshot(InterfaceKind::AutoWifi);
        supervisor.id = supervisor_id;
        supervisor.connection = ConnectionState::Disconnected;
        let mut member = snapshot(InterfaceKind::WifiPeer);
        member.id = member_id;
        member.connection = ConnectionState::Disconnected;
        member.membership = Membership::FleetMember { supervisor_id };

        let cards: heapless::Vec<Card, 4> = snapshots_to_cards(&[supervisor, member], |id| {
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("Wi-Fi/LAN")))
        });

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].liveness, Liveness::Live);
    }

    #[test]
    fn snapshots_to_details_keeps_zero_peer_row_for_idle_supervisor() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let card = Card {
            id: supervisor_id,
            kind: CardKind::Wifi,
            label: card_label("Wi-Fi/LAN"),
            liveness: Liveness::Dormant,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        };

        let details = snapshots_to_interface_menu_details(Some(&card), &[]);
        let rows = details.as_slice();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(), "Peers 0");
    }

    #[test]
    fn wifi_details_render_absent_networks_and_supervisor_peers() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let member_id =
            InterfaceId::new([InterfaceKind::WifiPeer as u8, 0x12, 0x34, 0, 0, 0, 0, 0]);
        let mut supervisor = snapshot(InterfaceKind::AutoWifi);
        supervisor.id = supervisor_id;
        let mut member = snapshot(InterfaceKind::WifiPeer);
        member.id = member_id;
        member.membership = Membership::FleetMember { supervisor_id };
        let cards: heapless::Vec<Card, 1> = snapshots_to_cards(&[supervisor, member], |id| {
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("Wi-Fi/LAN")))
        });

        let details = wifi_interface_menu_details(
            WifiNetworkStatus {
                station_ssid: None,
                access_point_ssid: Some("Hopspot-EW53"),
            },
            cards.first(),
            &[supervisor, member],
        );
        let rows = details.as_slice();

        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].text(), "STA None");
        assert_eq!(rows[1].text(), "AP Hopspot-EW53");
        assert_eq!(rows[2].text(), "Peers 1");
        assert_eq!(rows[3].text(), "P 1234 Live");
    }

    #[test]
    fn usb_details_distinguish_connected_and_absent_peers() {
        let connected = usb_interface_menu_details(ConnectionState::Connected);
        let disconnected = usb_interface_menu_details(ConnectionState::Disconnected);

        assert_eq!(connected.as_slice().len(), 2);
        assert_eq!(connected.as_slice()[0].text(), "Peers 1");
        assert_eq!(connected.as_slice()[1].text(), "P USB Live");
        assert_eq!(disconnected.as_slice().len(), 1);
        assert_eq!(disconnected.as_slice()[0].text(), "Peers 0");
    }
}
