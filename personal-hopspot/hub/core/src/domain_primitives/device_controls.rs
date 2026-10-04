use super::Connection;
use core::num::NonZeroU64;
use prns_core::remote_control::{
    RemoteControlDisplayVisibility, RemoteControlGnssPower, RemoteControlRequestKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceControlCommand {
    DisplayVisibility(RemoteControlDisplayVisibility),
    GnssPower(RemoteControlGnssPower),
}

impl DeviceControlCommand {
    pub const fn request_kind(self) -> RemoteControlRequestKind {
        match self {
            Self::DisplayVisibility(_) => RemoteControlRequestKind::SetDisplayVisibility,
            Self::GnssPower(_) => RemoteControlRequestKind::SetGnssPower,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceControlRequest {
    pub(crate) connection: Connection,
    pub(crate) generation: NonZeroU64,
    pub(crate) command: DeviceControlCommand,
}

impl DeviceControlRequest {
    pub const fn connection(self) -> Connection {
        self.connection
    }

    pub const fn command(self) -> DeviceControlCommand {
        self.command
    }
}
