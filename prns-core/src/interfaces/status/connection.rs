#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    /// A deliberate, reversible off state that retains the interface slot and learned routes while closing the wire and discarding egress.
    Disabled,
    Unknown,
}

impl ConnectionState {
    pub const fn is_online(self) -> bool {
        matches!(self, Self::Connected | Self::Degraded)
    }

    pub const fn as_u8(self) -> u8 {
        match self {
            ConnectionState::Initializing => 0,
            ConnectionState::Connected => 1,
            ConnectionState::Degraded => 2,
            ConnectionState::Reconnecting => 3,
            ConnectionState::Failed => 4,
            ConnectionState::Disconnected => 5,
            ConnectionState::Disabled => 6,
            ConnectionState::Unknown => 255,
        }
    }

    pub fn from_u8(code: u8) -> Self {
        match code {
            0 => ConnectionState::Initializing,
            1 => ConnectionState::Connected,
            2 => ConnectionState::Degraded,
            3 => ConnectionState::Reconnecting,
            4 => ConnectionState::Failed,
            5 => ConnectionState::Disconnected,
            6 => ConnectionState::Disabled,
            _ => ConnectionState::Unknown,
        }
    }
}
