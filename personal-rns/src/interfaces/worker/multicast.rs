use super::InterfaceWorker;

/// A multicast interface worker that tracks its current group membership.
///
/// Extends [`InterfaceWorker`]: only a worker on a peer-tracking multicast
/// medium (the WiFi auto-interface is the archetype) implements it, so the live
/// peer count lives here rather than as a field every interface's
/// [`InterfaceStats`](super::InterfaceStats) would have to carry and default.
pub trait TrackedPeerMulticastInterface: InterfaceWorker {
    /// How many peers the worker currently holds in its group.
    fn active_peer_count(&self) -> u16;
}
