pub const MAX_CARDS: usize = 8;

#[cfg(test)]
use heapless::Vec as HVec;
#[cfg(test)]
use personal_hopspot_ui::{Card, CardKind, Liveness};

#[cfg(test)]
pub fn dummy_cards() -> HVec<Card, MAX_CARDS> {
    let mut cards = HVec::new();
    let _ = cards.push(Card {
        kind: CardKind::Usb,
        label: "USB",
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
        kind: CardKind::Wifi,
        label: "WiFi",
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
        kind: CardKind::EspNow,
        label: "ESP-NOW",
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
        kind: CardKind::Ble,
        label: "BLE",
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
        kind: CardKind::LoRa,
        label: "LoRa",
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
