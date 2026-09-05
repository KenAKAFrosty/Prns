use core::fmt::Write as _;

use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::interfaces::{InterfaceId, InterfaceSnapshot, InterfaceStatus, Membership};

use super::bluetooth_auto::{BLE_SHARED, BLE_SUPERVISOR_ID, MEMBERS};
use super::node::INTERFACE_STORE;

pub(super) fn build_snapshots(
    lora: &dyn InterfaceStatus,
    usb: &dyn InterfaceStatus,
    modes: hopspot::InterfaceModeTable,
) -> heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), { MEMBERS + 4 }> =
        heapless::Vec::new();
    let _ = entries.push((lora, Membership::Independent));
    let _ = entries.push((usb, Membership::Independent));
    let supervisor_id = ble.id();
    let _ = entries.push((&ble, Membership::Independent));
    for member in ble.members() {
        let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
    }
    let mut snapshots: heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }> = heapless::Vec::new();
    for (status, membership) in &entries {
        let id = status.id();
        let counts = INTERFACE_STORE.counts(id);
        let _ = snapshots.push(InterfaceSnapshot {
            id,
            mode: hopspot::mode_from_table(modes, id.kind()),
            gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
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
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, { MEMBERS + 4 }> {
    let classify = |id: InterfaceId| -> Option<(hopspot::CardKind, hopspot::CardLabel)> {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else if id == usb_id {
            Some((hopspot::CardKind::Usb, hopspot::card_label("USB")))
        } else if id == BLE_SUPERVISOR_ID {
            Some((hopspot::CardKind::Ble, hopspot::card_label("BLE")))
        } else {
            let bytes = id.as_bytes();
            let mut label = hopspot::CardLabel::new();
            let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
            Some((hopspot::CardKind::Peer, label))
        }
    };
    hopspot::snapshots_to_cards(snapshots, classify)
}
