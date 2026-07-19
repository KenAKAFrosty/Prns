use super::*;

/// Build the unified snapshot set for the e-ink: the LoRa wire, the USB-auto wire, the BLE
/// aggregate, and one entry per settled peer, matching the Heltec/T-Beam and host faces.
pub(super) fn build_snapshots(
    lora: &EmbassyInterfaceStatus,
    usb: &EmbassyInterfaceStatus,
) -> heapless::Vec<InterfaceSnapshot, { crate::BLE_MEMBERS + 4 }> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), { crate::BLE_MEMBERS + 4 }> =
        heapless::Vec::new();
    let _ = entries.push((lora, Membership::Independent));
    let _ = entries.push((usb, Membership::Independent));
    let supervisor_id = ble.id();
    let _ = entries.push((&ble, Membership::Independent));
    for member in ble.members() {
        let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
    }
    let mut snapshots: heapless::Vec<InterfaceSnapshot, { crate::BLE_MEMBERS + 4 }> =
        heapless::Vec::new();
    for (status, membership) in &entries {
        let id = status.id();
        let counts = crate::INTERFACE_STORE.counts(id);
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
    lora_id: InterfaceId,
    usb_id: InterfaceId,
) -> heapless::Vec<hopspot::Card, { crate::BLE_MEMBERS + 4 }> {
    let classify = |id: InterfaceId| -> Option<(hopspot::CardKind, hopspot::CardLabel)> {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else if id == usb_id {
            Some((hopspot::CardKind::Usb, hopspot::card_label("USB")))
        } else if id == crate::BLE_FLEET_ID {
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
