#![no_std]

pub mod screen;

pub use screen::{
    card_label, draw, draw_with_state, splash, tcp_card_label, BatteryState, Card, CardKind,
    CardLabel, InputEvent, Liveness, UiAction, UiState,
};

use personal_rns::engine::InterfaceCounts;
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
/// card's [`Liveness`] (Dormant until a link confirms, then Live) — and the bytes it has moved, which
/// fill the Live card. `counts` supplies the engine-owned figures the interface itself can't know —
/// destinations routed via it and live Reticulum links over it — which a face reads through the
/// runtime's per-interface query. The card's rate is the interface's own published throughput (rx
/// plus tx, as bytes per second); last-activity is derived state no source carries yet, so it
/// reports a neutral value until then.
pub fn statuses_to_cards<S: InterfaceStatus, const N: usize>(
    statuses: &[S],
    mut classify: impl FnMut(InterfaceId) -> Option<(CardKind, CardLabel)>,
    mut counts: impl FnMut(InterfaceId) -> InterfaceCounts,
) -> heapless::Vec<Card, N> {
    let mut cards = heapless::Vec::new();
    for status in statuses {
        let id = status.id();
        let Some((kind, label)) = classify(id) else {
            continue;
        };
        let InterfaceCounts {
            destinations,
            links,
        } = counts(id);
        let _ = cards.push(Card {
            kind,
            label,
            selected: false,
            liveness: liveness(status.connection()),
            tx_bytes: status.tx_bytes(),
            rx_bytes: status.rx_bytes(),
            links,
            destinations,
            rate_bytes_per_sec: status
                .transfer_rates()
                .map(|rates| rates.rx_bps.saturating_add(rates.tx_bps) / 8)
                .unwrap_or(0),
            last_activity_secs: None,
        });
    }
    cards
}
