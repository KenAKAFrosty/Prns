#![no_std]

pub mod battery;
pub mod screen;

pub use battery::{BatteryGauge, BatterySource, NoBattery};
pub use screen::{
    card_label, draw, draw_with_state, splash, tcp_card_label, BatteryState, Card,
    CardActivityTracker, CardKind, CardLabel, InputEvent, Liveness, UiAction, UiState,
};

use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceSnapshot, Membership};

/// The faces' redraw-coalescing window, in milliseconds. A burst of engine changes inside this span
/// folds into one repaint (~30 fps). It bounds how fast a face repaints when things change; it is not
/// a frame clock — a face wakes on the store's signal and stays idle when nothing moves.
pub const COALESCE_MS: u64 = 33;

fn liveness(connection: ConnectionState) -> Liveness {
    match connection {
        ConnectionState::Connected | ConnectionState::Degraded => Liveness::Live,
        ConnectionState::Failed | ConnectionState::Unknown => Liveness::Failed,
        ConnectionState::Disabled => Liveness::Disabled,
        ConnectionState::Initializing
        | ConnectionState::Reconnecting
        | ConnectionState::Disconnected => Liveness::Dormant,
    }
}

/// Build the renderable [`Card`] list from one [`InterfaceSnapshot`] per interface. `classify` maps
/// an [`InterfaceId`] to its `(icon kind, label)`; returning `None` drops that interface. `N` bounds
/// the returned vector — pass the panel's card capacity.
///
/// A supervisor's fleet folds in: a [`FleetMember`](Membership::FleetMember) gets no card of its own,
/// and its engine counts roll up into its supervisor's card, so the root shows one card per
/// independent interface with the whole fleet's traffic summed under it — never a card per peer.
///
/// The snapshot carries everything else a card shows: the connection (which resolves the card's
/// [`Liveness`]), the bytes and rate the interface moved, and the engine counts riding over it. The
/// link glyph sums the two kinds the engine reports apart — links this node terminates and links it
/// merely carries for others — into one count of every live link on the interface.
pub fn snapshots_to_cards<const N: usize>(
    snapshots: &[InterfaceSnapshot],
    mut classify: impl FnMut(InterfaceId) -> Option<(CardKind, CardLabel)>,
) -> heapless::Vec<Card, N> {
    let mut cards = heapless::Vec::new();
    for snapshot in snapshots {
        if let Membership::FleetMember { .. } = snapshot.membership {
            continue;
        }
        let Some((kind, label)) = classify(snapshot.id) else {
            continue;
        };
        let mut destinations = snapshot.destinations;
        let mut links = snapshot.links;
        let mut transported_links = snapshot.transported_links;
        for member in snapshots {
            if let Membership::FleetMember { supervisor_id } = member.membership {
                if supervisor_id == snapshot.id {
                    destinations = destinations.saturating_add(member.destinations);
                    links = links.saturating_add(member.links);
                    transported_links = transported_links.saturating_add(member.transported_links);
                }
            }
        }
        let _ = cards.push(Card {
            id: snapshot.id,
            kind,
            label,
            selected: false,
            liveness: liveness(snapshot.connection),
            tx_bytes: snapshot.tx_bytes,
            rx_bytes: snapshot.rx_bytes,
            links: links.saturating_add(transported_links),
            destinations,
            rate_bytes_per_sec: snapshot
                .transfer_rates
                .map(|rates| rates.rx_bps.saturating_add(rates.tx_bps) / 8)
                .unwrap_or(0),
            last_activity_secs: None,
        });
    }
    cards
}
