//! The "Personal Hopspot" status screen: portrait 64x128, drawn against any `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land on the S3's SSD1306 OLED and on the desktop simulator window.

mod limits;
mod model;
mod render;
mod state;

pub use model::{
    card_label, liveness_from_connection, push_interface_menu_info, push_named_peer_row,
    push_supervisor_peer_rows, sort_cards_for_display, tcp_card_label, BatteryState, Card,
    CardActivityTracker, CardKind, CardLabel, InterfaceMenuDetailKind, InterfaceMenuDetailRow,
    InterfaceMenuDetailRows, InterfaceMenuDetailText, Liveness, SupervisorPeerMenuStatus, UiFooter,
    CARD_LABEL_CAP, INTERFACE_MENU_DETAIL_ROWS_CAP, INTERFACE_MENU_DETAIL_TEXT_CAP,
};
pub use render::{
    draw, draw_at, draw_with_state, draw_with_state_at, draw_with_state_footer_at,
    draw_with_state_footer_details_at, splash,
};
pub use state::{
    AccessPointState, DisplayPowerControl, InputEvent, UiAction, UiConfiguration, UiNotice, UiState,
};

#[cfg(test)]
mod tests;
