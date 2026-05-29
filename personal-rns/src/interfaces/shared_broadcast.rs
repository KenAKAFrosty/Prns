use crate::interfaces::Interface;

/// Per-medium-kind sub-trait for **shared-broadcast** transports:
/// every neighbor on the medium hears every transmission. LoRa, plain
/// ESP-NOW broadcast, classic packet radio. Pairs with
/// [`MediumKind::SharedHalfDuplex`](crate::interfaces::MediumKind::SharedHalfDuplex)
/// and typically
/// [`Capabilities::repeats`](crate::interfaces::Capabilities::repeats)`=true`
/// (the medium IS the propagation mechanism — retransmission on the
/// source interface is correct, not gossip-back).
///
/// The trait trades only in raw Reticulum packet bytes; each
/// implementation handles its own framing. The engine does all
/// Reticulum-layer parsing in `ingest`.
///
/// Like
/// [`PointToPointInterface`](crate::interfaces::PointToPointInterface),
/// calls are non-blocking — the same single-event-loop story.
///
/// **Self-echo:** on half-duplex media a sender may receive its own
/// outbound transmission back through `try_read`. Implementations
/// should suppress this where the hardware can (most LoRa radios mute
/// RX during TX); where they cannot, the engine's announce-id replay
/// defense catches it as a duplicate of an id we've already accepted,
/// so correctness is preserved, it just costs an ingest cycle.
pub trait SharedBroadcastInterface: Interface {
    /// Errors this interface can surface from a read or a write.
    type Error;

    /// Pull at most one Reticulum packet from the medium into `buf`,
    /// returning the byte length written, or `None` if the medium is
    /// currently idle. `buf` should be at least the engine's MTU.
    ///
    /// Must be non-blocking. A transport failure (radio off, hardware
    /// fault) returns `Err`; lifecycle changes flow through
    /// [`Interface::state`] separately.
    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    /// Broadcast one Reticulum packet on the medium. Every neighbor in
    /// range hears it; on half-duplex media the sender may also
    /// receive its own echo on a subsequent `try_read` (see the
    /// type-level docstring).
    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error>;
}
