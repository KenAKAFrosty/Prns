use heapless::Vec as HVec;
use personal_hopspot_ui::{
    draw_with_state, snapshots_to_cards, splash, BatteryState, Card, InputEvent, UiAction, UiState,
};
use personal_rns::interfaces::{InterfaceSnapshot, InterfaceStatus, Membership};
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;

use crate::cards::MAX_CARDS;
use crate::engine::{classify, shared_status};
use crate::framebuffer::FrameBuffer;

pub struct HopspotFace {
    state: UiState,
    framebuffer: FrameBuffer,
    statuses: Vec<TokioInterfaceStatus>,
}

impl HopspotFace {
    pub fn new() -> Self {
        Self {
            state: UiState::new(),
            framebuffer: FrameBuffer::new(),
            statuses: std::vec![shared_status()],
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

    fn build_cards(&self) -> HVec<Card, MAX_CARDS> {
        let snapshots: HVec<InterfaceSnapshot, MAX_CARDS> = self
            .statuses
            .iter()
            .map(|status| InterfaceSnapshot {
                id: status.id(),
                connection: status.connection(),
                rx_bytes: status.rx_bytes(),
                tx_bytes: status.tx_bytes(),
                transfer_rates: status.transfer_rates(),
                destinations: 0,
                links: 0,
                transported_links: 0,
                membership: Membership::Independent,
            })
            .collect();
        snapshots_to_cards(&snapshots, classify)
    }

    fn render_cards(&mut self, cards: &[Card], out_rgba: &mut [u8]) {
        self.state.sync_card_count(cards.len());
        self.framebuffer.clear();
        if cards.is_empty() {
            splash(&mut self.framebuffer, "connecting");
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
    use crate::cards::dummy_cards;
    use crate::framebuffer::{DARK_RGBA, RGBA_BYTES};

    impl HopspotFace {
        fn detached() -> Self {
            Self {
                state: UiState::new(),
                framebuffer: FrameBuffer::new(),
                statuses: Vec::new(),
            }
        }
    }

    fn fresh_buffer() -> Vec<u8> {
        vec![0u8; RGBA_BYTES]
    }

    #[test]
    fn rendered_cards_light_some_pixels() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render_cards(&dummy_cards(), &mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn an_empty_status_set_renders_the_connecting_splash() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render(&mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut face = HopspotFace::detached();
        let cards = dummy_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &mut before);
        let _ = face
            .state
            .handle_input(InputEvent::ShortPress, cards.len(), None);
        face.render_cards(&cards, &mut after);

        assert_ne!(before, after);
    }

    #[test]
    fn a_long_press_opens_a_menu_changing_the_screen() {
        let mut face = HopspotFace::detached();
        let cards = dummy_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &mut before);
        let _ = face
            .state
            .handle_input(InputEvent::LongPress, cards.len(), None);
        face.render_cards(&cards, &mut after);

        assert_ne!(before, after);
    }
}
