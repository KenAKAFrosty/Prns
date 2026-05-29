//! Typed egress directives.
//!
//! `EgressDirective` is the engine's typed view of what it wants
//! emitted. `tick` (eventually) produces directives; some host-side
//! site pattern-matches and dispatches. Each variant carries the typed
//! values it operates on AND provides a `to_wire` method that handles
//! the serialization — the engine owns wire-protocol knowledge; the
//! host doesn't need to know how any packet kind frames onto the wire.
//!
//! `received_from` on `ReemitAnnounce` is provenance data (which
//! interface delivered the announce we're re-emitting), not a fanout
//! policy directive. Once engine-held-interfaces lands, directives
//! will gain explicit positive `fire_on: TargetSet` targets that the
//! engine computes; until then, hosts apply their own fanout policy
//! using `received_from` as input.
//!
//! This module ships the directive type, serialization, and tests
//! today. The `tick` refactor that actually produces directives lands
//! in a subsequent slice once we've picked a delivery shape
//! (callback / iterator / lent sink).

use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::wire::{
    Context, ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WirePacketHeader,
    HEADER_LEN,
};

#[derive(Debug)]
pub enum EgressSerializeError {
    BufferTooShort,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum EgressDirective<'a> {
    ReemitAnnounce {
        announce: Announce<'a>,
        emit_hops: u8,
        received_from: InterfaceId,
    },
}

impl<'a> EgressDirective<'a> {
    pub fn to_wire(&self, buf: &mut [u8]) -> Result<usize, EgressSerializeError> {
        match self {
            Self::ReemitAnnounce {
                announce,
                emit_hops,
                ..
            } => {
                let context_flag = if announce.maybe_ratchet.is_some() {
                    ContextFlag::Set
                } else {
                    ContextFlag::Unset
                };
                let header = WirePacketHeader {
                    ifac_flag: IfacFlag::Open,
                    context_flag,
                    propagation: PropagationType::Broadcast,
                    destination_type: DestinationType::Single,
                    packet_type: PacketType::Announce,
                    hops: *emit_hops,
                    transport_id: None,
                    destination: announce.destination,
                    context: Context::None,
                };
                let total_len = HEADER_LEN + announce.wire_len();
                if buf.len() < total_len {
                    return Err(EgressSerializeError::BufferTooShort);
                }
                header
                    .write(&mut buf[..HEADER_LEN])
                    .map_err(|_| EgressSerializeError::BufferTooShort)?;
                announce
                    .to_wire(&mut buf[HEADER_LEN..])
                    .map_err(|_| EgressSerializeError::BufferTooShort)?;
                Ok(total_len)
            }
        }
    }

    pub fn received_from(&self) -> Option<InterfaceId> {
        match self {
            Self::ReemitAnnounce { received_from, .. } => Some(*received_from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A genuine RNS 1.3.1 announce; same vector the other engine
    /// tests use.
    const RAW_ANNOUNCE: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                                59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                                0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                                7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                                4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    #[test]
    fn reemit_announce_to_wire_produces_a_well_formed_wire_packet() {
        // The classic round-trip: take a real RNS announce, build a
        // ReemitAnnounce out of it with hop count incremented, serialize,
        // and verify the bytes parse back to the expected shape.
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: orig_header.hops + 1,
            received_from: iface(0xAB),
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let wire = &buf[..n];

        let (parsed_header, parsed_payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(parsed_header.packet_type, PacketType::Announce);
        assert_eq!(parsed_header.destination_type, DestinationType::Single);
        assert_eq!(parsed_header.propagation, PropagationType::Broadcast);
        assert_eq!(parsed_header.transport_id, None);
        // Hops use the emit_hops we provided.
        assert_eq!(parsed_header.hops, orig_header.hops + 1);
        // Destination preserved.
        assert_eq!(parsed_header.destination, orig_header.destination);
        // Announce body bytes preserved — this is what keeps the
        // original Ed25519 signature valid on any peer.
        assert_eq!(parsed_payload, orig_payload);
    }

    #[test]
    fn to_wire_with_buffer_too_short_returns_buffer_too_short() {
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: 1,
            received_from: iface(0xAB),
        };

        let mut tiny_buf = [0u8; 8];
        assert!(matches!(
            directive.to_wire(&mut tiny_buf),
            Err(EgressSerializeError::BufferTooShort)
        ));
    }

    #[test]
    fn received_from_accessor_returns_the_stamped_source() {
        let raw = hx(RAW_ANNOUNCE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let announce = Announce::from_wire(&header, payload).unwrap();

        let directive = EgressDirective::ReemitAnnounce {
            announce,
            emit_hops: header.hops + 1,
            received_from: iface(0xCD),
        };

        assert_eq!(directive.received_from(), Some(iface(0xCD)));
    }

    #[test]
    fn to_wire_output_round_trips_to_an_equivalent_announce() {
        // Stronger property: serialize, re-parse, re-construct the
        // Announce — equality holds. Means we can chain emissions
        // through the directive type without semantic drift.
        let raw = hx(RAW_ANNOUNCE);
        let (orig_header, orig_payload) = WirePacketHeader::parse(&raw).unwrap();
        let orig_announce = Announce::from_wire(&orig_header, orig_payload).unwrap();

        let directive = EgressDirective::ReemitAnnounce {
            announce: orig_announce.clone(),
            emit_hops: 5,
            received_from: iface(0x42),
        };

        let mut buf = [0u8; 500];
        let n = directive.to_wire(&mut buf).unwrap();
        let (re_header, re_payload) = WirePacketHeader::parse(&buf[..n]).unwrap();
        let re_announce = Announce::from_wire(&re_header, re_payload).unwrap();

        assert_eq!(re_header.hops, 5);
        assert_eq!(re_announce, orig_announce);
    }
}
