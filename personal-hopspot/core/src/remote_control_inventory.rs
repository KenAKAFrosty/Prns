use core::fmt::Write as _;

use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::interfaces::lora::RadioProfile;
use personal_rns::interfaces::{
    DiscoveryGroupSet, InterfaceId, InterfaceKind, InterfaceSnapshot, Membership,
};
use personal_rns::remote_control::{
    wifi_station_inventory_config, RemoteControlBuildVersion, RemoteControlBuildVersionLabelError,
    RemoteControlInterfaceCard, RemoteControlInterfaceCardError,
    RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceContinuation,
    RemoteControlInterfaceCursor, RemoteControlInterfaceEntry, RemoteControlInterfaceInventory,
    RemoteControlInterfaceInventoryError, RemoteControlInterfacePage, RemoteControlInterfacePeer,
    RemoteControlInterfacePeerPage, RemoteControlInterfacePeersOutcome,
    RemoteControlPeerContinuation, RemoteControlPeerCursor, RemoteControlPeerPage,
    RemoteControlResponse, REMOTE_CONTROL_INTERFACE_INVENTORY_CAP,
    REMOTE_CONTROL_INTERFACE_PEER_CAP,
};
use prns_core::engine::MAX_RESPOND_DATA_LEN;

const _: () = assert!(RemoteControlResponse::MAX_ENCODED_LEN <= MAX_RESPOND_DATA_LEN);

pub fn hopspot_remote_control_build_version(
) -> Result<RemoteControlBuildVersion, RemoteControlBuildVersionLabelError> {
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
    page: RemoteControlInterfacePage,
) -> Result<RemoteControlInterfaceInventory, RemoteControlInterfaceInventoryError> {
    reject_duplicate_snapshot_ids(snapshots)?;
    let mut inventory = RemoteControlInterfaceInventory::empty();
    let mut after = match page {
        RemoteControlInterfacePage::First => None,
        RemoteControlInterfacePage::After(cursor) => Some(cursor.id()),
    };
    while inventory.entries().len() < REMOTE_CONTROL_INTERFACE_INVENTORY_CAP {
        let Some(snapshot) = snapshots
            .iter()
            .filter(|snapshot| operator_interface(snapshot))
            .filter(|snapshot| after.is_none_or(|after| snapshot.id.as_bytes() > after.as_bytes()))
            .min_by_key(|snapshot| *snapshot.id.as_bytes())
        else {
            break;
        };
        let Some(kind) = operator_kind(snapshot) else {
            break;
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
        inventory.push(entry)?;
        after = Some(snapshot.id);
    }
    if let Some(last) = inventory.entries().last() {
        let has_more = snapshots.iter().any(|snapshot| {
            operator_interface(snapshot) && snapshot.id.as_bytes() > last.id.as_bytes()
        });
        if has_more {
            inventory.set_continuation(RemoteControlInterfaceContinuation::More(
                RemoteControlInterfaceCursor::after(last.id),
            ))?;
        }
    }
    Ok(inventory)
}

/// One supervisor card: name, group, LoRa tune, destinations, failure.
pub fn remote_control_interface_config_from_snapshots(
    snapshots: &[InterfaceSnapshot],
    id: InterfaceId,
    decorate: impl FnOnce(
        &InterfaceSnapshot,
        &mut RemoteControlInterfaceCard,
    ) -> Result<(), RemoteControlInterfaceCardError>,
) -> Result<RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceCardError> {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.id == id && operator_interface(snapshot))
    else {
        return Ok(RemoteControlInterfaceConfigOutcome::UnknownInterface);
    };
    let mut card = RemoteControlInterfaceCard::empty();
    decorate(snapshot, &mut card)?;
    if let Some(failure) = snapshot.failure_reason {
        if card.failure.is_empty() {
            card.set_failure(failure)?;
        }
    }
    card.destinations = snapshot.destinations;
    card.transported_links = snapshot.transported_links;
    Ok(RemoteControlInterfaceConfigOutcome::Card(card))
}

/// One packed page of fleet members for a supervisor. `offset` is the
/// number of members to skip; `total` is how many the host currently has.
pub fn remote_control_interface_peers_from_snapshots(
    snapshots: &[InterfaceSnapshot],
    id: InterfaceId,
    requested_page: RemoteControlPeerPage,
) -> Result<RemoteControlInterfacePeersOutcome, RemoteControlInterfaceInventoryError> {
    reject_duplicate_snapshot_ids(snapshots)?;
    let Some(supervisor) = snapshots
        .iter()
        .find(|snapshot| snapshot.id == id && operator_interface(snapshot))
    else {
        return Ok(RemoteControlInterfacePeersOutcome::UnknownInterface);
    };
    let mut after = match requested_page {
        RemoteControlPeerPage::First => None,
        RemoteControlPeerPage::After(cursor) => Some(cursor.id()),
    };
    let mut page = RemoteControlInterfacePeerPage::empty(supervisor.id);
    while page.peers.len() < REMOTE_CONTROL_INTERFACE_PEER_CAP {
        let Some(peer) = snapshots
            .iter()
            .filter_map(|snapshot| remote_control_peer_for_supervisor(id, snapshot))
            .filter(|peer| after.is_none_or(|after| peer.id.as_bytes() > after.as_bytes()))
            .min_by_key(|peer| *peer.id.as_bytes())
        else {
            break;
        };
        after = Some(peer.id);
        page.push(peer)?;
    }
    if let Some(last) = page.peers.last() {
        let has_more = snapshots
            .iter()
            .filter_map(|snapshot| remote_control_peer_for_supervisor(id, snapshot))
            .any(|peer| peer.id.as_bytes() > last.id.as_bytes());
        if has_more {
            page.set_continuation(RemoteControlPeerContinuation::More(
                RemoteControlPeerCursor::after(last.id),
            ))?;
        }
    }
    Ok(RemoteControlInterfacePeersOutcome::Page(page))
}

/// Shared Hopspot name, LoRa tune, Auto Wi-Fi SSID, and compatibility group label for one card.
pub fn decorate_hopspot_remote_control_card(
    snapshot: &InterfaceSnapshot,
    card: &mut RemoteControlInterfaceCard,
    discovery_group: Option<&str>,
    lora_profile: Option<RadioProfile>,
    wifi_ssid: Option<&str>,
    ble_identity: Option<BleIdentity>,
) -> Result<(), RemoteControlInterfaceCardError> {
    let Some(kind) = operator_kind(snapshot) else {
        return Ok(());
    };
    if kind == InterfaceKind::BluetoothAuto {
        if let Some(identity) = ble_identity {
            card.set_name(bluetooth_auto_interface_name(identity)?.as_str())?;
        } else {
            card.set_name(kind.name())?;
        }
    } else {
        card.set_name(kind.name())?;
    }
    if matches!(kind, InterfaceKind::LoRa | InterfaceKind::Rnode) {
        if let Some(profile) = lora_profile {
            card.set_config(profile.inventory_config().as_str())?;
        } else {
            card.set_config("LoRa")?;
        }
    }
    if kind == InterfaceKind::AutoWifi {
        let config = wifi_station_inventory_config(wifi_ssid.unwrap_or(""))
            .map_err(|_| RemoteControlInterfaceCardError::ConfigTooLong)?;
        card.set_config(config.as_str())?;
    }
    if matches!(kind, InterfaceKind::BluetoothAuto | InterfaceKind::AutoWifi) {
        if let Some(group) = discovery_group.filter(|group| !group.is_empty()) {
            card.set_group(group)?;
        }
    }
    Ok(())
}

/// Compatibility inventory has room for one group only. A multi-group interface deliberately
/// leaves that legacy field empty; callers retrieve the complete set through the plural operation.
#[must_use]
pub fn singleton_discovery_group(groups: &DiscoveryGroupSet) -> Option<&str> {
    if groups.len() == 1 {
        groups.iter().next().map(|group| group.as_str())
    } else {
        None
    }
}

fn reject_duplicate_snapshot_ids(
    snapshots: &[InterfaceSnapshot],
) -> Result<(), RemoteControlInterfaceInventoryError> {
    for (index, snapshot) in snapshots.iter().enumerate() {
        if snapshots
            .iter()
            .skip(index.saturating_add(1))
            .any(|other| other.id == snapshot.id)
        {
            return Err(RemoteControlInterfaceInventoryError::NonAscending);
        }
    }
    Ok(())
}

/// Same `bluetooth-auto XXXX` title the Controller uses on Settings.
pub fn bluetooth_auto_interface_name(
    identity: BleIdentity,
) -> Result<heapless::String<32>, RemoteControlInterfaceCardError> {
    let id = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, identity.as_bytes());
    let bytes = id.as_bytes();
    let mut name = heapless::String::new();
    match (bytes.get(1), bytes.get(2)) {
        (Some(first), Some(second)) => {
            write!(&mut name, "bluetooth-auto {first:02x}{second:02x}")
                .map_err(|_| RemoteControlInterfaceCardError::NameTooLong)?;
        }
        (Some(_) | None, Some(_) | None) => {
            name.push_str("bluetooth-auto")
                .map_err(|_| RemoteControlInterfaceCardError::NameTooLong)?;
        }
    }
    Ok(name)
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
        let inventory =
            remote_control_inventory_from_snapshots(&snapshots, RemoteControlInterfacePage::First)
                .unwrap();

        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].links, 3);
        let Ok(RemoteControlInterfaceConfigOutcome::Card(card)) =
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
            bluetooth_auto_interface_name(BleIdentity::new(*b"stable-identity!"))
                .unwrap()
                .as_str()
        );
        assert_eq!(card.group.as_str(), "lab");
        assert!(card.config.is_empty());
        assert_eq!(card.destinations, 3);
        assert_eq!(card.transported_links, 2);

        let Ok(RemoteControlInterfacePeersOutcome::Page(page)) =
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                supervisor_id,
                RemoteControlPeerPage::First,
            )
        else {
            panic!("BLE supervisor should page its members");
        };
        assert_eq!(page.peers.len(), 1);
        assert_eq!(page.continuation(), RemoteControlPeerContinuation::Complete);
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
        let Ok(RemoteControlInterfaceConfigOutcome::Card(card)) =
            remote_control_interface_config_from_snapshots(
                &[supervisor],
                supervisor_id,
                |snapshot, card| {
                    decorate_hopspot_remote_control_card(
                        snapshot,
                        card,
                        Some("reticulum"),
                        None,
                        Some("field-lab"),
                        None,
                    )
                },
            )
        else {
            panic!("Auto Wi-Fi supervisor should return a config card");
        };
        assert_eq!(card.group.as_str(), "reticulum");
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

        let inventory =
            remote_control_inventory_from_snapshots(&snapshots, RemoteControlInterfacePage::First)
                .unwrap();

        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].kind, InterfaceKind::AutoWifi);
        assert!(
            RemoteControlResponse::InventoryInterfaces(inventory).encoded_len()
                <= MAX_RESPOND_DATA_LEN
        );

        let Ok(RemoteControlInterfacePeersOutcome::Page(first)) =
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                supervisor_id,
                RemoteControlPeerPage::First,
            )
        else {
            panic!("Wi-Fi supervisor should page its members");
        };
        assert_eq!(first.peers.len(), REMOTE_CONTROL_INTERFACE_PEER_CAP);
        let RemoteControlPeerContinuation::More(cursor) = first.continuation() else {
            panic!("first page should continue");
        };
        let Ok(RemoteControlInterfacePeersOutcome::Page(second)) =
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                supervisor_id,
                RemoteControlPeerPage::After(cursor),
            )
        else {
            panic!("Wi-Fi supervisor should page remaining members");
        };
        assert_eq!(second.peers.len(), REMOTE_CONTROL_INTERFACE_PEER_CAP);
        let RemoteControlPeerContinuation::More(cursor) = second.continuation() else {
            panic!("second page should continue");
        };
        let Ok(RemoteControlInterfacePeersOutcome::Page(third)) =
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                supervisor_id,
                RemoteControlPeerPage::After(cursor),
            )
        else {
            panic!("Wi-Fi supervisor should page final members");
        };
        assert_eq!(third.peers.len(), REMOTE_CONTROL_INTERFACE_PEER_CAP);
        assert_eq!(
            third.continuation(),
            RemoteControlPeerContinuation::Complete
        );
        assert_eq!(
            remote_control_interface_peers_from_snapshots(
                &snapshots,
                InterfaceId::new([0xff; 8]),
                RemoteControlPeerPage::First,
            ),
            Ok(RemoteControlInterfacePeersOutcome::UnknownInterface)
        );
    }

    #[test]
    fn hand_rolled_usb_ids_leave_as_usb_auto_device() {
        let id = InterfaceId::new(*b"heltecr8");
        let mut cookie = snapshot(InterfaceKind::Loopback);
        cookie.id = id;
        let inventory =
            remote_control_inventory_from_snapshots(&[cookie], RemoteControlInterfacePage::First)
                .unwrap();
        assert_eq!(inventory.entries().len(), 1);
        assert_eq!(inventory.entries()[0].id, id);
        assert_eq!(inventory.entries()[0].kind, InterfaceKind::UsbAutoDevice);
    }

    #[test]
    fn duplicate_snapshot_ids_are_rejected_instead_of_disappearing_between_pages() {
        let duplicate = snapshot(InterfaceKind::LoRa);
        assert_eq!(
            remote_control_inventory_from_snapshots(
                &[duplicate, duplicate],
                RemoteControlInterfacePage::First,
            ),
            Err(RemoteControlInterfaceInventoryError::NonAscending),
        );
    }

    #[test]
    fn overlong_card_values_are_rejected_instead_of_truncated() {
        let supervisor = snapshot(InterfaceKind::AutoWifi);
        let long_ssid = "s".repeat(personal_rns::remote_control::REMOTE_CONTROL_WIFI_SSID_CAP + 1);
        assert_eq!(
            remote_control_interface_config_from_snapshots(
                &[supervisor],
                supervisor.id,
                |snapshot, card| decorate_hopspot_remote_control_card(
                    snapshot,
                    card,
                    None,
                    None,
                    Some(long_ssid.as_str()),
                    None,
                ),
            ),
            Err(RemoteControlInterfaceCardError::ConfigTooLong),
        );
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

        let first =
            remote_control_inventory_from_snapshots(&snapshots, RemoteControlInterfacePage::First)
                .unwrap();
        let RemoteControlInterfaceContinuation::More(cursor) = first.continuation() else {
            panic!("first supervisor page should continue");
        };
        let second = remote_control_inventory_from_snapshots(
            &snapshots,
            RemoteControlInterfacePage::After(cursor),
        )
        .unwrap();
        assert_eq!(
            first.entries().len() + second.entries().len(),
            kinds.len(),
            "encoded first={} second={} of {MAX_RESPOND_DATA_LEN}",
            RemoteControlResponse::InventoryInterfaces(first.clone()).encoded_len(),
            RemoteControlResponse::InventoryInterfaces(second.clone()).encoded_len()
        );
        let lora_id = first
            .entries()
            .iter()
            .chain(second.entries())
            .find(|entry| entry.kind == InterfaceKind::LoRa)
            .expect("LoRa supervisor")
            .id;
        let Ok(RemoteControlInterfaceConfigOutcome::Card(lora)) =
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
                    )
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
            first
                .entries()
                .iter()
                .chain(second.entries())
                .filter(|entry| entry.kind.supervisor_kind().is_some())
                .count(),
            0,
        );
        assert!(
            RemoteControlResponse::InventoryInterfaces(first).encoded_len() <= MAX_RESPOND_DATA_LEN
        );
        assert!(
            RemoteControlResponse::InventoryInterfaces(second).encoded_len()
                <= MAX_RESPOND_DATA_LEN
        );
    }

    #[test]
    fn legacy_group_inventory_is_truthful_for_single_and_multi_group_interfaces() {
        let singleton = DiscoveryGroupSet::try_from_slice(&[
            personal_rns::interfaces::DiscoveryGroupId::parse("field").unwrap(),
        ])
        .unwrap();
        assert_eq!(singleton_discovery_group(&singleton), Some("field"));

        let multiple = DiscoveryGroupSet::try_from_slice(&[
            personal_rns::interfaces::DiscoveryGroupId::parse("field").unwrap(),
            personal_rns::interfaces::DiscoveryGroupId::parse("relay").unwrap(),
        ])
        .unwrap();
        assert_eq!(singleton_discovery_group(&multiple), None);
    }
}
