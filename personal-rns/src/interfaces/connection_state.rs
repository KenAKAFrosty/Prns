#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    Unknown,
}

impl ConnectionState {
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            ConnectionState::Initializing => 0,
            ConnectionState::Connected => 1,
            ConnectionState::Degraded => 2,
            ConnectionState::Reconnecting => 3,
            ConnectionState::Failed => 4,
            ConnectionState::Disconnected => 5,
            ConnectionState::Unknown => 255,
        }
    }

    pub(crate) fn from_u8(code: u8) -> Self {
        match code {
            0 => ConnectionState::Initializing,
            1 => ConnectionState::Connected,
            2 => ConnectionState::Degraded,
            3 => ConnectionState::Reconnecting,
            4 => ConnectionState::Failed,
            5 => ConnectionState::Disconnected,
            _ => ConnectionState::Unknown,
        }
    }
}
