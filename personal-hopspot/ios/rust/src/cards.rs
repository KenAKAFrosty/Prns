pub const MAX_CARDS: usize = 8;

#[cfg(test)]
use heapless::Vec as HVec;
#[cfg(test)]
use personal_hopspot_ui::{card_label, Card, CardKind, Liveness};
#[cfg(test)]
use personal_rns::interfaces::InterfaceId;

#[cfg(test)]
pub fn dummy_cards() -> HVec<Card, MAX_CARDS> {
    let mut cards = HVec::new();
    let _ = cards.push(Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Usb,
        label: card_label("USB"),
        selected: false,
        liveness: Liveness::Live,
        tx_bytes: 1_204_000,
        rx_bytes: 938_000,
        links: 2,
        destinations: 5,
        rate_bytes_per_sec: 8_100,
        last_activity_secs: Some(2),
    });
    let _ = cards.push(Card {
        id: InterfaceId::new([1; 8]),
        kind: CardKind::Wifi,
        label: card_label("WiFi"),
        selected: false,
        liveness: Liveness::Live,
        tx_bytes: 22_400_000,
        rx_bytes: 41_900_000,
        links: 4,
        destinations: 12,
        rate_bytes_per_sec: 96_000,
        last_activity_secs: Some(0),
    });
    let _ = cards.push(Card {
        id: InterfaceId::new([2; 8]),
        kind: CardKind::EspNow,
        label: card_label("ESP-NOW"),
        selected: false,
        liveness: Liveness::Live,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 999_999,
        destinations: 1_234_567,
        rate_bytes_per_sec: 987_000,
        last_activity_secs: Some(0),
    });
    let _ = cards.push(Card {
        id: InterfaceId::new([3; 8]),
        kind: CardKind::Ble,
        label: card_label("BLE"),
        selected: false,
        liveness: Liveness::Live,
        tx_bytes: 42,
        rx_bytes: 12_340,
        links: 7,
        destinations: 12,
        rate_bytes_per_sec: 1_200,
        last_activity_secs: Some(42),
    });
    let _ = cards.push(Card {
        id: InterfaceId::new([4; 8]),
        kind: CardKind::LoRa,
        label: card_label("LoRa"),
        selected: false,
        liveness: Liveness::Failed,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    });
    cards
}
