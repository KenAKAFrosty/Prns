pub mod display;
pub mod face_64x128;
mod limits;
mod model;
mod notice;
mod render;
mod state;

pub(crate) use model::sort_cards_for_display;
pub use model::{
    card_label, subg_card, tcp_card_label, BluetoothRecoveryMenuDetails, Card, CardActivityTracker,
    CardKind, CardLabel, InterfaceMenuDetails, LoRaSpectrumMenuDetails, LocalDocsAccess,
    ScreenContent, SubGCardState, WifiNetworkStatus, WifiStationStatus,
};
pub use notice::PresentedNoticeTimer;
pub use render::cards::card_label_max_chars;
pub use state::{
    apply_and_persist_subg_configuration, AccessPointState, ActiveSubGConfiguration,
    GnssAvailability, InputEvent, PersistenceNotice, SharedInstanceConfigExport,
    SubGConfigurationChangeResult, SubGConfigurationPersistenceOutcome,
    SubGConfigurationStepOutcome, UiAction, UiConfiguration, UiNotice, UiState, UserBlanking,
};

#[cfg(test)]
mod tests;
