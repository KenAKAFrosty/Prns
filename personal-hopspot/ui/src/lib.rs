#![no_std]

pub mod screen;

pub use screen::{
    card_label, draw, draw_with_state, splash, tcp_card_label, BatteryState, Card, CardKind,
    CardLabel, InputEvent, Liveness, UiAction, UiState,
};

use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceStatus};

fn liveness(connection: ConnectionState) -> Liveness {
    match connection {
        ConnectionState::Connected | ConnectionState::Degraded => Liveness::Live,
        ConnectionState::Failed | ConnectionState::Unknown => Liveness::Offline,
        ConnectionState::Initializing
        | ConnectionState::Reconnecting
        | ConnectionState::Disconnected => Liveness::Dormant,
    }
}

/// Build the renderable [`Card`] list from the interfaces' live status handles,
/// one card per handle, in the order given. `classify` maps each interface's
/// [`InterfaceId`] to its `(icon kind, label)`; returning `None` drops that
/// interface from the screen. `N` bounds the returned vector — pass the panel's
/// card capacity.
///
/// The handle carries the facts the interface knows first-hand: its connection — which resolves the
/// card's [`Liveness`] (Dormant until a link confirms, then Live) — the bytes it has moved, which
/// fill the Live card, and its live link count (a supervisor reports its peer count here). Rate and
/// last-activity are derived state the handle does not carry yet, so they report neutral values
/// until their own sources land.
pub fn statuses_to_cards<S: InterfaceStatus, const N: usize>(
    statuses: &[S],
    mut classify: impl FnMut(InterfaceId) -> Option<(CardKind, CardLabel)>,
) -> heapless::Vec<Card, N> {
    let mut cards = heapless::Vec::new();
    for status in statuses {
        let Some((kind, label)) = classify(status.id()) else {
            continue;
        };
        let _ = cards.push(Card {
            kind,
            label,
            selected: false,
            liveness: liveness(status.connection()),
            tx_bytes: status.tx_bytes(),
            rx_bytes: status.rx_bytes(),
            links: status.links(),
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        });
    }
    cards
}
