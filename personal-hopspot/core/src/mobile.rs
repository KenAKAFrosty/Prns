use embedded_graphics::geometry::Point;
use personal_rns::interfaces::{
    DiscoveryGroupId, DiscoveryGroupSet, DiscoveryGroupSetError, MAX_DISCOVERY_GROUPS,
    MAX_DISCOVERY_GROUP_ID_LEN,
};

use crate::{face_64x128, InputEvent, UiAction};

pub const MOBILE_PANEL_WIDTH: usize = face_64x128::WIDTH as usize;
pub const MOBILE_PANEL_HEIGHT: usize = face_64x128::HEIGHT as usize;
pub const MOBILE_PIXEL_COUNT: usize = MOBILE_PANEL_WIDTH * MOBILE_PANEL_HEIGHT;
pub const MOBILE_RGBA_BYTES: usize = MOBILE_PIXEL_COUNT * 4;
pub const MOBILE_LIT_RGBA: [u8; 4] = [0x4a, 0x9e, 0xff, 0xff];
pub const MOBILE_DARK_RGBA: [u8; 4] = [0x00, 0x06, 0x1a, 0xff];
pub const MOBILE_DISCOVERY_GROUPS_WIRE_MAX_LEN: usize =
    1 + MAX_DISCOVERY_GROUPS * (1 + MAX_DISCOVERY_GROUP_ID_LEN);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileDiscoveryInterface {
    BluetoothAuto = 0,
    AutoWifi = 1,
}

impl MobileDiscoveryInterface {
    pub fn decode(code: i32) -> Result<Self, InvalidMobileDiscoveryInterface> {
        match code {
            value if value == Self::BluetoothAuto as i32 => Ok(Self::BluetoothAuto),
            value if value == Self::AutoWifi as i32 => Ok(Self::AutoWifi),
            code => Err(InvalidMobileDiscoveryInterface { code }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMobileDiscoveryInterface {
    code: i32,
}

impl InvalidMobileDiscoveryInterface {
    #[must_use]
    pub const fn code(self) -> i32 {
        self.code
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MobileDiscoveryGroupOutcome {
    Applied = 0,
    Unchanged = 1,
    EngineUnavailable = 2,
    Unsupported = 3,
    InvalidInterface = 4,
    InvalidEncoding = 5,
    BufferTooShort = 6,
    ApplyFailed = 7,
    Busy = 8,
}

impl MobileDiscoveryGroupOutcome {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn inventory_error_code(self) -> i32 {
        -1 - self.code()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileDiscoveryGroupsCodecError {
    Empty,
    TooMany,
    InvalidGroupId,
    Duplicate,
    Unsorted,
    Truncated,
    TrailingBytes,
    OutputTooShort,
}

pub fn encode_mobile_discovery_groups(
    groups: &DiscoveryGroupSet,
    out: &mut [u8],
) -> Result<usize, MobileDiscoveryGroupsCodecError> {
    let Some(count_out) = out.first_mut() else {
        return Err(MobileDiscoveryGroupsCodecError::OutputTooShort);
    };
    *count_out = groups.len() as u8;
    let mut offset = 1usize;
    for group in groups.iter() {
        let required = offset
            .saturating_add(1)
            .saturating_add(group.as_bytes().len());
        let Some(record) = out.get_mut(offset..required) else {
            return Err(MobileDiscoveryGroupsCodecError::OutputTooShort);
        };
        record[0] = group.as_bytes().len() as u8;
        record[1..].copy_from_slice(group.as_bytes());
        offset = required;
    }
    Ok(offset)
}

pub fn parse_mobile_discovery_groups(
    encoded: &[u8],
) -> Result<DiscoveryGroupSet, MobileDiscoveryGroupsCodecError> {
    let Some((&count, mut remaining)) = encoded.split_first() else {
        return Err(MobileDiscoveryGroupsCodecError::Truncated);
    };
    let count = usize::from(count);
    if count == 0 {
        return Err(MobileDiscoveryGroupsCodecError::Empty);
    }
    if count > MAX_DISCOVERY_GROUPS {
        return Err(MobileDiscoveryGroupsCodecError::TooMany);
    }
    let mut parsed: [Option<DiscoveryGroupId>; MAX_DISCOVERY_GROUPS] =
        core::array::from_fn(|_| None);
    for index in 0..count {
        let Some((&length, tail)) = remaining.split_first() else {
            return Err(MobileDiscoveryGroupsCodecError::Truncated);
        };
        let length = usize::from(length);
        let Some((group, tail)) = tail.split_at_checked(length) else {
            return Err(MobileDiscoveryGroupsCodecError::Truncated);
        };
        parsed[index] = Some(
            DiscoveryGroupId::from_bytes(group)
                .map_err(|_| MobileDiscoveryGroupsCodecError::InvalidGroupId)?,
        );
        if let (Some(previous), Some(current)) = (
            index
                .checked_sub(1)
                .and_then(|previous| parsed[previous].as_ref()),
            parsed[index].as_ref(),
        ) {
            match previous.cmp(current) {
                core::cmp::Ordering::Less => {}
                core::cmp::Ordering::Equal => {
                    return Err(MobileDiscoveryGroupsCodecError::Duplicate);
                }
                core::cmp::Ordering::Greater => {
                    return Err(MobileDiscoveryGroupsCodecError::Unsorted);
                }
            }
        }
        remaining = tail;
    }
    if !remaining.is_empty() {
        return Err(MobileDiscoveryGroupsCodecError::TrailingBytes);
    }
    let groups = parsed[..count]
        .iter()
        .filter_map(Option::as_ref)
        .cloned()
        .collect::<heapless::Vec<_, MAX_DISCOVERY_GROUPS>>();
    DiscoveryGroupSet::try_from_slice(groups.as_slice()).map_err(|error| match error {
        DiscoveryGroupSetError::Empty => MobileDiscoveryGroupsCodecError::Empty,
        DiscoveryGroupSetError::TooMany => MobileDiscoveryGroupsCodecError::TooMany,
        DiscoveryGroupSetError::Duplicate(_) => MobileDiscoveryGroupsCodecError::Duplicate,
    })
}

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
            | UiAction::OpenDiscoveryGroupsEditor(_)
            | UiAction::ReplaceDiscoveryGroups
            | UiAction::OpenDocs
            | UiAction::OpenSubGEditor
            | UiAction::SetSubGConfiguration(_)
            | UiAction::ClearSubGConfiguration
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

    #[test]
    fn discovery_group_wire_values_round_trip_canonically() {
        let groups = DiscoveryGroupSet::try_from_slice(&[
            DiscoveryGroupId::parse("zulu").unwrap(),
            DiscoveryGroupId::parse("alpha").unwrap(),
        ])
        .unwrap();
        let mut encoded = [0u8; MOBILE_DISCOVERY_GROUPS_WIRE_MAX_LEN];
        let len = encode_mobile_discovery_groups(&groups, &mut encoded).unwrap();
        assert_eq!(&encoded[..len], b"\x02\x05alpha\x04zulu");
        assert_eq!(parse_mobile_discovery_groups(&encoded[..len]), Ok(groups));
    }

    #[test]
    fn discovery_group_wire_values_reject_non_whole_inputs() {
        assert_eq!(
            parse_mobile_discovery_groups(&[]),
            Err(MobileDiscoveryGroupsCodecError::Truncated)
        );
        assert_eq!(
            parse_mobile_discovery_groups(b"\x00"),
            Err(MobileDiscoveryGroupsCodecError::Empty)
        );
        assert_eq!(
            parse_mobile_discovery_groups(b"\x01\x01a\x00"),
            Err(MobileDiscoveryGroupsCodecError::TrailingBytes)
        );
        assert_eq!(
            parse_mobile_discovery_groups(b"\x02\x01a\x01a"),
            Err(MobileDiscoveryGroupsCodecError::Duplicate)
        );
        assert_eq!(
            parse_mobile_discovery_groups(b"\x02\x01b\x01a"),
            Err(MobileDiscoveryGroupsCodecError::Unsorted)
        );
    }
}
