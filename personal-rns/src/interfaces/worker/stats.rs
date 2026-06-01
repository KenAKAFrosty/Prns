/// The live state of an interface's link, as its worker observes it — distinct
/// from the descriptor's [`ConnectionState`](crate::interfaces::ConnectionState),
/// which is the engine-facing routing lifecycle, not the moment-to-moment link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    /// No usable link right now (the default for a freshly-built interface).
    #[default]
    Down,
    /// The link is up and able to carry traffic.
    Up,
}

impl LinkState {
    pub const fn from_up(is_up: bool) -> Self {
        if is_up {
            Self::Up
        } else {
            Self::Down
        }
    }

    pub const fn is_up(self) -> bool {
        matches!(self, Self::Up)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterfaceStats {
    pub link: LinkState,
    pub rx_packet_count: u32,
    pub tx_packet_count: u32,
    /// Peers the worker currently holds, when that is a meaningful concept for
    /// the medium: `None` for a point-to-point link (no peer-count there),
    /// `Some(n)` for a multi-peer medium like the WiFi auto-interface.
    pub active_peer_count: Option<u16>,
}
