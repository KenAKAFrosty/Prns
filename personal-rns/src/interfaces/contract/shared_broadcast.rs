use super::Interface;

/// Semantic marker for **shared-broadcast** transports: every neighbor
/// on the medium hears every transmission. LoRa, plain ESP-NOW
/// broadcast, classic packet radio. Pairs with
/// [`MediumKind::SharedHalfDuplex`](crate::interfaces::MediumKind::SharedHalfDuplex)
/// and typically
/// [`Capabilities::repeats`](crate::interfaces::Capabilities::repeats)`=true`
/// (the medium IS the propagation mechanism — retransmission on the
/// source interface is correct, not gossip-back).
///
/// The universal byte I/O surface (`try_read`, `write`, `read_inbound`)
/// lives on the base [`Interface`] trait. This sub-trait declares
/// shared-broadcast semantics and is where shared-medium metadata, such
/// as reception quality or neighbor context, should land once needed.
///
/// **Self-echo:** on half-duplex media a sender may receive its own
/// outbound transmission back through `try_read`. Implementations
/// should suppress this where the hardware can (most LoRa radios mute
/// RX during TX); where they cannot, the engine's announce-id replay
/// defense catches it as a duplicate of an id we've already accepted,
/// so correctness is preserved, it just costs an ingest cycle.
///
/// Hosts declare the intent with a single empty impl:
///
/// ```ignore
/// impl Interface for MyLoRaInterface { /* … */ }
/// impl SharedBroadcastInterface for MyLoRaInterface {}
/// ```
pub trait SharedBroadcastInterface: Interface {}
