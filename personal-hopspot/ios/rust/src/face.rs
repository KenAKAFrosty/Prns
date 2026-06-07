//WIP NEEDS REVIEW
use heapless::Vec as HVec;
use personal_hopspot_ui::{
    draw_with_state, splash, BatteryState, Card, InputEvent, UiAction, UiState,
};

use crate::cards::{dummy_cards, MAX_CARDS};
use crate::framebuffer::FrameBuffer;

pub struct HopspotFace {
    state: UiState,
    framebuffer: FrameBuffer,
}

impl HopspotFace {
    pub fn new() -> Self {
        Self {
            state: UiState::new(),
            framebuffer: FrameBuffer::new(),
        }
    }

    pub fn post_input(&mut self, event: InputEvent) -> UiAction {
        let cards = self.build_cards();
        self.state.sync_card_count(cards.len());
        self.state.handle_input(event, cards.len())
    }

    pub fn render(&mut self, out_rgba: &mut [u8]) {
        let cards = self.build_cards();
        self.render_cards(&cards, out_rgba);
    }

    fn build_cards(&self) -> HVec<Card, MAX_CARDS> {
        dummy_cards()
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
    use crate::framebuffer::{DARK_RGBA, RGBA_BYTES};

    fn fresh_buffer() -> Vec<u8> {
        vec![0u8; RGBA_BYTES]
    }

    #[test]
    fn the_dummy_cards_light_some_pixels() {
        let mut face = HopspotFace::new();
        let mut out = fresh_buffer();
        face.render(&mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut face = HopspotFace::new();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render(&mut before);
        let _ = face.post_input(InputEvent::ShortPress);
        face.render(&mut after);

        assert_ne!(before, after);
    }

    #[test]
    fn a_long_press_opens_a_menu_changing_the_screen() {
        let mut face = HopspotFace::new();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render(&mut before);
        let _ = face.post_input(InputEvent::LongPress);
        face.render(&mut after);

        assert_ne!(before, after);
    }
}
