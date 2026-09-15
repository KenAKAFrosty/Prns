use core::fmt::Write as _;

use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::interfaces::lora::RadioProfile;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, Membership};
use personal_rns::remote_control::{
    wifi_station_inventory_config, RemoteControlBuildVersion, RemoteControlInterfaceCard,
    RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceEntry,
    RemoteControlInterfaceInventory, RemoteControlInterfaceInventoryError,
    RemoteControlInterfacePeer, RemoteControlInterfacePeerPage, RemoteControlInterfacePeersOutcome,
    REMOTE_CONTROL_INTERFACE_PEER_CAP,
};
use prns_core::engine::MAX_RESPOND_DATA_LEN;

/// Matches `RemoteControlResponse` header (version + kind).
const INVENTORY_RESPONSE_HEADER_LEN: usize = 2;

#[must_use]
pub fn hopspot_remote_control_build_version() -> RemoteControlBuildVersion {
    RemoteControlBuildVersion::from_label(
        crate::node_pages::BUILD_VERSION,
        crate::node_pages::BUILD_COMMIT,
    )
}

/// Supervisor list only. Name, group, LoRa tune, Auto Wi-Fi SSID, and
/// failure come from `InventoryInterfaceConfig`. Peers come from
/// `InventoryInterfacePeers`.
pub fn remote_control_inventory_from_snapshots(
    snapshots: &[InterfaceSnapshot],
) -> RemoteControlInterfaceInventory {
    let mut inventory = RemoteControlInterfaceInventory::empty();
    for snapshot in snapshots {
        let Some(kind) = operator_kind(snapshot) else {
            continue;
        };
        let entry = RemoteControlInterfaceEntry {
            id: snapshot.id,
            kind,
            mode: snapshot.mode,
            connection: snapshot.connection,
            enabled: snapshot.connection != personal_rns::interfaces::ConnectionState::Disabled,
            tx_bytes: snapshot.tx_bytes,
            rx_bytes: snapshot.rx_bytes,
            links: snapshot.links.saturating_add(snapshot.transported_links),
            rate_bytes_per_sec: rate_bytes_per_sec(snapshot),
        };
        if inventory.push(entry).is_err() {
            break;
        }
        if inventory_response_len(&inventory) > MAX_RESPOND_DATA_LEN {
            let _ = inventory.pop();
            break;
        }
    }
    inventory
}

/// One supervisor card: name, group, LoRa tune, destinations, failure.
pub fn remote_control_interface_config_from_snapshots(
    snapshots: &[InterfaceSnapshot],
    id: InterfaceId,
    decorate: impl FnOnce(&InterfaceSnapshot, &mut RemoteControlInterfaceCard),
) -> RemoteControlInterfaceConfigOutcome {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.id == id && operator_interface(snapshot))
    else {
        return RemoteControlInterfaceConfigOutcome::UnknownInterface;
    };
    let mut card = RemoteControlInterfaceCard::empty();
    decorate(snapshot, &mut card);
    if let Some(failure) = snapshot.failure_reason {
        if card.failure.is_empty() {
            card.set_failure(failure);
        }
    }
    card.destinations = snapshot.destinations;
    card.transported_links = snapshot.transported_links;
    RemoteControlInterfaceConfigOutcome::Card(card)
}

/// One packed page of fleet members for a supervisor. `offset` is the
/// number of members to skip; `total` is how many the host currently has.
pub fn remote_control_interface_peers_from_snapshots(
    snapshots: &[InterfaceSnapshot],
    id: InterfaceId,
    offset: u8,
) -> RemoteControlInterfacePeersOutcome {
    let Some(supervisor) = snapshots
        .iter()
        .find(|snapshot| snapshot.id == id && operator_interface(snapshot))
    else {
        return RemoteControlInterfacePeersOutcome::UnknownInterface;
    };
    let mut total = 0u8;
    let mut page = RemoteControlInterfacePeerPage::empty(supervisor.id, offset, 0);
    for snapshot in snapshots {
        let Some(peer) = remote_control_peer_for_supervisor(id, snapshot) else {
            continue;
        };
        if total >= offset && page.peers.len() < REMOTE_CONTROL_INTERFACE_PEER_CAP {
            match page.push(peer) {
                Ok(()) => {}
                Err(RemoteControlInterfaceInventoryError::Full) => {}
            }
        }
        total = total.saturating_add(1);
    }
    page.total = total;
    RemoteControlInterfacePeersOutcome::Page(page)
}

fn inventory_response_len(inventory: &RemoteControlInterfaceInventory) -> usize {
    INVENTORY_RESPONSE_HEADER_LEN.saturating_add(inventory.encoded_body_len())
}

/// Shared Hopspot name, LoRa tune, Auto Wi-Fi SSID, and BLE group labels for one config card.
pub fn decorate_hopspot_remote_control_card(
    snapshot: &InterfaceSnapshot,
    card: &mut RemoteControlInterfaceCard,
    ble_group: Option<&str>,
    lora_profile: Option<RadioProfile>,
    wifi_ssid: Option<&str>,
    ble_identity: Option<BleIdentity>,
) {
    let Some(kind) = operator_kind(snapshot) else {
        return;
    };
    if kind == InterfaceKind::BluetoothAuto {
        if let Some(identity) = ble_identity {
            card.set_name(bluetooth_auto_interface_name(identity).as_str());
        } else {
            card.set_name(kind.name());
        }
    } else {
        card.set_name(kind.name());
    }
    if matches!(kind, InterfaceKind::LoRa | InterfaceKind::Rnode) {
        if let Some(profile) = lora_profile {
            card.set_config(profile.inventory_config().as_str());
        } else {
            card.set_config("LoRa");
        }
    }
    if kind == InterfaceKind::AutoWifi {
        card.set_config(wifi_station_inventory_config(wifi_ssid.unwrap_or("")).as_str());
    }
    if kind == InterfaceKind::BluetoothAuto {
        if let Some(group) = ble_group.filter(|group| !group.is_empty()) {
            card.set_group(group);
        }
    }
}

/// Same `bluetooth-auto XXXX` title the Controller uses on Settings.
#[must_use]
pub fn bluetooth_auto_interface_name(identity: BleIdentity) -> heapless::String<32> {
    let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, identity.as_bytes());
    let bytes = id.as_bytes();
    let mut name = heapless::String::new();
    match (bytes.get(1), bytes.get(2)) {
        (Some(first), Some(second)) => {
            let _ = write!(&mut name, "bluetooth-auto {first:02x}{second:02x}");
        }
        (Some(_) | None, Some(_) | None) => {
            let _ = name.push_str("bluetooth-auto");
        }
    }
    name
}

fn operator_interface(snapshot: &InterfaceSnapshot) -> bool {
    operator_kind(snapshot).is_some()
}

/// Hopspot USB device ids are log-legible cookies (`heltecr8`, `techousb`),
/// not `kind ++ hash`. Inventory still reports them as `UsbAutoDevice`.
///
/// Any independent supervisor whose first id byte is not a known
/// `InterfaceKind` gets that same stamp. Fleet members stay omitted; a
/// decodable member kind is dropped, not relabeled USB.
fn operator_kind(snapshot: &InterfaceSnapshot) -> Option<InterfaceKind> {
    if matches!(snapshot.membership, Membership::FleetMember { .. }) {
        return None;
    }
    match snapshot.id.kind() {
        Some(kind) if kind.supervisor_kind().is_none() => Some(kind),
        None => Some(InterfaceKind::UsbAutoDevice),
        Some(_) => None,
    }
}

fn remote_control_peer_for_supervisor(
    supervisor_id: personal_rns::interfaces::InterfaceId,
    snapshot: &InterfaceSnapshot,
) -> Option<RemoteControlInterfacePeer> {
    match snapshot.membership {
        Membership::FleetMember {
            supervisor_id: owner,
        } if owner == supervisor_id => Some(RemoteControlInterfacePeer {
            id: snapshot.id,
            connection: snapshot.connection,
            tx_bytes: snapshot.tx_bytes,
            rx_bytes: snapshot.rx_bytes,
            links: snapshot.links,
            destinations: snapshot.destinations,
            rate_bytes_per_sec: rate_bytes_per_sec(snapshot),
            radio: snapshot.radio,
            details: snapshot.details,
        }),
        Membership::Independent | Membership::FleetMember { .. } => None,
    }
}

fn rate_bytes_per_sec(snapshot: &InterfaceSnapshot) -> u32 {
    snapshot
        .transfer_rates
        .map(|rates| rates.rx_bps.saturating_add(rates.tx_bps) / 8)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{
        ConnectionState, InterfaceGravity, InterfaceId, InterfaceKind, InterfaceMode, PeerDetails,
        RadioIndication, TransferRates,
    };
    use personal_rns::remote_control::RemoteControlResponse;

    fn snapshot(kind: InterfaceKind) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id: InterfaceId::new([kind as u8, 0, 0, 0, 0, 0, 0, 0]),
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 0,
            tx_bytes: 0,
            transfer_rates: None::<TransferRates>,
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: Membership::Independent,
            radio: RadioIndication::for_kind(Some(kind)),
            details: PeerDetails::NotApplicable,
        }
    }

    #[test]
    fn inventory_keeps_supervisors_and_pages_members_separately() {
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
        supervisor.links = 1;
        supervisor.transported_links = 2;
        supervisor.destinations = 3;
        let mut member = snapshot(InterfaceKind::BluetoothPeer);
        member.id = member_id;
        member.tx_bytes = 4;
        member.rx_bytes = 5;
        member.links = 6;
        member.destinations = 7;
        member.membership = Membership::FleetMember { supervisor_id };

        let snapshots = [supervisor, member];
        let inventory = remote_control_inventory_from_snapshots(&snapshots);

        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].links, 3);
        assert!(inventory.card(0).is_none());
        let RemoteControlInterfaceConfigOutcome::Card(card) =
            remote_control_interface_config_from_snapshots(
                &snapshots,
                supervisor_id,
                |snapshot, card| {
                    decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        Some("lab"),
                        None,
                        None,
                        Some(BleIdentity::new(*b"stable-identity!")),
                    )
                },
            )
        else {
            panic!("BLE supervisor should return a config card");
        };
        assert_eq!(
            card.name.as_str(),
            bluetooth_auto_interface_name(BleIdentity::new(*b"stable-identity!")).as_str()
        );
        assert_eq!(card.group.as_str(), "lab");
        assert!(card.config.is_empty());
        assert_eq!(card.destinations, 3);
        assert_eq!(card.transported_links, 2);
        assert!(card.peers.is_empty());

        let RemoteControlInterfacePeersOutcome::Page(page) =
            remote_control_interface_peers_from_snapshots(&snapshots, supervisor_id, 0)
        else {
            panic!("BLE supervisor should page its members");
        };
        assert_eq!(page.total, 1);
        assert_eq!(page.offset, 0);
        assert_eq!(page.peers.len(), 1);
        assert_eq!(page.peers[0].id, member_id);
        assert_eq!(page.peers[0].tx_bytes, 4);
        assert_eq!(page.peers[0].rx_bytes, 5);
        assert_eq!(page.peers[0].links, 6);
        assert_eq!(page.peers[0].destinations, 7);
    }

    #[test]
    fn auto_wifi_config_publishes_ssid_and_never_a_password() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let mut supervisor = snapshot(InterfaceKind::AutoWifi);
        supervisor.id = supervisor_id;
        let RemoteControlInterfaceConfigOutcome::Card(card) =
            remote_control_interface_config_from_snapshots(
                &[supervisor],
                supervisor_id,
                |snapshot, card| {
                    decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        None,
                        None,
                        Some("field-lab"),
                        None,
                    )
                },
            )
        else {
            panic!("Auto Wi-Fi supervisor should return a config card");
        };
        assert_eq!(card.config.as_str(), "W,field-lab");
        assert!(!card.config.as_str().contains("secret"));
    }

    #[test]
    fn peer_pages_cover_a_wifi_fleet_and_omit_members_as_interfaces() {
        let supervisor_id = InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
        let mut supervisor = snapshot(InterfaceKind::AutoWifi);
        supervisor.id = supervisor_id;
        let mut snapshots = heapless::Vec::<InterfaceSnapshot, 13>::new();
        assert!(snapshots.push(supervisor).is_ok());
        for suffix in 1..=12 {
            let mut member = snapshot(InterfaceKind::WifiPeer);
            member.id = InterfaceId::new([InterfaceKind::WifiPeer as u8, suffix, 0, 0, 0, 0, 0, 0]);
            member.membership = Membership::FleetMember { supervisor_id };
            assert!(snapshots.push(member).is_ok());
        }

        let inventory = remote_control_inventory_from_snapshots(&snapshots);

        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].kind, InterfaceKind::AutoWifi);
        assert!(inventory.card(0).is_none());
        assert!(
            RemoteControlResponse::InventoryInterfaces(inventory).encoded_len()
                <= MAX_RESPOND_DATA_LEN
        );

        let RemoteControlInterfacePeersOutcome::Page(first) =
            remote_control_interface_peers_from_snapshots(&snapshots, supervisor_id, 0)
        else {
            panic!("Wi-Fi supervisor should page its members");
        };
        assert_eq!(first.total, 12);
        assert_eq!(first.peers.len(), REMOTE_CONTROL_INTERFACE_PEER_CAP);
        let RemoteControlInterfacePeersOutcome::Page(second) =
            remote_control_interface_peers_from_snapshots(&snapshots, supervisor_id, 8)
        else {
            panic!("Wi-Fi supervisor should page remaining members");
        };
        assert_eq!(second.total, 12);
        assert_eq!(second.offset, 8);
        assert_eq!(second.peers.len(), 4);
        assert_eq!(
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                InterfaceId::new([0xff; 8]),
                0
            ),
            RemoteControlInterfacePeersOutcome::UnknownInterface
        );
    }

    #[test]
    fn hand_rolled_usb_ids_leave_as_usb_auto_device() {
        let id = InterfaceId::new(*b"heltecr8");
        let mut cookie = snapshot(InterfaceKind::Loopback);
        cookie.id = id;
        let inventory = remote_control_inventory_from_snapshots(&[cookie]);
        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].id, id);
        assert_eq!(inventory.entries()[0].kind, InterfaceKind::UsbAutoDevice);
        assert!(inventory.card(0).is_none());
    }

    #[test]
    fn hopspot_supervisor_set_keeps_the_link_packet_budget_when_peers_exist() {
        let ble_id = InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
        let kinds = [
            InterfaceKind::LoRa,
            InterfaceKind::BluetoothAuto,
            InterfaceKind::AutoWifi,
            InterfaceKind::EspNow,
            InterfaceKind::UsbAutoDevice,
            InterfaceKind::TcpClient,
        ];
        let mut snapshots = heapless::Vec::<InterfaceSnapshot, 14>::new();
        for kind in kinds {
            let mut item = snapshot(kind);
            if kind == InterfaceKind::BluetoothAuto {
                item.id = ble_id;
            }
            assert!(snapshots.push(item).is_ok());
        }
        for suffix in 1..=8 {
            let mut member = snapshot(InterfaceKind::BluetoothPeer);
            member.id =
                InterfaceId::new([InterfaceKind::BluetoothPeer as u8, suffix, 0, 0, 0, 0, 0, 0]);
            member.membership = Membership::FleetMember {
                supervisor_id: ble_id,
            };
            assert!(snapshots.push(member).is_ok());
        }

        let inventory = remote_control_inventory_from_snapshots(&snapshots);

        assert_eq!(
            inventory.entries().len(),
            kinds.len(),
            "encoded {} of {MAX_RESPOND_DATA_LEN}",
            RemoteControlResponse::InventoryInterfaces(inventory.clone()).encoded_len()
        );
        let lora_id = inventory
            .entries()
            .iter()
            .find(|entry| entry.kind == InterfaceKind::LoRa)
            .expect("LoRa supervisor")
            .id;
        let RemoteControlInterfaceConfigOutcome::Card(lora) =
            remote_control_interface_config_from_snapshots(
                &snapshots,
                lora_id,
                |snapshot, card| {
                    decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        Some("lab"),
                        Some(personal_rns::interfaces::lora::DEFAULT_915_PROFILE),
                        None,
                        None,
                    );
                },
            )
        else {
            panic!("LoRa supervisor should return a config card");
        };
        assert_eq!(
            lora.config.as_str(),
            personal_rns::interfaces::lora::DEFAULT_915_PROFILE
                .inventory_config()
                .as_str()
        );
        assert_eq!(
            inventory
                .entries()
                .iter()
                .filter(|entry| entry.kind.supervisor_kind().is_some())
                .count(),
            0,
        );
        assert!(
            RemoteControlResponse::InventoryInterfaces(inventory).encoded_len()
                <= MAX_RESPOND_DATA_LEN
        );
    }
}
