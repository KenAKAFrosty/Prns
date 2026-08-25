//! The "Personal Hopspot" status screen: portrait 64x128, drawn against any `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land on the S3's SSD1306 OLED and on the desktop simulator window.

mod eink;
mod geometry;
mod limits;
mod model;
mod power;
mod render;
mod state;

pub use eink::{EinkRefresh, EinkRefreshPolicy, EinkRefreshUrgency};
pub use geometry::{CanvasDimensions, LogicalPoint, QuarterTurn, RotatedCanvasMapping};
pub(crate) use model::sort_cards_for_display;
pub use model::{
    card_label, tcp_card_label, BluetoothRecoveryMenuDetails, Card, CardActivityTracker, CardKind,
    CardLabel, InterfaceMenuDetails, LoRaSpectrumMenuDetails, LocalDocsAccess, ScreenContent,
    WifiNetworkStatus, WifiStationStatus,
};
pub use power::{
    DisplayAutoOff, DisplayAutoOffDuration, DisplayButtonOutcome, DisplayDarkReason,
    DisplayPowerCommand, DisplayPowerState, DEFAULT_DISPLAY_AUTO_OFF,
};
pub use render::cards::card_label_max_chars;
pub use render::{render, splash, RenderFrame, SplashContent};
pub use state::{
    apply_and_persist_radio_profile, AccessPointState, DisplayPowerControl, GnssAvailability,
    InputEvent, PersistenceNotice, RadioProfileChangeResult, SharedInstanceConfigExport, UiAction,
    UiConfiguration, UiNotice, UiState,
};

#[cfg(test)]
mod tests;
