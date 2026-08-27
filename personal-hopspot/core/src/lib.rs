#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "host")]
extern crate std;
#[cfg(all(test, not(feature = "host")))]
extern crate std;

mod destinations;
mod flash_identity;
mod flash_layout;
mod identity;
mod mobile;
pub mod node_pages;
mod persistence;
mod radio_profile_store;
mod screen;
mod soft_ap;

pub use destinations::{
    hopspot_destination_hashes, HopspotDestinationHashes, HopspotDestinationSet,
};
pub use flash_identity::{
    bootstrap_flash_ble_identity, bootstrap_flash_node_identity, FlashIdentityError,
};
pub use flash_layout::{
    FirmwareAddressRange, HopspotS3FlashLayout, Nrf52840FirmwareMemory,
    HELTEC_DISPLAY_NRF52840_FIRMWARE_MEMORY, HELTEC_DISPLAY_NRF52840_JOURNAL_LAYOUT,
    HOPSPOT_FLASH_PAGE_BYTES, MESH_TOWER_V2_BLE_IDENTITY_FLASH_OFFSET,
    MESH_TOWER_V2_FIRMWARE_MEMORY, MESH_TOWER_V2_JOURNAL_LAYOUT,
    MESH_TOWER_V2_RADIO_PROFILE_FLASH_OFFSET, MESH_TOWER_V2_RECOVERY_BOOTLOADER_FLASH_OFFSET,
    NRF52840_BLE_IDENTITY_FLASH_OFFSET, NRF52840_MIN_ARENA_BYTES,
    NRF52840_NODE_IDENTITY_FLASH_OFFSET, NRF52840_RADIO_PROFILE_PAGES, S3_16_MIB_FLASH_LAYOUT,
    S3_8_MIB_FLASH_LAYOUT, T096_APPLICATION_DATA_END, T096_FACTORY_RESERVED_FLASH_OFFSET,
    T096_RECOVERY_BOOTLOADER_FLASH_OFFSET, T1000E_FIRMWARE_MEMORY, T1000E_JOURNAL_LAYOUT,
    T1000E_NODE_IDENTITY_FLASH_OFFSET, T1000E_RECOVERY_BOOTLOADER_FLASH_OFFSET,
    T114_RECOVERY_BOOTLOADER_FLASH_OFFSET, T_ECHO_BLE_IDENTITY_FLASH_OFFSET, T_ECHO_JOURNAL_LAYOUT,
    T_ECHO_MIN_ARENA_BYTES, T_ECHO_RESERVED_FLASH_END, T_ECHO_S140_V6_FIRMWARE_MEMORY,
    T_ECHO_S140_V7_FIRMWARE_MEMORY,
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
pub use persistence::PersistenceState;
pub use prns_core::capabilities::positioning::gnss::{
    GnssFix, GnssReceiverCommand, GnssSnapshot, NmeaParser,
};
pub use prns_core::capabilities::positioning::{
    AltitudeMillimeters, CoordinateOutOfRange, GeographicPosition, LatitudeE7, LongitudeE7,
};
pub use prns_core::capabilities::power::{
    BatteryGauge, BatteryPercent, BatterySource, ChargingState, ExternalPowerState, NoBattery,
    PowerSnapshot,
};
pub use radio_profile_store::{
    LoadedRadioProfile, RadioProfileLoadNotice, RadioProfileStore, RadioProfileStoreError,
};
pub use screen::{
    apply_and_persist_radio_profile, card_label, card_label_max_chars, render, splash,
    tcp_card_label, AccessPointState, BluetoothRecoveryMenuDetails, CanvasDimensions, Card,
    CardActivityTracker, CardKind, CardLabel, DisplayAutoOff, DisplayAutoOffDuration,
    DisplayButtonOutcome, DisplayDarkReason, DisplayPowerCommand, DisplayPowerControl,
    DisplayPowerState, EinkRefresh, EinkRefreshPolicy, EinkRefreshUrgency, GnssAvailability,
    InputEvent, InterfaceMenuDetails, LoRaSpectrumMenuDetails, LocalDocsAccess, LogicalPoint,
    PersistenceNotice, QuarterTurn, RadioProfileChangeResult, RenderFrame, RotatedCanvasMapping,
    ScreenContent, SharedInstanceConfigExport, SplashContent, UiAction, UiConfiguration, UiNotice,
    UiState, WifiNetworkStatus, WifiStationStatus, DEFAULT_DISPLAY_AUTO_OFF,
};
pub use soft_ap::SoftApLeaseTable;

use personal_rns::engine::{
    EngineProtocolPolicy, LinkMtuDiscovery, LocalHopCountOverride, ProofForm,
    RecursivePathRequestDefault,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceSnapshot, Membership};

pub const EMBEDDED_HOPSPOT_PROTOCOL_POLICY: EngineProtocolPolicy = EngineProtocolPolicy {
    proof_form: ProofForm::Implicit,
    link_mtu_discovery: LinkMtuDiscovery::Enabled,
    local_hop_count_override: LocalHopCountOverride::Disabled,
    recursive_path_request_default: RecursivePathRequestDefault::Enabled,
};

/// The faces' redraw-coalescing window, in milliseconds. A burst of engine changes inside this span
/// folds into one repaint (~30 fps). It bounds how fast a face repaints when things change; it is not
/// a frame clock — a face wakes on the store's signal and stays idle when nothing moves.
pub const COALESCE_MS: u64 = 33;

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
        let mut peers = 0u32;
        let mut tx_bytes = snapshot.tx_bytes;
        let mut rx_bytes = snapshot.rx_bytes;
        let mut member_rate_rx_bps = 0u32;
        let mut member_rate_tx_bps = 0u32;
        let mut has_member_rates = false;
        for member in snapshots {
            if let Membership::FleetMember { supervisor_id } = member.membership {
                if supervisor_id == snapshot.id {
                    peers = peers.saturating_add(1);
                    destinations = destinations.saturating_add(member.destinations);
                    links = links.saturating_add(member.links);
                    transported_links = transported_links.saturating_add(member.transported_links);
                    tx_bytes = tx_bytes.saturating_add(member.tx_bytes);
                    rx_bytes = rx_bytes.saturating_add(member.rx_bytes);
                    if let Some(rates) = member.transfer_rates {
                        member_rate_rx_bps = member_rate_rx_bps.saturating_add(rates.rx_bps);
                        member_rate_tx_bps = member_rate_tx_bps.saturating_add(rates.tx_bps);
                        has_member_rates = true;
                    }
                }
            }
        }
        let rate_bytes_per_sec = if peers > 0 {
            has_member_rates.then_some(
                member_rate_rx_bps
                    .saturating_add(member_rate_tx_bps)
                    .saturating_div(8),
            )
        } else {
            snapshot
                .transfer_rates
                .map(|rates| rates.rx_bps.saturating_add(rates.tx_bps) / 8)
        }
        .unwrap_or(0);
        let _ = cards.push(Card {
            id: snapshot.id,
            kind,
            label,
            connection: snapshot.connection,
            failure_reason: snapshot.failure_reason,
            tx_bytes,
            rx_bytes,
            links: links.saturating_add(transported_links),
            peers: interface_kind_shows_supervisor_peers(snapshot.id).then_some(peers),
            destinations,
            rate_bytes_per_sec,
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
            (supervisor_id == card.id).then_some((snapshot.id, snapshot.connection))
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
    let station_label = match status.station {
        WifiStationStatus::Unconfigured => "None",
        WifiStationStatus::Joining => "Joining",
        WifiStationStatus::Connected(ssid) => ssid,
        WifiStationStatus::Disabled => "Off",
    };
    details.push_info("STA", station_label);
    details.push_info("AP", status.access_point_ssid.unwrap_or("None"));
    let _ = push_snapshot_supervisor_peer_rows(&mut details, selected_card, snapshots);
    details
}

pub fn usb_interface_menu_details(connection: ConnectionState) -> InterfaceMenuDetails {
    let mut details = InterfaceMenuDetails::empty();
    let peer = matches!(
        connection,
        ConnectionState::Connected | ConnectionState::Degraded
    )
    .then_some(connection);
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
            mode: personal_rns::interfaces::InterfaceMode::Full,
            gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
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
            Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("LAN"))),
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
            connection: ConnectionState::Connected,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            peers: Some(1),
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
    fn snapshots_to_cards_preserves_supervisor_connection_when_members_exist() {
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
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("LAN")))
        });

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].connection, ConnectionState::Disconnected);
    }

    #[test]
    fn snapshots_to_cards_rolls_up_fleet_member_traffic_bytes() {
        let supervisor_id =
            InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
        let member_id = InterfaceId::new([
            InterfaceKind::BluetoothPeer as u8,
            0x78,
            0xa8,
            0,
            0,
            0,
            0,
            0,
        ]);
        let mut supervisor = snapshot(InterfaceKind::BluetoothAuto);
        supervisor.id = supervisor_id;
        supervisor.tx_bytes = 0;
        supervisor.rx_bytes = 0;
        let mut member = snapshot(InterfaceKind::BluetoothPeer);
        member.id = member_id;
        member.tx_bytes = 20;
        member.rx_bytes = 183;
        member.transfer_rates = Some(TransferRates {
            rx_bps: 800,
            tx_bps: 160,
        });
        member.membership = Membership::FleetMember { supervisor_id };

        let cards: heapless::Vec<Card, 4> = snapshots_to_cards(&[supervisor, member], |id| {
            (id == supervisor_id).then_some((CardKind::Ble, card_label("BLE")))
        });

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].tx_bytes, 20);
        assert_eq!(cards[0].rx_bytes, 183);
        assert_eq!(cards[0].rate_bytes_per_sec, 120);
    }

    #[test]
    fn snapshots_to_cards_rolls_up_more_members_than_the_card_capacity() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let mut supervisor = snapshot(InterfaceKind::AutoWifi);
        supervisor.id = supervisor_id;
        let mut snapshots: heapless::Vec<InterfaceSnapshot, 13> = heapless::Vec::new();
        assert!(snapshots.push(supervisor).is_ok());
        for suffix in 1..=12 {
            let mut member = snapshot(InterfaceKind::WifiPeer);
            member.id = InterfaceId::new([InterfaceKind::WifiPeer as u8, suffix, 0, 0, 0, 0, 0, 0]);
            member.destinations = 40;
            member.membership = Membership::FleetMember { supervisor_id };
            assert!(snapshots.push(member).is_ok());
        }

        let cards: heapless::Vec<Card, 1> = snapshots_to_cards(&snapshots, |id| {
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("LAN")))
        });

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].peers, Some(12));
        assert_eq!(cards[0].destinations, 480);
    }

    #[test]
    fn snapshots_to_details_keeps_zero_peer_row_for_idle_supervisor() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let card = Card {
            id: supervisor_id,
            kind: CardKind::Wifi,
            label: card_label("LAN"),
            connection: ConnectionState::Disconnected,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            peers: Some(0),
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
            (id == supervisor_id).then_some((CardKind::Wifi, card_label("LAN")))
        });

        let details = wifi_interface_menu_details(
            WifiNetworkStatus {
                station: WifiStationStatus::Unconfigured,
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
    fn wifi_details_distinguish_station_uplink_states() {
        let cases = [
            (WifiStationStatus::Unconfigured, "STA None"),
            (WifiStationStatus::Joining, "STA Joining"),
            (WifiStationStatus::Connected("DeskNet"), "STA DeskNet"),
            (WifiStationStatus::Disabled, "STA Off"),
        ];

        for (station, expected_row) in cases {
            let details = wifi_interface_menu_details(
                WifiNetworkStatus {
                    station,
                    access_point_ssid: None,
                },
                None,
                &[],
            );
            assert_eq!(details.as_slice()[0].text(), expected_row);
        }
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
