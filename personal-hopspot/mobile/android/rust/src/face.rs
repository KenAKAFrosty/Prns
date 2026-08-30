use heapless::Vec as HVec;
use personal_hopspot_core::{
    ble_interface_menu_details, expand_face_rgba, face_64x128, snapshots_to_cards,
    AccessPointState, Card, CardActivityTracker, InputEvent, PowerSnapshot, ScreenContent,
    UiAction, UiConfiguration, UiNotice, UiState, UserBlanking, MOBILE_RGBA_BYTES,
};
use personal_rns::interfaces::InterfaceSnapshot;
use personal_rns::storage::{GrowableHeap, StorageLayout};
use std::time::{Duration, Instant};

use crate::engine::{
    classify, interface_snapshots, sleep_interfaces, toggle_interface, wake_interfaces,
};
const MAX_CARDS: usize = 16;
const NOTICE_TIMEOUT: Duration = Duration::from_millis(900);

fn ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: <GrowableHeap as StorageLayout>::LIMITS,
        user_blanking: UserBlanking::unavailable(),
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: personal_hopspot_core::SharedInstanceConfigExport::Available,
        gnss: personal_hopspot_core::GnssAvailability::Unavailable,
        ble_group_editor: personal_hopspot_core::BleGroupEditor::Available,
    })
}

pub struct HopspotFace {
    state: UiState,
    framebuffer: face_64x128::Frame,
    battery: PowerSnapshot,
    activity: CardActivityTracker<MAX_CARDS>,
    activity_started: Instant,
    notice_started: Option<Instant>,
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
        }
    }

    fn show_notice(&mut self, notice: UiNotice) {
        self.state.show_notice(notice);
        self.notice_started = Some(Instant::now());
    }

    pub fn set_battery(&mut self, battery: PowerSnapshot) {
        self.battery = battery;
    }

    pub fn post_input(&mut self, event: InputEvent) -> UiAction {
        let cards = self.build_cards();
        let content = ScreenContent {
            cards: &cards,
            local_docs: None,
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
            UiAction::None
            | UiAction::BlankDisplay
            | UiAction::ToggleDisplayAutoOff
            | UiAction::ControlGnss(_)
            | UiAction::ToggleStationUplink
            | UiAction::OpenDocs
            | UiAction::OpenLoRaEditor
            | UiAction::SetLoRaProfile(_)
            | UiAction::ResetLoRaProfile
            | UiAction::SwapRadioMode => {}
            UiAction::OpenBleGroupEditor => {
                let group = crate::engine::ble_discovery_group()
                    .unwrap_or_else(|| personal_hopspot_core::DEFAULT_BLE_GROUP.to_string());
                self.state.open_ble_group_editor(&group);
            }
            UiAction::SetBleDiscoveryGroup(name) => {
                if crate::engine::set_ble_discovery_group(name.as_str()) {
                    self.show_notice(UiNotice::Saved);
                } else {
                    self.show_notice(UiNotice::ApplyFailed);
                }
            }
        }
        action
    }

    pub fn render(&mut self, out_rgba: &mut [u8; MOBILE_RGBA_BYTES]) {
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
        snapshots_to_cards(snapshots, classify)
    }

    fn render_cards(
        &mut self,
        cards: &[Card],
        snapshots: &[InterfaceSnapshot],
        out_rgba: &mut [u8; MOBILE_RGBA_BYTES],
    ) {
        let content = ScreenContent {
            cards,
            local_docs: None,
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
            face_64x128::splash(&mut self.framebuffer, face_64x128::SplashContent::Starting);
        } else {
            let group = crate::engine::ble_discovery_group();
            let interface_menu_details = ble_interface_menu_details(
                group.as_deref(),
                self.state.selected_card(content.cards),
                snapshots,
            );
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
    use personal_hopspot_core::{card_label, CardKind, MOBILE_DARK_RGBA};
    use personal_rns::interfaces::{
        ConnectionState, InterfaceId, InterfaceSnapshot, Membership, TransferRates,
    };

    impl HopspotFace {
        fn detached() -> Self {
            Self {
                state: ui_state(),
                framebuffer: face_64x128::Frame::new(),
                battery: PowerSnapshot::UNKNOWN,
                activity: CardActivityTracker::new(),
                activity_started: Instant::now(),
                notice_started: None,
            }
        }
    }

    fn snapshot(
        tag: u8,
        tx_bytes: u64,
        rx_bytes: u64,
        links: u32,
        destinations: u32,
        rate_bytes_per_sec: u32,
    ) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id: InterfaceId::new([tag, 0, 0, 0, 0, 0, 0, 0]),
            mode: personal_rns::interfaces::InterfaceMode::Full,
            gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes,
            tx_bytes,
            transfer_rates: Some(TransferRates {
                rx_bps: rate_bytes_per_sec.saturating_mul(8),
                tx_bps: 0,
            }),
            destinations,
            links,
            transported_links: 0,
            membership: Membership::Independent,
        }
    }

    fn stub_cards() -> HVec<Card, MAX_CARDS> {
        let snapshots = [
            snapshot(1, 1_204_000, 938_000, 2, 5, 8_100),
            snapshot(2, 22_400_000, 41_900_000, 4, 12, 96_000),
        ];
        snapshots_to_cards(&snapshots, |id| match id.as_bytes()[0] {
            1 => Some((CardKind::Usb, card_label("USB"))),
            2 => Some((CardKind::Wifi, card_label("LAN"))),
            _ => None,
        })
    }

    fn fresh_buffer() -> [u8; MOBILE_RGBA_BYTES] {
        [0; MOBILE_RGBA_BYTES]
    }

    #[test]
    fn rendered_cards_light_some_pixels() {
        let mut face = HopspotFace::detached();
        let cards = stub_cards();
        let mut out = fresh_buffer();
        face.render_cards(&cards, &[], &mut out);
        assert!(out
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| *px != MOBILE_DARK_RGBA));
    }

    #[test]
    fn an_empty_card_set_renders_the_starting_splash() {
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
        let cards = stub_cards();
        let mut before = fresh_buffer();
        let mut after = fresh_buffer();

        face.render_cards(&cards, &[], &mut before);
        let _ = face.state.handle_input(
            InputEvent::ShortPress,
            ScreenContent {
                cards: &cards,
                local_docs: None,
            },
        );
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
        let _ = face.state.handle_input(
            InputEvent::LongPress,
            ScreenContent {
                cards: &cards,
                local_docs: None,
            },
        );
        face.render_cards(&cards, &[], &mut after);

        assert_ne!(before, after);
    }
}
