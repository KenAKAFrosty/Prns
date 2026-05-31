#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterfaceStats {
    //REVIEW we don't need to do it now, but this needs to change to a proper kind of enum
    pub online: bool,
    pub rx_packet_count: u32,
    pub tx_packet_count: u32,
    /// Peers the worker currently holds (medium-specific; 0 for point-to-point).
    /// REVIEW this very commment makes me think this should be an Option<u16> instead, yeah?
    pub active_peer_count: u16,
}
