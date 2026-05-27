//! Host seam: the platform body the pure engine runs on.
//!
//! The engine is platform-agnostic. Each target (daemon, microcontroller, SDK)
//! supplies a `Host` providing the clock, the inbound queue, and outbound
//! transmission. This trait is the complete inventory of what the stack asks of
//! the world; keep it small.

use crate::engine::{InboundPacket, InstantMillis, OutboundPacket};

pub trait Host {
    type Error;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error>;

    /// Drain a batch of queued inbound packets, each stamped with its arrival
    /// instant. The host owns the backing storage and lends the batch for one
    /// `ingest`. Draining need not be exhaustive: the host may cap the batch so
    /// a burst can't make one `step` do unbounded work — the remainder waits for
    /// the next call. An empty slice means nothing is queued.
    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error>;

    /// Surface a batch of packets to the host for transmission. The host sends them over
    /// whatever transport it owns; the engine never touches the wire. An empty
    /// batch is a no-op.
    fn pump_outbound_packets(&mut self, packets: &[OutboundPacket<'_>]) -> Result<(), Self::Error>;
}
