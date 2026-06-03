//! Typed classification of inbound packets.
//!
//! `Ingress` is the engine's typed view of what's on the wire. Bytes
//! arrive at the engine boundary as `InboundPacket`; the engine's
//! first move is `Ingress::classify` to turn them into a typed
//! variant. Decision sites then pattern-match, providing exhaustive
//! compile-time checks and no unnecessary re-parsing.
//!
//! Today only `Announce` carries fields; the other wire-kind variants are bare
//! discriminants documenting packet types the engine recognizes but does not yet
//! handle.

use crate::engine::InstantMillis;
use crate::interfaces::{InboundPacket, InterfaceId};
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

    /// Wire packet type `LinkRequest`. Engine recognizes, doesn't yet act.
    LinkRequest,

    /// Wire packet type `Proof`. Engine recognizes, doesn't yet act.
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

        let received_hops = header.hops.saturating_add(1); //IRC this is *general, broad packet behavior*, e.g., Link will probably want this too.

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
                    received_hops,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        Context, ContextFlag, DestinationHash, IfacFlag, PropagationType, WirePacketHeader,
        HEADER_LEN,
    };

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

    fn header_bytes(packet_type: PacketType) -> [u8; HEADER_LEN] {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type,
            hops: 0,
            transport_id: None,
            destination: DestinationHash::new([0xA5; 16]),
            context: Context::None,
        };
        let mut bytes = [0u8; HEADER_LEN];
        assert_eq!(header.write(&mut bytes).unwrap(), HEADER_LEN);
        bytes
    }

    #[test]
    fn malformed_headers_are_unparseable() {
        let packet = InboundPacket {
            arrived_at: InstantMillis(7),
            source_interface: iface(0x01),
            bytes: &[0x01],
        };

        assert!(matches!(Ingress::classify(&packet), Ingress::Unparseable));
    }

    #[test]
    fn recognized_non_announce_packets_classify_from_the_header() {
        for packet_type in [PacketType::Data, PacketType::LinkRequest, PacketType::Proof] {
            let bytes = header_bytes(packet_type);
            let packet = InboundPacket {
                arrived_at: InstantMillis(9),
                source_interface: iface(0x02),
                bytes: &bytes,
            };

            let classified = Ingress::classify(&packet);
            match packet_type {
                PacketType::Data => assert!(matches!(classified, Ingress::Data)),
                PacketType::LinkRequest => assert!(matches!(classified, Ingress::LinkRequest)),
                PacketType::Proof => assert!(matches!(classified, Ingress::Proof)),
                PacketType::Announce => unreachable!(),
            }
        }
    }

    #[test]
    fn announce_packets_must_target_a_single_destination() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[0] |= (DestinationType::Group as u8) << 2;
        let packet = InboundPacket {
            arrived_at: InstantMillis(11),
            source_interface: iface(0x03),
            bytes: &raw,
        };

        assert!(matches!(Ingress::classify(&packet), Ingress::Unparseable));
    }

    #[test]
    fn announce_received_hops_saturates_at_wire_max() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[1] = u8::MAX;
        let source_interface = iface(0x04);
        let arrived_at = InstantMillis(13);
        let packet = InboundPacket {
            arrived_at,
            source_interface,
            bytes: &raw,
        };

        let classified = Ingress::classify(&packet);
        let Ingress::Announce {
            received_hops,
            source_interface: classified_source,
            arrived_at: classified_arrival,
            ..
        } = classified
        else {
            panic!("valid announce should classify as announce");
        };
        assert_eq!(received_hops, u8::MAX);
        assert_eq!(classified_source, source_interface);
        assert_eq!(classified_arrival, arrived_at);
    }
}
