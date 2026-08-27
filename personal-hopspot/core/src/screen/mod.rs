pub mod display;
mod eink;
pub mod face_64x128;
mod geometry;
mod limits;
mod model;
mod power;
mod render;
mod state;

pub use eink::{EinkRefresh, EinkRefreshPolicy, EinkRefreshUrgency};
#[doc(hidden)]
pub use face_64x128::SplashContent;
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
#[doc(hidden)]
pub type RenderFrame<'frame, 'docs> = face_64x128::RenderInput<'frame, 'docs>;
#[doc(hidden)]
pub fn render<D>(display: &mut D, input: RenderFrame<'_, '_>)
where
    D: embedded_graphics::prelude::DrawTarget<Color = embedded_graphics::pixelcolor::BinaryColor>,
{
    render::draw(display, input);
}
#[doc(hidden)]
pub fn splash<D>(display: &mut D, content: SplashContent)
where
    D: embedded_graphics::prelude::DrawTarget<Color = embedded_graphics::pixelcolor::BinaryColor>,
{
    render::draw_splash(display, content);
}
pub use state::{
    apply_and_persist_radio_profile, AccessPointState, DisplayPowerControl, GnssAvailability,
    InputEvent, PersistenceNotice, RadioProfileChangeResult, SharedInstanceConfigExport, UiAction,
    UiConfiguration, UiNotice, UiState, UserBlanking,
};

#[cfg(test)]
mod tests;
