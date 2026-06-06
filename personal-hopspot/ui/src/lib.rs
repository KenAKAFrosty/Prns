//! The Personal Hopspot status screen — one renderer for both faces of the app:
//! the S3's SSD1306 OLED and the Linux debug window. [`draw`]/[`splash`] are
//! generic over any `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the
//! identical pixels land on the panel and on `embedded-graphics-simulator`.
//!
//! [`snapshot_to_cards`] turns a runtime snapshot into the renderable [`Card`]
//! list. [`UiState`] adds the small amount of single-button interaction the
//! renderer needs: short press advances from the global row through the
//! interface cards and keeps the focused item visible; long press opens either
//! the global menu or the focused interface's menu, where short press advances
//! dummy rows and long press exits. The engine snapshot is deliberately
//! product-agnostic — it carries only each interface's opaque [`InterfaceId`] —
//! so the host supplies the icon kind and label (its own product knowledge)
//! through a `classify` closure.

#![no_std]

pub mod screen;

pub use screen::{
    draw, draw_with_state, splash, BatteryState, Card, CardKind, InputEvent, UiAction, UiState,
};

use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::runtime::RuntimeSnapshot;

/// Build the renderable [`Card`] list from a runtime snapshot, one card per
/// interface view, in snapshot order. `classify` maps each interface's
/// [`InterfaceId`] to its `(icon kind, label)`; returning `None` drops that
/// interface from the screen (e.g. a board hiding a card that doesn't fit yet).
/// `N` bounds the returned vector — pass the panel's card capacity.
pub fn snapshot_to_cards<const N: usize>(
    snapshot: &RuntimeSnapshot,
    mut classify: impl FnMut(InterfaceId) -> Option<(CardKind, &'static str)>,
) -> heapless::Vec<Card, N> {
    let mut cards = heapless::Vec::new();
    for view in &snapshot.interfaces {
        let Some((kind, label)) = classify(view.id) else {
            continue;
        };
        let _ = cards.push(Card {
            kind,
            label,
            selected: false,
            // The status dot collapses the engine's ConnectionState: an interface
            // reads as online when it is routable (Connected or Degraded), matching
            // the engine's own routability grouping.
            online: matches!(
                view.connection_state,
                ConnectionState::Connected | ConnectionState::Degraded
            ),
            tx_bytes: view.reticulum_tx_byte_count,
            rx_bytes: view.reticulum_rx_byte_count,
            // The runtime snapshot does not expose active-link counts yet. Keep
            // the renderer surface ready while reporting the honest current
            // value.
            links: 0,
            destinations: view.tracked_destinations,
            // The runtime snapshot does not expose rate or last-activity age yet.
            // Keep the renderer surface ready while reporting neutral values.
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        });
    }
    cards
}
