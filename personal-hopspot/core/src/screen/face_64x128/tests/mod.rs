use core::convert::Infallible;

use embedded_graphics::mock_display::MockDisplay;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use heapless::Vec as HVec;
use personal_rns::interfaces::{ConnectionState, InterfaceId};

use crate::{BatteryPercent, ChargingState, ExternalPowerState, GnssSnapshot, PowerSnapshot};

use super::render::cards::{
    card_label_max_chars, connection_status_label, draw_card_with_selection,
};
use super::render::glyphs::{
    draw_battery, draw_clock, draw_interface_icon, draw_link, draw_person,
};
use super::render::layout::{
    ACTIVITY_TEXT_X, CARD_H, CARD_SLOT_STEP, CARD_TOP, FIRST_CARD_WITH_GLOBAL_TOP,
    FIRST_CARD_WITH_GNSS_TOP, FOOTER_FOURTH_LINE_OFFSET, FOOTER_SECOND_LINE_OFFSET,
    GLOBAL_BACKING_H, GLOBAL_BACKING_X, GLOBAL_BACKING_Y, GLOBAL_ICON_X, GLOBAL_ROW_H,
    GLOBAL_ROW_TOP, GNSS_PANEL_TOP, HEIGHT, MENU_BACKING_X, MENU_DIVIDER_Y, MENU_HEADER_Y,
    MENU_ITEM_STEP, MENU_ITEM_TOP, MENU_MARK_X, MENU_REASON_X, NAME_BACKING_X, NAME_BACKING_Y,
    NAME_ICON_X, NAME_LINE_Y, STAT_ICON_X, STAT_TEXT_X, WIDTH,
};
use super::render::menus::{
    draw_interface_menu, menu_item_text_right, station_uplink_action_label,
};
use super::render::metrics::{
    compact_numeric_width, draw_compact_number, fmt_activity_age, fmt_bytes, fmt_count,
    fmt_rate_bytes_per_sec,
};
use crate::screen::state::{POWER_MENU_ITEM, POWER_ONLY_MENU_ITEMS, WIFI_MENU_ITEMS};
use crate::screen::tests::{
    render_with_state, test_card, test_content, test_ui_state, test_ui_state_with_gnss,
};
use crate::screen::{
    card_label, face_64x128, Card, CardKind, InputEvent, InterfaceMenuDetails, LocalDocsAccess,
    ScreenContent, ScreenRenderInput, SharedInstanceConfigExport, UiAction, UiState,
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

fn render_with_local_docs<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: PowerSnapshot,
    state: &UiState,
    local_docs: &LocalDocsAccess<'_>,
) {
    let interface_menu_details = InterfaceMenuDetails::empty();
    face_64x128::render::render(
        display,
        ScreenRenderInput {
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

mod cards;
mod flow;
mod glyphs;
mod metrics;
mod splash;
