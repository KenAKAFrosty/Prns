use core::fmt::Write as _;

use personal_hopspot_core as hopspot;
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::interfaces::subghz::SubGConfigurationState;
use personal_rns::interfaces::{InterfaceId, InterfaceSnapshot, InterfaceStatus, Membership};

use super::bluetooth_auto::{BLE_SHARED, BLE_SUPERVISOR_ID, MEMBERS};
use super::node::INTERFACE_STORE;

pub(super) fn build_snapshots(
    lora: &dyn InterfaceStatus,
    usb: &dyn InterfaceStatus,
) -> heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), { MEMBERS + 4 }> =
        heapless::Vec::new();
    assert!(
        entries.push((lora, Membership::Independent)).is_ok(),
        "interface capacity covers LoRa"
    );
    assert!(
        entries.push((usb, Membership::Independent)).is_ok(),
        "interface capacity covers USB"
    );
    let supervisor_id = ble.id();
    assert!(
        entries.push((&ble, Membership::Independent)).is_ok(),
        "interface capacity covers Bluetooth"
    );
    for member in ble.members() {
        assert!(
            entries
                .push((member, Membership::FleetMember { supervisor_id }))
                .is_ok(),
            "interface capacity covers Bluetooth members"
        );
    }
    let mut snapshots: heapless::Vec<InterfaceSnapshot, { MEMBERS + 4 }> = heapless::Vec::new();
    for (status, membership) in &entries {
        let id = status.id();
        let counts = INTERFACE_STORE.counts(id);
        assert!(
            snapshots
                .push(InterfaceSnapshot {
                    id,
                    mode: personal_rns::interfaces::InterfaceMode::Full,
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
                    radio: status.radio(),
                    details: status.details(),
                })
                .is_ok(),
            "snapshot capacity matches interface capacity"
        );
    }
    snapshots
}

pub(super) fn build_cards(
    snapshots: &[InterfaceSnapshot],
    subg_configuration: SubGConfigurationState,
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, { MEMBERS + 4 }> {
    let classify = |id: InterfaceId| -> Option<(hopspot::CardKind, hopspot::CardLabel)> {
        if id == lora_id {
            Some(hopspot::subg_card(subg_configuration))
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
