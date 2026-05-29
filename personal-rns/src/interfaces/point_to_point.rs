use crate::interfaces::Interface;

/// Per-medium-kind sub-trait for **point-to-point** transports: one
/// interface instance speaks to one identified peer. TCP, USB CDC,
/// BLE GATT, USB serial, paired loopback. Pairs with
/// [`MediumKind::DirectPeer`](crate::interfaces::MediumKind::DirectPeer)
/// and
/// [`MediumKind::SwitchedNetwork`](crate::interfaces::MediumKind::SwitchedNetwork).
///
/// The trait trades only in raw Reticulum packet bytes; each
/// implementation is responsible for whatever transport-level framing
/// the wire requires (HDLC for TCP streams, COBS for serial,
/// length-prefix for length-aware media, raw frames for datagram
/// media). The engine does all Reticulum-layer parsing in `ingest`.
///
/// Calls are **non-blocking**: `try_read` must return immediately if
/// no packet is currently available. The host poll loop (or async
/// runtime) decides when to call again. This is a deliberate
/// improvement over RNS's thread-per-interface blocking-read model
/// (it lets a no_std embedded host drive every interface from one
/// event loop without a thread budget per transport).
pub trait PointToPointInterface: Interface {
    /// Errors this interface can surface from a read or a write.
    type Error;

    /// Pull at most one Reticulum packet from the transport into
    /// `buf`, returning the byte length written, or `None` if the
    /// transport is currently idle. `buf` should be at least the
    /// engine's MTU; a packet larger than `buf.len()` is an
    /// implementation-surfaced error rather than silently truncated.
    ///
    /// Must be non-blocking. A transport failure (peer closed, IO
    /// error) returns `Err`; the engine observes the corresponding
    /// lifecycle change via [`Interface::state`] separately.
    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    /// Push one Reticulum packet onto the transport. The
    /// implementation applies whatever transport-level framing it
    /// uses; the caller passes the raw Reticulum bytes only.
    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error>;
}
