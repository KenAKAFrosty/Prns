use heapless::Vec as HVec;
use personal_hopspot_ui::{
    draw_with_state, BatteryState, Card, CardKind, InputEvent, UiAction, UiState,
};

use crate::framebuffer::FrameBuffer;

const MAX_CARDS: usize = 8;

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
        let cards = stub_cards();
        self.state.sync_card_count(cards.len());
        self.state.handle_input(event, cards.len())
    }

    pub fn render(&mut self, out_rgba: &mut [u8]) {
        let cards = stub_cards();
        self.state.sync_card_count(cards.len());
        self.framebuffer.clear();
        draw_with_state(
            &mut self.framebuffer,
            &cards,
            BatteryState::Unknown,
            &self.state,
        );
        self.framebuffer.expand_rgba(out_rgba);
    }
}

impl Default for HopspotFace {
    fn default() -> Self {
        Self::new()
    }
}

fn stub_cards() -> HVec<Card, MAX_CARDS> {
    let mut cards = HVec::new();
    let _ = cards.push(Card {
        kind: CardKind::Usb,
        label: "USB",
        selected: false,
        online: true,
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
        online: true,
        tx_bytes: 22_400_000,
        rx_bytes: 41_900_000,
        links: 4,
        destinations: 12,
        rate_bytes_per_sec: 96_000,
        last_activity_secs: Some(0),
    });
    let _ = cards.push(Card {
        kind: CardKind::LoRa,
        label: "LoRa",
        selected: false,
        online: false,
        tx_bytes: 3_200,
        rx_bytes: 1_100,
        links: 0,
        destinations: 1,
        rate_bytes_per_sec: 0,
        last_activity_secs: Some(58),
    });
    let _ = cards.push(Card {
        kind: CardKind::EspNow,
        label: "ESP-NOW",
        selected: false,
        online: true,
        tx_bytes: 540_000,
        rx_bytes: 612_000,
        links: 3,
        destinations: 7,
        rate_bytes_per_sec: 12_400,
        last_activity_secs: Some(5),
    });
    cards
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::{ARGB_BYTES, DARK_RGBA};

    fn fresh_buffer() -> Vec<u8> {
        vec![0u8; ARGB_BYTES]
    }

    #[test]
    fn a_fresh_render_lights_some_pixels() {
        let mut face = HopspotFace::new();
        let mut buf = fresh_buffer();
        face.render(&mut buf);
        assert!(buf.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut before = HopspotFace::new();
        let mut after = HopspotFace::new();
        let mut buf_before = fresh_buffer();
        let mut buf_after = fresh_buffer();

        before.render(&mut buf_before);
        let _ = after.post_input(InputEvent::ShortPress);
        after.render(&mut buf_after);

        assert_ne!(buf_before, buf_after);
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
