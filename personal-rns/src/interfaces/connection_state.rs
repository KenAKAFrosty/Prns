/// Connection lifecycle state visible to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// Transport setup is in progress.
    Initializing,

    /// Healthy and routable.
    Connected,

    /// Still routable, but unhealthy enough that the engine may prefer alternatives.
    Degraded,

    /// Temporarily down while the host attempts recovery.
    Reconnecting,

    /// Down until the host explicitly retries or reconfigures it.
    Failed,

    /// Intentionally torn down and ready for removal from the engine.
    Disconnected,
}
