//! The "Personal Hopspot" status screen: portrait 64x128, drawn against any `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land on the S3's SSD1306 OLED and on the desktop simulator window.

mod limits;
mod model;
mod render;
mod state;

pub use model::{
    card_label, tcp_card_label, Card, CardActivityTracker, CardKind, CardLabel,
    InterfaceMenuDetails, Liveness, LocalDocsAccess, WifiNetworkStatus,
};
pub(crate) use model::{liveness_from_connection, sort_cards_for_display};
pub use render::{render, splash, RenderFrame};
pub use state::{
    AccessPointState, DisplayPowerControl, InputEvent, UiAction, UiConfiguration, UiNotice, UiState,
};

#[cfg(test)]
mod tests;
