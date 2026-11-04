//! Opt-in metadata at the host egress admission boundary. Admission is not transmission.
use std::time::Instant;

use crate::interfaces::InterfaceId;
use crate::routing::links::{handshake::link_request_from, LinkId};
use crate::wire::{DestinationType, PacketType, WireContext, WirePacketHeader};

/// Synchronous, nonblocking sink. Return true only when this event was admitted.
/// Admission is not durable log persistence; exporters need their own loss accounting.
pub type EgressHandshakeObserver = fn(EgressHandshakeEvent) -> bool;
pub const EGRESS_HANDSHAKE_EVENT_LIMIT: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressHandshakePacket {
    Request,
    Proof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressHandshakePhase {
    Enqueued,
    Deferred,
    Unavailable,
    DroppedFull,
    LaneMissing,
    IfacRejected,
    Flushed,
    PendingDiscarded,
    /// At least one producer event was omitted. Later drops require a snapshot.
    LimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressHandshakeSnapshot {
    pub attempted: u64,
    /// Admitted data callbacks, excluding the separately reserved limit notice.
    pub emitted: u64,
    pub producer_dropped: u64,
    pub sink_dropped: u64,
    pub limit: u64,
}

impl Default for EgressHandshakeSnapshot {
    fn default() -> Self {
        Self {
            attempted: 0,
            emitted: 0,
            producer_dropped: 0,
            sink_dropped: 0,
            limit: EGRESS_HANDSHAKE_EVENT_LIMIT,
        }
    }
}

/// Ephemeral public Link IDs correlate request/proof across interfaces; sinks should hash them
/// before persistence and salt interface tags per process. No payloads, keys or destination IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressHandshakeEvent {
    pub sequence: u64,
    pub link_id: Option<LinkId>,
    pub interface: Option<InterfaceId>,
    pub packet: Option<EgressHandshakePacket>,
    pub phase: EgressHandshakePhase,
    pub queue_age_ms: Option<u64>,
    /// Exact counts before this callback; its admission or rejection is visible next time.
    pub snapshot: EgressHandshakeSnapshot,
}

#[derive(Clone, Copy)]
pub(super) struct HandshakePacket {
    pub(super) link_id: LinkId,
    pub(super) kind: EgressHandshakePacket,
}

#[derive(Clone, Copy)]
pub(super) struct PendingHandshake {
    pub(super) packet: HandshakePacket,
    queued_at: Instant,
}

impl PendingHandshake {
    pub(super) fn new(packet: HandshakePacket) -> Self {
        Self {
            packet,
            queued_at: Instant::now(),
        }
    }

    pub(super) fn age_ms(self) -> u64 {
        u64::try_from(self.queued_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[derive(Default)]
pub(super) struct HandshakeDiagnostics {
    observer: Option<EgressHandshakeObserver>,
    snapshot: EgressHandshakeSnapshot,
}

impl HandshakeDiagnostics {
    pub(super) fn set_observer(&mut self, observer: Option<EgressHandshakeObserver>) {
        self.observer = observer;
    }

    pub(super) fn snapshot(&self) -> EgressHandshakeSnapshot {
        self.snapshot
    }

    pub(super) fn classify(&self, bytes: &[u8]) -> Option<HandshakePacket> {
        self.observer?;
        let (header, body) = WirePacketHeader::parse(bytes).ok()?;
        match (header.packet_type, header.destination_type, header.context) {
            (PacketType::LinkRequest, DestinationType::Single, WireContext::None) => {
                Some(HandshakePacket {
                    link_id: link_request_from(&header, body).ok()?.link_id,
                    kind: EgressHandshakePacket::Request,
                })
            }
            (PacketType::Proof, DestinationType::Link, WireContext::LinkRequestProof) => {
                Some(HandshakePacket {
                    link_id: LinkId::from_address(header.address),
                    kind: EgressHandshakePacket::Proof,
                })
            }
            _ => None,
        }
    }

    pub(super) fn observe(
        &mut self,
        packet: Option<HandshakePacket>,
        interface: InterfaceId,
        phase: EgressHandshakePhase,
        queue_age_ms: Option<u64>,
    ) {
        let (Some(observer), Some(packet)) = (self.observer, packet) else {
            return;
        };
        self.snapshot.attempted = self.snapshot.attempted.saturating_add(1);
        let exhausted = self.snapshot.attempted > self.snapshot.limit;
        if exhausted {
            self.snapshot.producer_dropped = self.snapshot.producer_dropped.saturating_add(1);
            if self.snapshot.producer_dropped != 1 {
                return;
            }
        }
        let event = EgressHandshakeEvent {
            sequence: self.snapshot.attempted,
            link_id: (!exhausted).then_some(packet.link_id),
            interface: (!exhausted).then_some(interface),
            packet: (!exhausted).then_some(packet.kind),
            phase: if exhausted {
                EgressHandshakePhase::LimitReached
            } else {
                phase
            },
            queue_age_ms: if exhausted { None } else { queue_age_ms },
            snapshot: self.snapshot,
        };
        if observer(event) {
            if !exhausted {
                self.snapshot.emitted = self.snapshot.emitted.saturating_add(1);
            }
        } else {
            self.snapshot.sink_dropped = self.snapshot.sink_dropped.saturating_add(1);
        }
    }
}
