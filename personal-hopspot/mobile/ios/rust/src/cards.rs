pub const MAX_CARDS: usize = 16;

#[cfg(test)]
use heapless::Vec as HVec;
#[cfg(test)]
use personal_hopspot_core::{card_label, snapshots_to_cards, Card, CardKind};
#[cfg(test)]
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceSnapshot, Membership, TransferRates,
};

#[cfg(test)]
fn snapshot(
    tag: u8,
    connection: ConnectionState,
    tx_bytes: u64,
    rx_bytes: u64,
    links: u32,
    destinations: u32,
    rate_bytes_per_sec: u32,
) -> InterfaceSnapshot {
    InterfaceSnapshot {
        id: InterfaceId::new([tag, 0, 0, 0, 0, 0, 0, 0]),
        connection,
        failure_reason: None,
        rx_bytes,
        tx_bytes,
        transfer_rates: Some(TransferRates {
            rx_bps: rate_bytes_per_sec.saturating_mul(8),
            tx_bps: 0,
        }),
        destinations,
        links,
        transported_links: 0,
        membership: Membership::Independent,
    }
}

#[cfg(test)]
pub fn dummy_cards() -> HVec<Card, MAX_CARDS> {
    let snapshots = [
        snapshot(
            1,
            ConnectionState::Connected,
            1_204_000,
            938_000,
            2,
            5,
            8_100,
        ),
        snapshot(
            2,
            ConnectionState::Connected,
            22_400_000,
            41_900_000,
            4,
            12,
            96_000,
        ),
        snapshot(
            3,
            ConnectionState::Connected,
            0,
            0,
            999_999,
            1_234_567,
            987_000,
        ),
        snapshot(4, ConnectionState::Connected, 42, 12_340, 7, 12, 1_200),
        snapshot(5, ConnectionState::Failed, 0, 0, 0, 0, 0),
    ];
    snapshots_to_cards(&snapshots, |id| match id.as_bytes()[0] {
        1 => Some((CardKind::Usb, card_label("USB"))),
        2 => Some((CardKind::Wifi, card_label("WiFi/LAN"))),
        3 => Some((CardKind::EspNow, card_label("ESP-NOW"))),
        4 => Some((CardKind::Ble, card_label("BLE"))),
        5 => Some((CardKind::LoRa, card_label("LoRa"))),
        _ => None,
    })
}
