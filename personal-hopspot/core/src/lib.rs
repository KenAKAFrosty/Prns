#![no_std]
#![forbid(unsafe_code)]

pub mod battery;
pub mod screen;

pub use battery::{BatteryGauge, BatteryPercent, BatterySource, BatteryState, NoBattery};
pub use screen::{
    card_label, draw_with_state, draw_with_state_at, draw_with_state_footer_at,
    draw_with_state_footer_details_at, liveness_from_connection, push_interface_menu_info,
    push_named_peer_row, push_supervisor_peer_rows, splash, tcp_card_label, AccessPointState, Card,
    CardActivityTracker, CardKind, CardLabel, DisplayPowerControl, InputEvent,
    InterfaceMenuDetailKind, InterfaceMenuDetailRow, InterfaceMenuDetailRows,
    InterfaceMenuDetailText, Liveness, SupervisorPeerMenuStatus, UiAction, UiConfiguration,
    UiFooter, UiNotice, UiState,
};

use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceSnapshot, Membership};

/// The faces' redraw-coalescing window, in milliseconds. A burst of engine changes inside this span
/// folds into one repaint (~30 fps). It bounds how fast a face repaints when things change; it is not
/// a frame clock — a face wakes on the store's signal and stays idle when nothing moves.
pub const COALESCE_MS: u64 = 33;

fn liveness(connection: ConnectionState) -> Liveness {
    liveness_from_connection(connection)
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

/// Append peer detail rows for the selected supervisor card from the same snapshot set that built
/// the card stack. Non-supervisor cards leave `rows` unchanged; supervisor interface kinds get a
/// `Peers 0` row even when no members are currently connected.
pub fn push_snapshot_supervisor_peer_rows(
    rows: &mut InterfaceMenuDetailRows,
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
            (supervisor_id == card.id).then_some(SupervisorPeerMenuStatus {
                id: snapshot.id,
                liveness: liveness(snapshot.connection),
            })
        } else {
            None
        }
    });
    screen::push_supervisor_peer_rows(rows, peers)
}

/// Build the standard selected-interface detail rows from snapshots. Faces with no board-specific
/// details can pass this straight to the renderer; faces with extra rows can append via
/// [`push_snapshot_supervisor_peer_rows`].
pub fn snapshots_to_interface_menu_details(
    selected_card: Option<&Card>,
    snapshots: &[InterfaceSnapshot],
) -> InterfaceMenuDetailRows {
    let mut rows = InterfaceMenuDetailRows::new();
    let _ = push_snapshot_supervisor_peer_rows(&mut rows, selected_card, snapshots);
    rows
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
            Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("WiFi/LAN"))),
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

        let rows = snapshots_to_interface_menu_details(Some(&card), &[supervisor, member]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text(), "Peers 1");
        assert_eq!(rows[1].kind(), InterfaceMenuDetailKind::Peer);
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
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("WiFi/LAN")))
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
            label: card_label("WiFi/LAN"),
            liveness: Liveness::Dormant,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        };

        let rows = snapshots_to_interface_menu_details(Some(&card), &[]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(), "Peers 0");
    }
}
