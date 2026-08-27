mod frame;
mod transform;

use crate::{GnssSnapshot, PowerSnapshot};

use super::{InterfaceMenuDetails, ScreenContent, UiState};

pub use frame::{Frame, FRAME_BYTES, HEIGHT, WIDTH};
pub use transform::{
    LogicalPoint, MappedPoint, PanelScale, PanelSize, PanelSizeError, PanelTransform,
    PanelViewport, PhysicalPoint, PointMapError, QuarterTurn, TransformError,
};

pub struct RenderInput<'frame, 'docs> {
    pub content: ScreenContent<'frame, 'docs>,
    pub battery: PowerSnapshot,
    pub gnss: Option<GnssSnapshot>,
    pub state: &'frame UiState,
    pub interface_menu_details: &'frame InterfaceMenuDetails,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplashContent {
    Brand,
    Starting,
    Connecting,
}

impl SplashContent {
    #[cfg(test)]
    pub(in crate::screen) const ALL: [Self; 3] = [Self::Brand, Self::Starting, Self::Connecting];

    pub(in crate::screen) fn lines(self) -> &'static [&'static str] {
        match self {
            Self::Brand => &["Personal", "Hopspot"],
            Self::Starting => &["starting"],
            Self::Connecting => &["connecting"],
        }
    }
}

pub fn render(frame: &mut Frame, input: RenderInput<'_, '_>) {
    super::render::draw(frame, input);
}

pub fn splash(frame: &mut Frame, content: SplashContent) {
    super::render::draw_splash(frame, content);
}
