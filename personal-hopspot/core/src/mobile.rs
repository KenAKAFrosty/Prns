use embedded_graphics::geometry::Point;

use crate::{face_64x128, InputEvent, UiAction};

pub const MOBILE_PANEL_WIDTH: usize = face_64x128::WIDTH as usize;
pub const MOBILE_PANEL_HEIGHT: usize = face_64x128::HEIGHT as usize;
pub const MOBILE_PIXEL_COUNT: usize = MOBILE_PANEL_WIDTH * MOBILE_PANEL_HEIGHT;
pub const MOBILE_RGBA_BYTES: usize = MOBILE_PIXEL_COUNT * 4;
pub const MOBILE_LIT_RGBA: [u8; 4] = [0x4a, 0x9e, 0xff, 0xff];
pub const MOBILE_DARK_RGBA: [u8; 4] = [0x00, 0x06, 0x1a, 0xff];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileInputCode {
    ShortPress = 0,
    LongPress = 1,
}

impl MobileInputCode {
    pub fn decode(code: i32) -> Result<InputEvent, InvalidMobileInputCode> {
        match code {
            value if value == Self::ShortPress as i32 => Ok(InputEvent::ShortPress),
            value if value == Self::LongPress as i32 => Ok(InputEvent::LongPress),
            code => Err(InvalidMobileInputCode { code }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMobileInputCode {
    code: i32,
}

impl InvalidMobileInputCode {
    #[must_use]
    pub const fn code(self) -> i32 {
        self.code
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileActionCode {
    None = 0,
    Announce = 1,
    CopySharedInstanceConfig = 2,
}

impl MobileActionCode {
    #[must_use]
    pub const fn encode(action: UiAction) -> Self {
        match action {
            UiAction::Announce => Self::Announce,
            UiAction::CopySharedInstanceConfig => Self::CopySharedInstanceConfig,
            UiAction::None
            | UiAction::BlankDisplay
            | UiAction::ToggleDisplayAutoOff
            | UiAction::Sleep
            | UiAction::Wake
            | UiAction::ControlGnss(_)
            | UiAction::ToggleSelectedInterface
            | UiAction::ToggleStationUplink
            | UiAction::OpenDocs
            | UiAction::OpenLoRaEditor
            | UiAction::SetLoRaProfile(_)
            | UiAction::ResetLoRaProfile
            | UiAction::OpenBleGroupEditor
            | UiAction::SetBleDiscoveryGroup(_)
            | UiAction::SwapRadioMode => Self::None,
        }
    }

    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileEngineState {
    Stopped = 0,
    Starting = 1,
    Running = 2,
    Failed = 3,
}

impl MobileEngineState {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileEngineFailure {
    None = 0,
    StorageConfiguration = 1,
    WorkerSpawn = 2,
    RuntimeBuild = 3,
    LocalListenerBind = 4,
    RpcListenerBind = 5,
    StartupTimeout = 6,
    WorkerStopped = 7,
    ShutdownTimeout = 8,
    PersistenceWrite = 9,
}

impl MobileEngineFailure {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::StorageConfiguration => "storage_configuration",
            Self::WorkerSpawn => "worker_spawn",
            Self::RuntimeBuild => "runtime_build",
            Self::LocalListenerBind => "local_listener_bind",
            Self::RpcListenerBind => "rpc_listener_bind",
            Self::StartupTimeout => "startup_timeout",
            Self::WorkerStopped => "worker_stopped",
            Self::ShutdownTimeout => "shutdown_timeout",
            Self::PersistenceWrite => "persistence_write",
        }
    }
}

pub fn expand_face_rgba(frame: &face_64x128::Frame, out: &mut [u8; MOBILE_RGBA_BYTES]) {
    for (index, chunk) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let point = Point::new(
            (index % MOBILE_PANEL_WIDTH) as i32,
            (index / MOBILE_PANEL_WIDTH) as i32,
        );
        chunk.copy_from_slice(if frame.pixel_is_on(point) {
            &MOBILE_LIT_RGBA
        } else {
            &MOBILE_DARK_RGBA
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::prelude::*;
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    #[test]
    fn input_codes_decode_only_the_closed_contract() {
        assert_eq!(
            MobileInputCode::decode(MobileInputCode::ShortPress as i32),
            Ok(InputEvent::ShortPress)
        );
        assert_eq!(
            MobileInputCode::decode(MobileInputCode::LongPress as i32),
            Ok(InputEvent::LongPress)
        );
        assert_eq!(
            MobileInputCode::decode(2),
            Err(InvalidMobileInputCode { code: 2 })
        );
    }

    #[test]
    fn actions_encode_to_the_closed_contract() {
        assert_eq!(
            MobileActionCode::encode(UiAction::None),
            MobileActionCode::None
        );
        assert_eq!(
            MobileActionCode::encode(UiAction::Announce),
            MobileActionCode::Announce
        );
        assert_eq!(
            MobileActionCode::encode(UiAction::CopySharedInstanceConfig),
            MobileActionCode::CopySharedInstanceConfig
        );
        assert_eq!(
            MobileActionCode::encode(UiAction::Sleep),
            MobileActionCode::None
        );
    }

    #[test]
    fn engine_state_and_failure_names_are_stable() {
        assert_eq!(MobileEngineState::Stopped.wire_name(), "stopped");
        assert_eq!(MobileEngineState::Starting.wire_name(), "starting");
        assert_eq!(MobileEngineState::Running.wire_name(), "running");
        assert_eq!(MobileEngineState::Failed.wire_name(), "failed");
        assert_eq!(MobileEngineFailure::None.wire_name(), "none");
        assert_eq!(
            MobileEngineFailure::StorageConfiguration.wire_name(),
            "storage_configuration"
        );
        assert_eq!(
            MobileEngineFailure::ShutdownTimeout.wire_name(),
            "shutdown_timeout"
        );
        assert_eq!(
            MobileEngineFailure::PersistenceWrite.wire_name(),
            "persistence_write"
        );
    }

    #[test]
    fn a_drawn_rectangle_lands_in_the_expanded_buffer() {
        let mut frame = face_64x128::Frame::new();
        Rectangle::new(Point::new(0, 0), Size::new(2, 2))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut frame)
            .unwrap();

        let mut out = [0u8; MOBILE_RGBA_BYTES];
        expand_face_rgba(&frame, &mut out);

        assert_eq!(&out[0..4], &MOBILE_LIT_RGBA);
        let below = (2 * MOBILE_PANEL_WIDTH) * 4;
        assert_eq!(&out[below..below + 4], &MOBILE_DARK_RGBA);
    }

    #[test]
    fn out_of_bounds_pixels_are_dropped() {
        let mut frame = face_64x128::Frame::new();
        frame
            .draw_iter([
                Pixel(Point::new(-1, -1), BinaryColor::On),
                Pixel(Point::new(MOBILE_PANEL_WIDTH as i32, 0), BinaryColor::On),
                Pixel(Point::new(0, MOBILE_PANEL_HEIGHT as i32), BinaryColor::On),
            ])
            .unwrap();

        let mut out = [0u8; MOBILE_RGBA_BYTES];
        expand_face_rgba(&frame, &mut out);
        assert!(out
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == MOBILE_DARK_RGBA));
    }
}
