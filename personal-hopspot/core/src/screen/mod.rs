//! Display-independent Personal Hopspot UI state plus the canonical 64 by 128 face and its
//! presentation contracts.

mod blanking;
pub mod face_64x128;
mod limits;
mod model;
pub mod presentation;
mod render_input;
mod state;
mod transform;

pub use blanking::{
    DisplayAutoOff, DisplayBlankReason, DisplayBlankingAttempt, DisplayBlankingCommand,
    DisplayBlankingDecision, DisplayBlankingError, DisplayBlankingFeedback, DisplayBlankingResult,
    DisplayBlankingState, DisplayBufferKnowledge, DisplayButtonDecision, DisplayButtonOutcome,
    DisplayOperationOutcome, DisplayVisibility,
};
pub use face_64x128::render::cards::card_label_max_chars;
pub(crate) use model::sort_cards_for_display;
pub use model::{
    card_label, tcp_card_label, BluetoothRecoveryMenuDetails, Card, CardActivityTracker, CardKind,
    CardLabel, InterfaceMenuDetails, LoRaSpectrumMenuDetails, LocalDocsAccess, ScreenContent,
    WifiNetworkStatus, WifiStationStatus,
};
pub use render_input::ScreenRenderInput;
pub use state::{
    apply_and_persist_radio_profile, AccessPointState, GnssAvailability, InputEvent,
    PersistenceNotice, RadioProfileChangeResult, SharedInstanceConfigExport, UiAction,
    UiConfiguration, UiNotice, UiState, UserBlanking,
};
pub use transform::{
    LogicalPoint, LogicalSize, MappedPoint, PanelScale, PanelSize, PanelTransform, PanelViewport,
    PhysicalPoint, PointMapError, TransformError,
};

#[cfg(test)]
mod tests;
