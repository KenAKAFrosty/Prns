use crate::engine::{InboundPacket, InstantMillis};
use crate::interfaces::{Capabilities, ConnectionState, InterfaceId, InterfaceMode, MediumKind};

/// Base contract every transport interface honors. Exposes the declared
/// shape (id, capabilities, mode, medium), the observable lifecycle
/// state, and the universal byte I/O surface every transport
/// accommodates (`try_read`, `write`). Semantic markers like
/// [`PointToPointInterface`](crate::interfaces::PointToPointInterface)
/// and
/// [`SharedBroadcastInterface`](crate::interfaces::SharedBroadcastInterface)
/// extend this trait to declare medium-specific intent without adding
/// API today. Add methods there once a medium needs behavior the base
/// interface cannot honestly express.
///
/// Hosts implement this on concrete interface types (TCP, LoRa, BLE,
/// loopback, sim, …); the engine consumes the trait via dyn dispatch
/// to make per-interface routing and fanout decisions. Sims implement
/// the same trait, so the engine's behavior under sim and under real
/// hardware exercises one surface.
pub trait Interface {
    /// Errors this interface can surface from a read or a write.
    type Error;

    /// Stable identity for this interface.
    fn id(&self) -> InterfaceId;

    /// Declared capability set: what the host says this interface
    /// can and will do.
    fn capabilities(&self) -> Capabilities;

    /// Operational role the engine treats this interface in.
    fn mode(&self) -> InterfaceMode;

    /// Classification of the underlying physical or virtual medium.
    fn medium_kind(&self) -> MediumKind;

    /// Current lifecycle state.
    fn state(&self) -> ConnectionState;

    /// Identity of the parent interface, if this one was spawned by a
    /// server-style interface (e.g., a TCP-client interface spawned by
    /// a TCP-server on each accepted connection, or an auto-interface
    /// peer spawned by an auto-interface on discovery). `None` for
    /// top-level interfaces.
    ///
    /// Mirrors RNS's `parent_interface` / `spawned_interfaces`
    /// relationship: see
    /// [`Interface.received_announce`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L259-L267)
    /// for how RNS propagates per-interface stats up the chain.
    fn parent_interface(&self) -> Option<InterfaceId> {
        None
    }

    /// Pull at most one Reticulum packet from the transport into `buf`,
    /// returning the byte length written, or `None` if the transport
    /// is currently idle. `buf` should be at least the engine's MTU;
    /// a packet larger than `buf.len()` is an implementation-surfaced
    /// error rather than silently truncated.
    ///
    /// Must be non-blocking. A transport failure (peer closed, radio
    /// off, IO error) returns `Err`; the engine observes the
    /// corresponding lifecycle change via [`Interface::state`]
    /// separately.
    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    /// Push one Reticulum packet to the transport. The implementation
    /// applies whatever transport-level framing it uses (RNS serial
    /// framing for reference-compatible serial byte streams, raw
    /// frames for datagram media, a host-specific wrapper where
    /// appropriate, …); the caller passes the raw Reticulum bytes
    /// only.
    ///
    /// The semantic of "push" depends on the medium: a point-to-point
    /// interface delivers to one peer, a shared-broadcast interface
    /// emits to every neighbor on the medium, etc. The semantic trait
    /// markers (`PointToPointInterface`, `SharedBroadcastInterface`)
    /// document each case.
    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error>;

    /// Drain one Reticulum packet via [`try_read`] and wrap it as an
    /// [`InboundPacket`] ready for `engine::ingest`. Stamps
    /// `arrived_at` from the caller (so multiple interfaces can share
    /// a single host-supplied "now") and `source_interface` from
    /// [`Interface::id`] so the engine's "don't gossip back to source"
    /// rule has its tag without the caller plumbing it manually.
    ///
    /// Default implementation; an interface that can produce the
    /// `InboundPacket` more efficiently (e.g., zero-copy from its own
    /// pre-parsed scratch) may override.
    fn read_inbound<'a>(
        &mut self,
        buf: &'a mut [u8],
        arrived_at: InstantMillis,
    ) -> Result<Option<InboundPacket<'a>>, Self::Error> {
        let Some(n) = self.try_read(buf)? else {
            return Ok(None);
        };
        Ok(Some(InboundPacket {
            arrived_at,
            source_interface: self.id(),
            bytes: &buf[..n],
        }))
    }
}
