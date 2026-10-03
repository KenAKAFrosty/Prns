use core::num::NonZeroU64;
use prns_core::remote_control::{RemoteControlInterfacePage, RemoteControlRequest};

use super::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfacePageRequest {
    pub(crate) connection: Connection,
    pub(crate) generation: NonZeroU64,
    pub(crate) page: RemoteControlInterfacePage,
}

impl InterfacePageRequest {
    pub const fn connection(self) -> Connection {
        self.connection
    }

    pub const fn request(self) -> RemoteControlRequest {
        RemoteControlRequest::InventoryInterfaces { page: self.page }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceRefreshFailure {
    TimedOut,
    TransportLost,
    PermissionDenied,
    InvalidResponse,
    OutOfOrder,
    CapacityExceeded { maximum: usize },
}
