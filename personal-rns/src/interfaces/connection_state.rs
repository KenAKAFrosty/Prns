#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Initializing,

    Connected,

    Degraded,

    Reconnecting,

    Failed,

    Disconnected,
}
