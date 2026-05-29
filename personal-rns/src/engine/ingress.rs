//! Typed classification of inbound packets.
//!
//! `Ingress` is the engine's typed view of what's on the wire. Bytes
//! arrive at the engine boundary as `InboundPacket`; the engine's
//! first move is `Ingress::classify` to turn them into a typed
//! variant. Decision sites then pattern-match, providing exhaustive
//! compile-time checks and no unnnecessary re-parsing
//!
//! Today only `Announce` carries fields; the other wire-kind variants
//! are bare discriminants documenting "the engine sees these packets
//! exist" without yet acting on them. As future slices land
//! (path-request handling, link establishment, data routing, proofs),
//! each gets a clear place to extend its variant with the fields it
//! needs.

use crate::engine::{InboundPacket, InstantMillis};
use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::wire::{DestinationType, PacketType, WirePacketHeader, MTU};

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Ingress<'a> {
    Announce {
        announce: Announce<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    /// Wire packet type `Data`. The engine doesn't yet distinguish
    /// sub-contexts (path requests, app data) or act on any of them.
    Data,

    /// Wire packet type `LinkRequest`. Engine sees, doesn't yet act.
    LinkRequest,

    /// Wire packet type `Proof`. Engine sees, doesn't yet act.
    Proof,

    /// Bytes didn't decode (truncated header, malformed wire layout,
    /// announce-signature failure, destination-binding failure, etc.).
    Unparseable,
}

impl<'a> Ingress<'a> {
    /// Classify one inbound packet. Cheap parse → typed variant; no
    /// engine state touched, no allocator.
    pub fn classify(packet: &InboundPacket<'a>) -> Self {
        let Ok((header, payload)) = WirePacketHeader::parse(packet.bytes) else {
            return Self::Unparseable;
        };

        match header.packet_type {
            PacketType::Announce => {
                if header.destination_type != DestinationType::Single {
                    return Self::Unparseable;
                }
                let Ok(announce) = Announce::from_wire(&header, payload) else {
                    return Self::Unparseable;
                };

                // Debug self-check: parse↔serialize round-trip on every
                // accepted announce. If `to_wire` ever drifts from
                // `from_wire`, the engine would silently re-emit a
                // signature-broken packet on rebroadcast. Cheap in
                // debug (one MTU-sized scratch + compare), zero in
                // release.
                debug_assert!(
                    {
                        let mut scratch = [0u8; MTU];
                        announce
                            .to_wire(&mut scratch)
                            .map(|n| &scratch[..n] == payload)
                            .unwrap_or(false)
                    },
                    "Announce::to_wire(from_wire(payload)) must equal payload"
                );

                Self::Announce {
                    announce,
                    received_hops: header.hops.saturating_add(1),
                    source_interface: packet.source_interface,
                    arrived_at: packet.arrived_at,
                }
            }
            PacketType::Data => Self::Data,
            PacketType::LinkRequest => Self::LinkRequest,
            PacketType::Proof => Self::Proof,
        }
    }
}
