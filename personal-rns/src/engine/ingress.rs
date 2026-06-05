use crate::engine::InstantMillis;
use crate::interfaces::{InboundPacket, InterfaceId};
use crate::routing::announce::Announce;
use crate::wire::{
    DestinationHash, DestinationType, PacketType, TransportId, WireContext, WirePacketHeader, MTU,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DataPacket<'a> {
    pub destination_type: DestinationType,
    pub destination: DestinationHash,
    pub context: WireContext,
    pub maybe_transport_id: Option<TransportId>,
    pub payload: &'a mut [u8],
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Ingress<'a> {
    Announce {
        announce: Announce<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    Data {
        data: DataPacket<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    LinkRequest,

    Proof,

    Unparseable,
}

impl<'a> Ingress<'a> {
    pub fn classify(packet: InboundPacket<'a>) -> Self {
        let InboundPacket {
            arrived_at,
            source_interface,
            bytes,
        } = packet;
        let (header, payload_offset) = match WirePacketHeader::parse(bytes) {
            Ok((header, payload)) => (header, bytes.len() - payload.len()),
            Err(_) => return Self::Unparseable,
        };
        let (_, payload) = bytes.split_at_mut(payload_offset);

        let received_hops = header.hops.saturating_add(1);

        match header.packet_type {
            PacketType::Announce => {
                if header.destination_type != DestinationType::Single {
                    return Self::Unparseable;
                }

                //erase mutable since it's not needed in this arm
                let payload: &'a [u8] = payload;
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
                    source_interface,
                    arrived_at,
                }
            }
            PacketType::Data => Self::Data {
                data: DataPacket {
                    destination_type: header.destination_type,
                    destination: header.destination,
                    context: header.context,
                    maybe_transport_id: header.transport_id,
                    payload,
                },
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::LinkRequest => Self::LinkRequest,
            PacketType::Proof => Self::Proof,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{
        ContextFlag, DestinationHash, IfacFlag, PropagationType, TransportId, WireContext,
        WirePacketHeader, HEADER_LEN,
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
            context: WireContext::None,
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
            bytes: &mut [0x01],
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
    }

    #[test]
    fn recognized_non_announce_packets_classify_from_the_header() {
        for packet_type in [PacketType::Data, PacketType::LinkRequest, PacketType::Proof] {
            let mut bytes = header_bytes(packet_type);
            let packet = InboundPacket {
                arrived_at: InstantMillis(9),
                source_interface: iface(0x02),
                bytes: &mut bytes,
            };

            let classified = Ingress::classify(packet);
            match packet_type {
                PacketType::Data => assert!(matches!(classified, Ingress::Data { .. })),
                PacketType::LinkRequest => assert!(matches!(classified, Ingress::LinkRequest)),
                PacketType::Proof => assert!(matches!(classified, Ingress::Proof)),
                PacketType::Announce => unreachable!(),
            }
        }
    }

    #[test]
    fn data_packets_carry_their_typed_fields_through_classification() {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Transport,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 5,
            transport_id: Some(TransportId::new([0x11; 16])),
            destination: DestinationHash::new([0xA5; 16]),
            context: WireContext::Resource,
        };
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut expected_payload = payload;
        let mut bytes = [0u8; MTU];
        let header_len = header.write(&mut bytes).unwrap();
        bytes[header_len..header_len + payload.len()].copy_from_slice(&payload);

        let packet = InboundPacket {
            arrived_at: InstantMillis(21),
            source_interface: iface(0x05),
            bytes: &mut bytes[..header_len + payload.len()],
        };

        let Ingress::Data {
            data,
            received_hops,
            source_interface,
            arrived_at,
        } = Ingress::classify(packet)
        else {
            panic!("a data packet should classify as data");
        };
        assert_eq!(
            data,
            DataPacket {
                destination_type: DestinationType::Plain,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::Resource,
                maybe_transport_id: Some(TransportId::new([0x11; 16])),
                payload: &mut expected_payload,
            }
        );
        assert_eq!(received_hops, 6);
        assert_eq!(source_interface, iface(0x05));
        assert_eq!(arrived_at, InstantMillis(21));
    }

    #[test]
    fn data_packets_classify_for_every_destination_type() {
        for destination_type in [
            DestinationType::Single,
            DestinationType::Group,
            DestinationType::Plain,
            DestinationType::Link,
        ] {
            let header = WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type,
                packet_type: PacketType::Data,
                hops: 0,
                transport_id: None,
                destination: DestinationHash::new([0xA5; 16]),
                context: WireContext::None,
            };
            let mut bytes = [0u8; HEADER_LEN];
            assert_eq!(header.write(&mut bytes).unwrap(), HEADER_LEN);
            let packet = InboundPacket {
                arrived_at: InstantMillis(23),
                source_interface: iface(0x06),
                bytes: &mut bytes,
            };

            let Ingress::Data { data, .. } = Ingress::classify(packet) else {
                panic!("data packets to any destination type classify as data");
            };
            assert_eq!(data.destination_type, destination_type);
            assert!(data.payload.is_empty());
        }
    }

    #[test]
    fn announce_packets_must_target_a_single_destination() {
        let mut raw = hx(RAW_ANNOUNCE);
        raw[0] |= (DestinationType::Group as u8) << 2;
        let packet = InboundPacket {
            arrived_at: InstantMillis(11),
            source_interface: iface(0x03),
            bytes: &mut raw,
        };

        assert!(matches!(Ingress::classify(packet), Ingress::Unparseable));
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
            bytes: &mut raw,
        };

        let classified = Ingress::classify(packet);
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
