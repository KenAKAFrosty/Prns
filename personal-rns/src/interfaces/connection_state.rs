#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    /// Turned off from the application (a Hopspot "Turn Off"): the interface keeps its reserved
    /// slot and its learned routes but its driver has gone dormant — wire closed, nothing
    /// ingested, egress drained and discarded — until it is turned back on. Distinct from `Failed`
    /// (which is involuntary) and `Disconnected` (up but link-less): this is a deliberate, instantly
    /// reversible off.
    Disabled,
    Unknown,
}

#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
impl ConnectionState {
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
