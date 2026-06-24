use heapless::Vec as HVec;
use personal_hopspot_ui::{
    draw_with_state, snapshots_to_cards, splash, BatteryState, Card, InputEvent, UiAction, UiState,
};
use personal_rns::interfaces::InterfaceStatus;

use crate::engine::{classify, ensure_started, interface_snapshots, wifi_status};
use crate::framebuffer::FrameBuffer;

const MAX_CARDS: usize = 16;

pub struct HopspotFace {
    state: UiState,
    framebuffer: FrameBuffer,
}

impl HopspotFace {
    pub fn new() -> Self {
        ensure_started();
        Self {
            state: UiState::new(),
            framebuffer: FrameBuffer::new(),
        }
    }

    pub fn post_input(&mut self, event: InputEvent) -> UiAction {
        let cards = self.build_cards();
        self.state.sync_card_count(cards.len());
        let selected_kind = self
            .state
            .selected_card(cards.len())
            .and_then(|index| cards.get(index))
            .map(|card| card.kind);
        self.state.handle_input(event, cards.len(), selected_kind)
    }

    pub fn render(&mut self, out_rgba: &mut [u8]) {
        let cards = self.build_cards();
        self.render_cards(&cards, out_rgba);
    }

    /// The unified snapshot per interface each frame: the USB host and the WiFi supervisor at the
    /// root with their fleets folded in, the same shape every other face renders.
    fn build_cards(&self) -> HVec<Card, MAX_CARDS> {
        let snapshots = interface_snapshots();
        let wifi_id = wifi_status().id();
        snapshots_to_cards(&snapshots, |id| classify(id, wifi_id))
    }

    fn render_cards(&mut self, cards: &[Card], out_rgba: &mut [u8]) {
        self.state.sync_card_count(cards.len());
        self.framebuffer.clear();
        if cards.is_empty() {
            splash(&mut self.framebuffer, "starting");
        } else {
            draw_with_state(
                &mut self.framebuffer,
                cards,
                BatteryState::Unknown,
                &self.state,
            );
        }
        self.framebuffer.expand_rgba(out_rgba);
    }
}

impl Default for HopspotFace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::{ARGB_BYTES, DARK_RGBA};
    use personal_hopspot_ui::{card_label, CardKind, Liveness};
    use personal_rns::interfaces::InterfaceId;

    impl HopspotFace {
        fn detached() -> Self {
            Self {
                state: UiState::new(),
                framebuffer: FrameBuffer::new(),
            }
        }
    }

    fn stub_cards() -> HVec<Card, MAX_CARDS> {
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
            id: InterfaceId::new([0; 8]),
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
        cards
    }

    fn fresh_buffer() -> Vec<u8> {
        vec![0u8; ARGB_BYTES]
    }

    #[test]
    fn rendered_cards_light_some_pixels() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut out = fresh_buffer();
        face.render_cards(&cards, &mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn an_empty_card_set_renders_the_starting_splash() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render_cards(&[], &mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &mut before);
        let _ = face.state.handle_input(InputEvent::ShortPress, cards.len());
        face.render_cards(&cards, &mut after);

        assert_ne!(before, after);
    }

    #[test]
    fn a_long_press_opens_a_menu_changing_the_screen() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &mut before);
        let _ = face.state.handle_input(InputEvent::LongPress, cards.len());
        face.render_cards(&cards, &mut after);

        assert_ne!(before, after);
    }
}
