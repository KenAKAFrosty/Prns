//! Semantic input to the current face renderer.

use crate::{GnssSnapshot, PowerSnapshot};

use super::{InterfaceMenuDetails, ScreenContent, UiState};

/// Display-independent application and UI state consumed by the current face layout.
pub struct ScreenRenderInput<'frame, 'docs> {
    pub content: ScreenContent<'frame, 'docs>,
    pub battery: PowerSnapshot,
    pub gnss: Option<GnssSnapshot>,
    pub state: &'frame UiState,
    pub interface_menu_details: &'frame InterfaceMenuDetails,
    pub animation_ms: u64,
}
