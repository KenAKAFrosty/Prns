//! The canonical monochrome 64 by 128 Personal Hopspot face.

mod frame;
pub(in crate::screen) mod render;

pub use frame::{Frame, FRAME_BYTES, LOGICAL_HEIGHT, LOGICAL_WIDTH};
pub use render::SplashContent;

use super::ScreenRenderInput;

/// Render application state into the canonical logical frame.
pub fn render(frame: &mut Frame, input: ScreenRenderInput<'_, '_>) {
    render::render(frame, input);
}

/// Render startup content into the canonical logical frame.
pub fn splash(frame: &mut Frame, content: SplashContent) {
    render::splash(frame, content);
}

#[cfg(test)]
mod tests;
