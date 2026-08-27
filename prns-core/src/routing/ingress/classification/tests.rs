use super::*;
use crate::engine::test_support::*;
use crate::routing::ingress::testkit::{header_bytes, iface};
use crate::wire::{
    ContextFlag, DestinationType, PropagationType, TransportId, HEADER_MIN_LEN, MAX_HOP_COUNT,
};

#[test]
fn a_local_client_transit_is_discounted_one_hop() {
    let local_client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app-1");
    let tcp = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"1.2.3.4:4242");
    assert_eq!(local_adjusted_hops(5, local_client), 4);
    assert_eq!(local_adjusted_hops(5, tcp), 5);
    assert_eq!(local_adjusted_hops(0, local_client), 0);
}

#[test]
fn an_ifac_flagged_packet_is_dropped_at_the_door_like_rns_on_a_non_ifac_interface() {
    let mut raw = crate::engine::test_support::bytes_from_hex(
        crate::engine::test_support::RNS_1_4_2_ANNOUNCE,
    );
    raw[0] |= 0x80;
    let packet = InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: iface(0x01),
        bytes: &mut raw,
    };

    assert!(matches!(Ingress::classify(packet), Ingress::IfacRefused));
}

#[test]
fn an_ifac_flagged_packet_is_refused_even_when_its_masked_hops_byte_is_out_of_range() {
    let mut raw = crate::engine::test_support::bytes_from_hex(
        crate::engine::test_support::RNS_1_4_2_ANNOUNCE,
    );
    raw[0] |= 0x80;
    raw[1] = MAX_HOP_COUNT;
    let packet = InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: iface(0x01),
        bytes: &mut raw,
    };

    assert!(matches!(Ingress::classify(packet), Ingress::IfacRefused));
}

#[test]
fn malformed_headers_classify_as_malformed() {
    let packet = InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: iface(0x01),
        bytes: &mut [0x01],
    };

    assert!(matches!(Ingress::classify(packet), Ingress::Malformed));
}

#[test]
fn rejected_packets_never_expose_a_canonical_hash() {
    let mut malformed_bytes = [0x01];
    let malformed = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: iface(0x01),
        bytes: &mut malformed_bytes,
    });
    assert_eq!(malformed.packet_hash(), None);

    let mut ifac = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    ifac[0] |= 0x80;
    let refused = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(7),
        source_interface: iface(0x01),
        bytes: &mut ifac,
    });
    assert_eq!(refused.packet_hash(), None);
}

#[test]
fn classified_proof_exposes_its_parsed_fast_path_view() {
    let mut bytes = header_bytes(PacketType::Proof);
    let expected_address = WirePacketHeader::parse(&bytes).unwrap().0.address;
    let classified = ClassifiedInboundPacket::classify(InboundPacket {
        arrived_at: InstantMillis(9),
        source_interface: iface(0x02),
        bytes: &mut bytes,
    });

    assert_eq!(classified.proof(), Some((expected_address, &[][..])));
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
            PacketType::LinkRequest => {
                assert!(matches!(classified, Ingress::LinkRequest { .. }))
            }
            PacketType::Proof => assert!(matches!(classified, Ingress::Proof { .. })),
            PacketType::Announce => unreachable!(),
        }
    }
}

#[test]
fn packets_at_the_pathfinder_boundary_are_rejected_before_classification() {
    for packet_type in [
        PacketType::Data,
        PacketType::Announce,
        PacketType::LinkRequest,
        PacketType::Proof,
    ] {
        let mut bytes = match packet_type {
            PacketType::Announce => bytes_from_hex(RNS_1_4_2_ANNOUNCE),
            PacketType::Data | PacketType::LinkRequest | PacketType::Proof => {
                header_bytes(packet_type).to_vec()
            }
        };
        bytes[1] = MAX_HOP_COUNT;
        let packet = InboundPacket {
            arrived_at: InstantMillis(9),
            source_interface: iface(0x02),
            bytes: &mut bytes,
        };

        assert!(
            matches!(Ingress::classify(packet), Ingress::Malformed),
            "{packet_type:?} at PATHFINDER_M must be rejected like RNS 1.4.2",
        );
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
        address: WireAddress::new([0xA5; 16]),
        context: WireContext::Resource,
    };
    let payload = [0xDE, 0xAD, 0xBE, 0xEF];
    let mut expected_payload = payload;
    let mut bytes = [0u8; BROADCAST_MTU];
    let header_len = header.write(&mut bytes).unwrap();
    bytes[header_len..header_len + payload.len()].copy_from_slice(&payload);
    let packet_len = header_len + payload.len();
    let expected_hash = PacketHash::of_wire_packet(&bytes[..packet_len]).unwrap();

    let packet = InboundPacket {
        arrived_at: InstantMillis(21),
        source_interface: iface(0x05),
        bytes: &mut bytes[..packet_len],
    };
    let classified = ClassifiedInboundPacket::classify(packet);
    assert_eq!(classified.packet_hash(), Some(expected_hash));
    let (classified_source, ingress) = classified.into_parts();

    let Ingress::Data {
        packet_hash,
        data,
        received_hops,
        source_interface,
        arrived_at,
    } = ingress
    else {
        panic!("a data packet should classify as data");
    };
    assert_eq!(
        data,
        DataPacket {
            header,
            payload: &mut expected_payload,
        }
    );
    assert_eq!(received_hops, 6);
    assert_eq!(classified_source, iface(0x05));
    assert_eq!(source_interface, iface(0x05));
    assert_eq!(arrived_at, InstantMillis(21));
    assert_eq!(packet_hash, expected_hash);
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
            address: WireAddress::new([0xA5; 16]),
            context: WireContext::None,
        };
        let mut bytes = [0u8; HEADER_MIN_LEN];
        assert_eq!(header.write(&mut bytes).unwrap(), HEADER_MIN_LEN);
        let packet = InboundPacket {
            arrived_at: InstantMillis(23),
            source_interface: iface(0x06),
            bytes: &mut bytes,
        };

        let Ingress::Data { data, .. } = Ingress::classify(packet) else {
            panic!("data packets to any destination type classify as data");
        };
        assert_eq!(data.header.destination_type, destination_type);
        assert!(data.payload.is_empty());
    }
}

#[test]
fn announce_packets_must_target_a_single_destination() {
    let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    raw[0] |= (DestinationType::Group as u8) << 2;
    let packet = InboundPacket {
        arrived_at: InstantMillis(11),
        source_interface: iface(0x03),
        bytes: &mut raw,
    };

    assert!(matches!(Ingress::classify(packet), Ingress::Malformed));
}

#[test]
fn the_last_valid_wire_hop_reaches_the_pathfinder_boundary() {
    let mut raw = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
    raw[1] = MAX_HOP_COUNT - 1;
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
    assert_eq!(received_hops, MAX_HOP_COUNT);
    assert_eq!(classified_source, source_interface);
    assert_eq!(classified_arrival, arrived_at);
}
