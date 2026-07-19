use heapless::Vec as HVec;
use personal_hopspot_core::{
    draw_with_state_footer_details_at, snapshots_to_cards, snapshots_to_interface_menu_details,
    splash, BatteryState, Card, CardActivityTracker, InputEvent, UiAction, UiNotice, UiState,
};
use personal_rns::interfaces::{InterfaceSnapshot, InterfaceStatus};
use personal_rns::storage::{GrowableHeap, StorageLayout};
use std::time::{Duration, Instant};

use crate::engine::{
    classify, ensure_started, interface_snapshots, sleep_interfaces, toggle_interface,
    wake_interfaces, wifi_status,
};
use crate::framebuffer::FrameBuffer;

const MAX_CARDS: usize = 16;
const NOTICE_TIMEOUT: Duration = Duration::from_millis(900);

fn ui_state() -> UiState {
    let mut state = UiState::new();
    state.set_storage_limits(<GrowableHeap as StorageLayout>::LIMITS);
    state
}

pub struct HopspotFace {
    state: UiState,
    framebuffer: FrameBuffer,
    battery: BatteryState,
    activity: CardActivityTracker<MAX_CARDS>,
    activity_started: Instant,
    notice_started: Option<Instant>,
}

impl HopspotFace {
    pub fn new() -> Self {
        ensure_started();
        Self {
            state: ui_state(),
            framebuffer: FrameBuffer::new(),
            battery: BatteryState::Unknown,
            activity: CardActivityTracker::new(),
            activity_started: Instant::now(),
            notice_started: None,
        }
    }

    fn show_notice(&mut self, notice: UiNotice) {
        self.state.show_notice(notice);
        self.notice_started = Some(Instant::now());
    }

    pub fn set_battery(&mut self, battery: BatteryState) {
        self.battery = battery;
    }

    pub fn post_input(&mut self, event: InputEvent) -> UiAction {
        let cards = self.build_cards();
        self.state.sync_card_count(cards.len());
        let selected_kind = self
            .state
            .selected_card(cards.len())
            .and_then(|index| cards.get(index))
            .map(|card| card.kind);
        let selected_id = self
            .state
            .selected_card(cards.len())
            .and_then(|index| cards.get(index))
            .map(|card| card.id);
        let action = self.state.handle_input(event, cards.len(), selected_kind);
        match action {
            UiAction::ToggleSelectedInterface => {
                if let Some(id) = selected_id {
                    let turning_on = cards.iter().any(|card| {
                        card.id == id && card.liveness == personal_hopspot_core::Liveness::Disabled
                    });
                    self.show_notice(if turning_on {
                        UiNotice::TurningOn
                    } else {
                        UiNotice::TurningOff
                    });
                    toggle_interface(id);
                }
            }
            UiAction::Sleep => {
                self.show_notice(UiNotice::Sleeping);
                sleep_interfaces();
            }
            UiAction::Wake => {
                self.show_notice(UiNotice::Awake);
                wake_interfaces();
            }
            UiAction::Announce => self.show_notice(UiNotice::Announcing),
            UiAction::None
            | UiAction::OledOff
            | UiAction::OpenDocs
            | UiAction::OpenLoRaEditor
            | UiAction::SetLoRaProfile(_)
            | UiAction::SwapRadioMode => {}
        }
        action
    }

    pub fn render(&mut self, out_rgba: &mut [u8]) {
        let snapshots = interface_snapshots();
        let mut cards = self.build_cards_from_snapshots(&snapshots);
        let elapsed = self.activity_started.elapsed();
        let activity_secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
        self.activity.update(&mut cards, activity_secs);
        self.render_cards(&cards, &snapshots, out_rgba);
    }

    fn build_cards(&self) -> HVec<Card, MAX_CARDS> {
        let snapshots = interface_snapshots();
        self.build_cards_from_snapshots(&snapshots)
    }

    fn build_cards_from_snapshots(&self, snapshots: &[InterfaceSnapshot]) -> HVec<Card, MAX_CARDS> {
        let wifi_id = wifi_status().id();
        snapshots_to_cards(snapshots, |id| classify(id, wifi_id))
    }

    fn render_cards(
        &mut self,
        cards: &[Card],
        snapshots: &[InterfaceSnapshot],
        out_rgba: &mut [u8],
    ) {
        self.state.sync_card_count(cards.len());
        if self
            .notice_started
            .is_some_and(|started| started.elapsed() >= NOTICE_TIMEOUT)
        {
            self.state.clear_notice();
            self.notice_started = None;
        }
        self.framebuffer.clear();
        if cards.is_empty() {
            splash(&mut self.framebuffer, "starting");
        } else {
            let animation_ms = self
                .activity_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let interface_menu_details = snapshots_to_interface_menu_details(
                self.state
                    .selected_card(cards.len())
                    .and_then(|index| cards.get(index)),
                snapshots,
            );
            draw_with_state_footer_details_at(
                &mut self.framebuffer,
                cards,
                self.battery,
                &self.state,
                None,
                &interface_menu_details,
                animation_ms,
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
    use personal_hopspot_core::{card_label, CardKind, Liveness};
    use personal_rns::interfaces::InterfaceId;

    impl HopspotFace {
        fn detached() -> Self {
            Self {
                state: ui_state(),
                framebuffer: FrameBuffer::new(),
                battery: BatteryState::Unknown,
                activity: CardActivityTracker::new(),
                activity_started: Instant::now(),
                notice_started: None,
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
            failure_reason: None,
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
            label: card_label("WiFi/LAN"),
            selected: false,
            liveness: Liveness::Live,
            failure_reason: None,
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
        face.render_cards(&cards, &[], &mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn an_empty_card_set_renders_the_starting_splash() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render_cards(&[], &[], &mut out);
        assert!(out.chunks_exact(4).any(|px| px != DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &[], &mut before);
        let _ = face
            .state
            .handle_input(InputEvent::ShortPress, cards.len(), None);
        face.render_cards(&cards, &[], &mut after);

        assert_ne!(before, after);
    }

    #[test]
    fn a_long_press_opens_a_menu_changing_the_screen() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &[], &mut before);
        let _ = face
            .state
            .handle_input(InputEvent::LongPress, cards.len(), None);
        face.render_cards(&cards, &[], &mut after);

        assert_ne!(before, after);
    }
}
