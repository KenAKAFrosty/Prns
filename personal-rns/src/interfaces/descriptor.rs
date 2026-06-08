use crate::interfaces::{
    ConnectionState, InterfaceCapabilities, InterfaceId, InterfaceMode, MediumKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub medium: MediumKind,
    pub state: ConnectionState,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
}

/// Per-interface announce rebroadcast rate policy — RNS 1.3.1's
/// `announce_rate_target`/`announce_rate_grace`/`announce_rate_penalty`
/// (seconds widened to milliseconds here). `None` leaves rate limiting off,
/// the reference default for an interface that never sets a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRateLimit {
    pub target_ms: u64,
    pub grace: u16,
    pub penalty_ms: u64,
}
