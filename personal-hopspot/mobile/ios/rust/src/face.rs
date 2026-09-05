use heapless::Vec as HVec;
use personal_hopspot_core::{
    expand_face_rgba, face_64x128, interface_mode_slot, load_host_interface_modes,
    save_host_interface_modes, snapshots_to_cards, snapshots_to_interface_menu_details,
    AccessPointState, Card, CardActivityTracker, InputEvent, InterfaceModeTable, PowerSnapshot,
    ScreenContent, UiAction, UiConfiguration, UiNotice, UiState, UserBlanking,
    INTERFACE_MODE_STORAGE, MOBILE_RGBA_BYTES,
};
use personal_rns::interfaces::InterfaceSnapshot;
use personal_rns::storage::{GrowableHeap, StorageLayout};
use std::time::{Duration, Instant};

use crate::cards::MAX_CARDS;
use crate::engine::{
    classify, interface_snapshots, sleep_interfaces, storage_directory, toggle_interface,
    wake_interfaces,
};
const NOTICE_TIMEOUT: Duration = Duration::from_millis(900);

fn ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: <GrowableHeap as StorageLayout>::LIMITS,
        user_blanking: UserBlanking::unavailable(),
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export:
            personal_hopspot_core::SharedInstanceConfigExport::Unavailable,
        gnss: personal_hopspot_core::GnssAvailability::Unavailable,
    })
}

pub struct HopspotFace {
    state: UiState,
    framebuffer: face_64x128::Frame,
    battery: PowerSnapshot,
    activity: CardActivityTracker<MAX_CARDS>,
    activity_started: Instant,
    notice_started: Option<Instant>,
    interface_modes: InterfaceModeTable,
    modes_bound: bool,
}

impl HopspotFace {
    pub fn new() -> Self {
        Self {
            state: ui_state(),
            framebuffer: face_64x128::Frame::new(),
            battery: PowerSnapshot::UNKNOWN,
            activity: CardActivityTracker::new(),
            activity_started: Instant::now(),
            notice_started: None,
            interface_modes: InterfaceModeTable::DEFAULT,
            modes_bound: false,
        }
    }

    fn bind_modes_if_ready(&mut self) {
        if self.modes_bound {
            return;
        }
        let Some(storage_dir) = storage_directory() else {
            return;
        };
        self.interface_modes = load_host_interface_modes(&storage_dir.join(INTERFACE_MODE_STORAGE));
        self.modes_bound = true;
    }

    fn show_notice(&mut self, notice: UiNotice) {
        self.state.show_notice(notice);
        self.notice_started = Some(Instant::now());
    }

    pub fn set_battery(&mut self, battery: PowerSnapshot) {
        self.battery = battery;
    }

    pub fn post_input(&mut self, event: InputEvent) -> UiAction {
        self.bind_modes_if_ready();
        let cards = self.build_cards();
        let content = ScreenContent {
            cards: &cards,
            local_docs: None,
            interface_menu_details: None,
        };
        let action = self.state.handle_input(event, content);
        match action {
            UiAction::ToggleSelectedInterface => {
                if let Some(card) = self.state.selected_card(content.cards) {
                    let id = card.id();
                    let turning_on =
                        card.connection() == personal_rns::interfaces::ConnectionState::Disabled;
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
            UiAction::CopySharedInstanceConfig => {}
            UiAction::OpenInterfaceModeEditor => {
                if let Some(card) = self.state.selected_card(content.cards) {
                    if let Some(slot) = interface_mode_slot(card.kind()) {
                        let mut selection = self.interface_modes.get(slot);
                        selection.mode = card.mode();
                        self.state.open_interface_mode_editor(slot, selection);
                    }
                }
            }
            UiAction::SetInterfaceMode { slot, selection } => {
                self.interface_modes.set(slot, selection);
                let notice = match storage_directory() {
                    Some(storage_dir) => {
                        let path = storage_dir.join(INTERFACE_MODE_STORAGE);
                        if save_host_interface_modes(&path, self.interface_modes).is_ok() {
                            UiNotice::Saved
                        } else {
                            UiNotice::ProfileNotSaved
                        }
                    }
                    None => UiNotice::ProfileNotSaved,
                };
                self.show_notice(notice);
            }
            UiAction::None
            | UiAction::BlankDisplay
            | UiAction::ToggleDisplayAutoOff
            | UiAction::ControlGnss(_)
            | UiAction::ToggleStationUplink
            | UiAction::OpenLoRaEditor
            | UiAction::OpenDocs
            | UiAction::SetLoRaProfile(_)
            | UiAction::ResetLoRaProfile
            | UiAction::SwapRadioMode => {}
        }
        action
    }

    pub fn render(&mut self, out_rgba: &mut [u8; MOBILE_RGBA_BYTES]) {
        self.bind_modes_if_ready();
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
        let mut owned: std::vec::Vec<_> = snapshots.to_vec();
        for snapshot in &mut owned {
            snapshot.mode =
                personal_hopspot_core::mode_from_table(self.interface_modes, snapshot.id.kind());
        }
        snapshots_to_cards(&owned, classify)
    }

    fn render_cards(
        &mut self,
        cards: &[Card],
        snapshots: &[InterfaceSnapshot],
        out_rgba: &mut [u8; MOBILE_RGBA_BYTES],
    ) {
        let interface_menu_details =
            snapshots_to_interface_menu_details(self.state.selected_card(cards), snapshots);
        let content = ScreenContent {
            cards,
            local_docs: None,
            interface_menu_details: Some(&interface_menu_details),
        };
        self.state.sync(content);
        if self
            .notice_started
            .is_some_and(|started| started.elapsed() >= NOTICE_TIMEOUT)
        {
            self.state.clear_notice();
            self.notice_started = None;
        }
        if cards.is_empty() {
            face_64x128::splash(
                &mut self.framebuffer,
                face_64x128::SplashContent::Connecting,
            );
        } else {
            face_64x128::render(
                &mut self.framebuffer,
                face_64x128::RenderInput {
                    content,
                    battery: self.battery,
                    gnss: None,
                    state: &self.state,
                    interface_menu_details: &interface_menu_details,
                },
            );
        }
        expand_face_rgba(&self.framebuffer, out_rgba);
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
    use personal_hopspot_core::MOBILE_DARK_RGBA;

    impl HopspotFace {
        fn detached() -> Self {
            Self {
                state: ui_state(),
                framebuffer: face_64x128::Frame::new(),
                battery: PowerSnapshot::UNKNOWN,
                activity: CardActivityTracker::new(),
                activity_started: Instant::now(),
                notice_started: None,
                interface_modes: InterfaceModeTable::DEFAULT,
                modes_bound: true,
            }
        }
    }

    fn fresh_buffer() -> [u8; MOBILE_RGBA_BYTES] {
        [0; MOBILE_RGBA_BYTES]
    }

    #[test]
    fn rendered_cards_light_some_pixels() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render_cards(&dummy_cards(), &[], &mut out);
        assert!(out
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| *px != MOBILE_DARK_RGBA));
    }

    #[test]
    fn an_empty_card_set_renders_the_connecting_splash() {
        let mut face = HopspotFace::detached();
        let mut out = fresh_buffer();
        face.render_cards(&[], &[], &mut out);
        assert!(out
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| *px != MOBILE_DARK_RGBA));
    }

    #[test]
    fn a_short_press_changes_the_rendered_screen() {
        let mut face = HopspotFace::detached();
        let cards = dummy_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &[], &mut before);
        let _ = face.state.handle_input(
            InputEvent::ShortPress,
            ScreenContent {
                cards: &cards,
                local_docs: None,
                interface_menu_details: None,
            },
        );
        face.render_cards(&cards, &[], &mut after);

        assert_ne!(before, after);
    }

    #[test]
    fn a_long_press_opens_a_menu_changing_the_screen() {
        let mut face = HopspotFace::detached();
        let cards = dummy_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &[], &mut before);
        let _ = face.state.handle_input(
            InputEvent::LongPress,
            ScreenContent {
                cards: &cards,
                local_docs: None,
                interface_menu_details: None,
            },
        );
        face.render_cards(&cards, &[], &mut after);

        assert_ne!(before, after);
    }
}
