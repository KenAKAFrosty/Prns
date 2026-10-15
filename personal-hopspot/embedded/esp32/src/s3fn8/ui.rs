use personal_hopspot_core as hopspot;
use personal_rns::interfaces::subghz::SubGConfigurationState;
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot};

pub(crate) fn state(
    user_blanking: hopspot::UserBlanking,
    limits: personal_rns::storage::DisplayedStorageLimits,
) -> hopspot::UiState {
    hopspot::UiState::new(hopspot::UiConfiguration {
        storage_limits: limits,
        user_blanking,
        access_point: hopspot::AccessPointState::Unsupported,
        shared_instance_config_export: hopspot::SharedInstanceConfigExport::Unavailable,
        gnss: hopspot::GnssAvailability::Unavailable,
        #[cfg(feature = "remote-control-pairing")]
        remote_control_pairing: hopspot::RemoteControlPairingAvailability::Available,
        discovery_groups: hopspot::DiscoveryGroupEditorAvailability::Available,
    })
}

pub(crate) fn cards<const N: usize>(
    snapshots: &[InterfaceSnapshot],
    usb_id: InterfaceId,
    configuration: SubGConfigurationState,
) -> heapless::Vec<hopspot::Card, N> {
    hopspot::snapshots_to_cards(snapshots, |id| {
        if id == usb_id {
            return Some((hopspot::CardKind::Usb, hopspot::card_label("USB")));
        }
        match id.kind() {
            Some(InterfaceKind::LoRa) => Some(hopspot::subg_card(configuration)),
            Some(InterfaceKind::BluetoothAuto) => {
                Some((hopspot::CardKind::Ble, hopspot::card_label("BLE")))
            }
            _ => None,
        }
    })
}

#[cfg(test)]
const LONG_PRESS_MILLIS: u64 = 500;
#[cfg(test)]
const DEBOUNCE_MILLIS: u64 = 25;

/// Level sampler kept for the host tests. The V3 firmware waits on the pin instead.
#[cfg(test)]
struct Button {
    was_pressed: bool,
    started: Option<u64>,
}
#[cfg(test)]
impl Button {
    pub(crate) const fn new(initially_pressed: bool) -> Self {
        Self {
            was_pressed: initially_pressed,
            started: None,
        }
    }
    pub(crate) fn poll(
        &mut self,
        pressed: bool,
        now: u64,
        consume: bool,
    ) -> Option<hopspot::InputEvent> {
        let event = if pressed && !self.was_pressed {
            self.started = if consume { None } else { Some(now) };
            None
        } else if !pressed && self.was_pressed {
            self.started
                .take()
                .and_then(|started| match now.saturating_sub(started) {
                    held if held >= LONG_PRESS_MILLIS => Some(hopspot::InputEvent::LongPress),
                    held if held >= DEBOUNCE_MILLIS => Some(hopspot::InputEvent::ShortPress),
                    _ => None,
                })
        } else {
            None
        };
        self.was_pressed = pressed;
        event
    }
}

#[cfg(feature = "remote-control-pairing")]
pub(crate) fn confirmation_matches_press<Attempt: Copy + Eq>(
    attempt: Attempt,
    rendered: Option<Attempt>,
    pressed: Option<Attempt>,
) -> bool {
    rendered == Some(attempt) && pressed == Some(attempt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(kind: InterfaceKind) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id: InterfaceId::new([kind as u8, 0, 0, 0, 0, 0, 0, 0]),
            mode: personal_rns::interfaces::InterfaceMode::Full,
            gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
            connection: personal_rns::interfaces::ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 0,
            tx_bytes: 0,
            transfer_rates: None,
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: personal_rns::interfaces::Membership::Independent,
            radio: personal_rns::interfaces::RadioIndication::for_kind(Some(kind)),
            details: personal_rns::interfaces::PeerDetails::NotApplicable,
        }
    }
    #[test]
    fn v3_cards_keep_usb_lora_ble_and_route_selected_interface_action() {
        let usb_id = InterfaceId::new(*b"heltecv3");
        let mut usb = snapshot(InterfaceKind::UsbAutoDevice);
        usb.id = usb_id;
        let snapshots = [
            usb,
            snapshot(InterfaceKind::LoRa),
            snapshot(InterfaceKind::BluetoothAuto),
        ];
        let cards = cards::<8>(&snapshots, usb_id, SubGConfigurationState::Unconfigured);
        assert_eq!(cards.len(), 3);
        assert!(cards
            .iter()
            .any(|card| card.id() == usb_id && card.kind() == hopspot::CardKind::Usb));
        assert!(cards
            .iter()
            .any(|card| card.kind() == hopspot::CardKind::Ble));
        assert!(cards
            .iter()
            .any(|card| card.kind() == hopspot::CardKind::SubG(hopspot::SubGCardState::Setup)));
        let mut state = state(
            hopspot::UserBlanking::unavailable(),
            personal_rns::storage::DisplayedStorageLimits::DYNAMIC,
        );
        let content = hopspot::ScreenContent {
            cards: &cards,
            local_docs: None,
        };
        for _ in 0..cards.len() {
            state.handle_input(hopspot::InputEvent::ShortPress, content);
            if state
                .selected_card(&cards)
                .is_some_and(|card| card.id() == usb_id)
            {
                break;
            }
        }
        assert_eq!(
            state.selected_card(&cards).map(|card| card.id()),
            Some(usb_id)
        );
        assert_eq!(
            state.handle_input(hopspot::InputEvent::LongPress, content),
            hopspot::UiAction::None
        );
        assert_eq!(
            state.handle_input(hopspot::InputEvent::LongPress, content),
            hopspot::UiAction::ToggleSelectedInterface
        );
        assert_eq!(
            state.selected_card(&cards).map(|card| card.id()),
            Some(usb_id)
        );
    }
    #[test]
    fn boot_release_debounces_and_distinguishes_navigation_from_selection() {
        let mut button = Button::new(false);
        assert_eq!(button.poll(true, 0, false), None);
        assert_eq!(button.poll(false, 24, false), None);
        assert_eq!(button.poll(true, 100, false), None);
        assert_eq!(
            button.poll(false, 125, false),
            Some(hopspot::InputEvent::ShortPress)
        );
        assert_eq!(button.poll(true, 200, false), None);
        assert_eq!(button.poll(true, 700, false), None);
        assert_eq!(
            button.poll(false, 700, false),
            Some(hopspot::InputEvent::LongPress)
        );
        assert_eq!(button.poll(false, 701, false), None);
        let mut held_at_boot = Button::new(true);
        assert_eq!(held_at_boot.poll(false, 1000, false), None);
    }
    #[test]
    fn wake_press_is_consumed_even_when_held() {
        let mut button = Button::new(false);
        assert_eq!(button.poll(true, 0, true), None);
        assert_eq!(button.poll(false, 2000, true), None);
        assert_eq!(button.poll(true, 3000, false), None);
        assert_eq!(
            button.poll(false, 3500, false),
            Some(hopspot::InputEvent::LongPress)
        );
    }
    #[cfg(feature = "remote-control-pairing")]
    #[test]
    fn approval_requires_same_confirmation_visible_before_press_and_at_release() {
        assert!(!confirmation_matches_press(1u8, None, None));
        assert!(!confirmation_matches_press(1u8, Some(1), None));
        assert!(!confirmation_matches_press(1u8, None, Some(1)));
        assert!(!confirmation_matches_press(2u8, Some(2), Some(1)));
        assert!(!confirmation_matches_press(1u8, Some(2), Some(1)));
        assert!(confirmation_matches_press(1u8, Some(1), Some(1)));
    }
    #[test]
    fn global_menu_keeps_local_announce_and_pairing_as_distinct_actions() {
        let mut state = state(
            hopspot::UserBlanking::unavailable(),
            personal_rns::storage::DisplayedStorageLimits::DYNAMIC,
        );
        let content = hopspot::ScreenContent {
            cards: &[],
            local_docs: None,
        };
        assert_eq!(
            state.handle_input(hopspot::InputEvent::LongPress, content),
            hopspot::UiAction::None
        );
        assert_eq!(
            state.handle_input(hopspot::InputEvent::LongPress, content),
            hopspot::UiAction::Announce
        );
        state.handle_input(hopspot::InputEvent::LongPress, content);
        #[cfg(feature = "remote-control-pairing")]
        {
            state.handle_input(hopspot::InputEvent::ShortPress, content);
            assert_eq!(
                state.handle_input(hopspot::InputEvent::LongPress, content),
                hopspot::UiAction::OpenRemoteControlPairing
            );
        }
    }
}
