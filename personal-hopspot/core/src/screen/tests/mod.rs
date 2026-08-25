use embedded_graphics::mock_display::MockDisplay;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use heapless::Vec as HVec;
use personal_rns::interfaces::lora::{
    Frequency, ModemPreset, RadioProfile, Region, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

use crate::{GnssReceiverCommand, PersistenceState, PowerSnapshot};

use super::face_64x128::render::layout::{FONT_4X6_CHAR_W, FONT_5X8_CHAR_W, WIDTH};
use super::face_64x128::render::menus::lora::{LORA_DOT_X, LORA_EDITOR_TOP};
use super::face_64x128::render::menus::{limits_row_drawable, limits_row_text};
use super::limits::{build_limit_rows, LimitRow, LimitValue};
use super::model::InterfaceMenuDetailKind;
use super::state::lora::{
    region_index, step_custom_row, CustomRow, EditMode, FreqRow, LoRaScreen, PresetChoice,
    LORA_REGION_CANCEL, PRESET_CHOICES,
};
use super::state::{
    GlobalMenuItem, UiMode, ANNOUNCE_MENU_ITEM, DISPLAY_AUTO_OFF_MENU_ITEM, DISPLAY_OFF_MENU_ITEM,
    LORA_RESET_MENU_ITEM, LORA_TUNE_MENU_ITEM, POWER_MENU_ITEM, RADIO_MENU_ITEM_NO_DISPLAY,
    SHARED_INSTANCE_CONFIG_MENU_ITEM, SLEEP_MENU_ITEM, STATION_UPLINK_MENU_ITEM,
};
use super::{
    apply_and_persist_radio_profile, card_label, face_64x128, sort_cards_for_display,
    AccessPointState, BluetoothRecoveryMenuDetails, Card, CardActivityTracker, CardKind,
    GnssAvailability, InputEvent, InterfaceMenuDetails, LoRaSpectrumMenuDetails, PersistenceNotice,
    RadioProfileChangeResult, ScreenContent, ScreenRenderInput, SharedInstanceConfigExport,
    UiAction, UiConfiguration, UiNotice, UiState, UserBlanking,
};

pub(super) fn render_with_state<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: PowerSnapshot,
    state: &UiState,
) {
    let interface_menu_details = InterfaceMenuDetails::empty();
    face_64x128::render::render(
        display,
        ScreenRenderInput {
            content: test_content(cards),
            battery,
            gnss: None,
            state,
            interface_menu_details: &interface_menu_details,
            animation_ms: 0,
        },
    );
}

pub(super) fn test_card(label: &'static str) -> Card {
    Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Usb,
        label: card_label(label),
        connection: ConnectionState::Connected,
        failure_reason: None,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        peers: None,
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    }
}

fn test_cards<const N: usize>(kind: CardKind) -> [Card; N] {
    core::array::from_fn(|_| {
        let mut card = test_card("Test");
        card.kind = kind;
        card
    })
}

pub(super) fn test_content(cards: &[Card]) -> ScreenContent<'_, 'static> {
    ScreenContent {
        cards,
        local_docs: None,
    }
}

pub(super) fn test_ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_user_blanking() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::Available,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_access_point(access_point: AccessPointState) -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::Unavailable,
        access_point,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_shared_instance_config() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Available,
        gnss: super::GnssAvailability::Unavailable,
    })
}

pub(super) fn test_ui_state_with_gnss() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: GnssAvailability::Available,
    })
}

mod limits;
mod lora;
mod model;
mod state;
