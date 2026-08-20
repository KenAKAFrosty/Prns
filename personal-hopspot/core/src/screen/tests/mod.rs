use core::convert::Infallible;

use embedded_graphics::mock_display::MockDisplay;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use heapless::Vec as HVec;
use personal_rns::interfaces::lora::{
    Frequency, ModemPreset, RadioProfile, Region, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

use crate::{
    BatteryPercent, ChargingState, ExternalPowerState, GnssReceiverCommand, GnssSnapshot,
    PersistenceState, PowerSnapshot,
};

use super::limits::{build_limit_rows, LimitRow, LimitValue};
use super::model::InterfaceMenuDetailKind;
use super::render::cards::{
    card_label_max_chars, connection_status_label, draw_card_with_selection,
};
use super::render::glyphs::{
    draw_battery, draw_clock, draw_interface_icon, draw_link, draw_person,
};
use super::render::layout::{
    ACTIVITY_TEXT_X, CARD_H, CARD_SLOT_STEP, CARD_TOP, FIRST_CARD_WITH_GLOBAL_TOP,
    FIRST_CARD_WITH_GNSS_TOP, FONT_4X6_CHAR_W, FONT_5X8_CHAR_W, FOOTER_FOURTH_LINE_OFFSET,
    FOOTER_SECOND_LINE_OFFSET, GLOBAL_BACKING_H, GLOBAL_BACKING_X, GLOBAL_BACKING_Y, GLOBAL_ICON_X,
    GLOBAL_ROW_H, GLOBAL_ROW_TOP, GNSS_PANEL_TOP, HEIGHT, MENU_BACKING_X, MENU_DIVIDER_Y,
    MENU_HEADER_Y, MENU_ITEM_STEP, MENU_ITEM_TOP, MENU_MARK_X, MENU_REASON_X, NAME_BACKING_X,
    NAME_BACKING_Y, NAME_ICON_X, NAME_LINE_Y, STAT_ICON_X, STAT_TEXT_X, WIDTH,
};
use super::render::menus::lora::{LORA_DOT_X, LORA_EDITOR_TOP};
use super::render::menus::{
    draw_interface_menu, limits_row_drawable, limits_row_text, menu_item_text_right,
    station_uplink_action_label,
};
use super::render::metrics::{
    compact_numeric_width, draw_compact_number, fmt_activity_age, fmt_bytes, fmt_count,
    fmt_rate_bytes_per_sec,
};
use super::state::lora::{
    region_index, step_custom_row, CustomRow, EditMode, FreqRow, LoRaScreen, PresetChoice,
    LORA_REGION_CANCEL, PRESET_CHOICES,
};
use super::state::{
    GlobalMenuItem, UiMode, ANNOUNCE_MENU_ITEM, LORA_RESET_MENU_ITEM, LORA_TUNE_MENU_ITEM,
    OLED_AUTO_OFF_MENU_ITEM, OLED_OFF_MENU_ITEM, POWER_MENU_ITEM, POWER_ONLY_MENU_ITEMS,
    RADIO_MENU_ITEM_NO_DISPLAY, SHARED_INSTANCE_CONFIG_MENU_ITEM, SLEEP_MENU_ITEM,
    STATION_UPLINK_MENU_ITEM, WIFI_MENU_ITEMS,
};
use super::{
    apply_and_persist_radio_profile, card_label, render as render_screen, sort_cards_for_display,
    AccessPointState, BluetoothRecoveryMenuDetails, Card, CardActivityTracker, CardKind,
    DisplayPowerControl, GnssAvailability, InputEvent, InterfaceMenuDetails,
    LoRaSpectrumMenuDetails, LocalDocsAccess, PersistenceNotice, RadioProfileChangeResult,
    RenderFrame, ScreenContent, SharedInstanceConfigExport, UiAction, UiConfiguration, UiNotice,
    UiState,
};

const TEST_WIDTH: usize = WIDTH as usize;
const TEST_HEIGHT: usize = HEIGHT as usize;

struct PanelDisplay {
    pixels: [[Option<BinaryColor>; TEST_WIDTH]; TEST_HEIGHT],
}

impl PanelDisplay {
    fn new() -> Self {
        Self {
            pixels: [[None; TEST_WIDTH]; TEST_HEIGHT],
        }
    }

    fn get_pixel(&self, point: Point) -> Option<BinaryColor> {
        if point.x < 0 || point.y < 0 || point.x >= WIDTH || point.y >= HEIGHT {
            return None;
        }
        self.pixels[point.y as usize][point.x as usize]
    }
}

impl DrawTarget for PanelDisplay {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 && point.x < WIDTH && point.y < HEIGHT {
                self.pixels[point.y as usize][point.x as usize] = Some(color);
            }
        }
        Ok(())
    }
}

impl OriginDimensions for PanelDisplay {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

fn render_with_state<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: PowerSnapshot,
    state: &UiState,
) {
    let interface_menu_details = InterfaceMenuDetails::empty();
    render_screen(
        display,
        RenderFrame {
            content: test_content(cards),
            battery,
            gnss: None,
            state,
            interface_menu_details: &interface_menu_details,
            animation_ms: 0,
        },
    );
}

fn render_with_local_docs<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: PowerSnapshot,
    state: &UiState,
    local_docs: &LocalDocsAccess<'_>,
) {
    let interface_menu_details = InterfaceMenuDetails::empty();
    render_screen(
        display,
        RenderFrame {
            content: ScreenContent {
                cards,
                local_docs: Some(local_docs),
            },
            battery,
            gnss: None,
            state,
            interface_menu_details: &interface_menu_details,
            animation_ms: 0,
        },
    );
}

fn test_card(label: &'static str) -> Card {
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

fn test_content(cards: &[Card]) -> ScreenContent<'_, 'static> {
    ScreenContent {
        cards,
        local_docs: None,
    }
}

fn test_ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        display_power_control: DisplayPowerControl::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_display_power() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        display_power_control: DisplayPowerControl::Available,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_access_point(access_point: AccessPointState) -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        display_power_control: DisplayPowerControl::Unavailable,
        access_point,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_shared_instance_config() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        display_power_control: DisplayPowerControl::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Available,
        gnss: super::GnssAvailability::Unavailable,
    })
}

fn test_ui_state_with_gnss() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        display_power_control: DisplayPowerControl::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: GnssAvailability::Available,
    })
}

fn has_on_pixel(
    display: &PanelDisplay,
    xs: core::ops::Range<i32>,
    ys: core::ops::Range<i32>,
) -> bool {
    for y in ys {
        for x in xs.clone() {
            if display.get_pixel(Point::new(x, y)) == Some(BinaryColor::On) {
                return true;
            }
        }
    }
    false
}

mod limits;
mod lora;
mod model;
mod render;
mod state;
