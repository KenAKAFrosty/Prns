use super::*;
use proptest::prelude::*;

fn bytes_from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

#[test]
fn type1_header_round_trips() {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops: 3,
        transport_id: None,
        address: WireAddress::new([0xAB; TRUNCATED_HASH_BYTE_LEN]),
        context: WireContext::None,
    };

    let mut buf = [0u8; 64];
    let written = header.write(&mut buf).unwrap();
    assert_eq!(written, 2 + TRUNCATED_HASH_BYTE_LEN + 1);

    let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();
    assert_eq!(parsed, header);
    assert!(payload.is_empty());
}

#[test]
fn type2_header_round_trips_with_every_one_bit_set() {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Authenticated,
        context_flag: ContextFlag::Set,
        propagation: PropagationType::Transport,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 7,
        transport_id: Some(TransportId::new([0x11; TRUNCATED_HASH_BYTE_LEN])),
        address: WireAddress::new([0x22; TRUNCATED_HASH_BYTE_LEN]),
        context: WireContext::PathResponse,
    };

    let mut buf = [0u8; 64];
    let written = header.write(&mut buf).unwrap();
    assert_eq!(written, 2 + 2 * TRUNCATED_HASH_BYTE_LEN + 1);

    let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();
    assert_eq!(parsed, header);
    assert!(payload.is_empty());
}

#[test]
fn write_rejects_one_byte_short_header_buffers() {
    let type1 = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops: 3,
        transport_id: None,
        address: WireAddress::new([0xAB; TRUNCATED_HASH_BYTE_LEN]),
        context: WireContext::None,
    };
    let mut type1_short = [0u8; 2 + TRUNCATED_HASH_BYTE_LEN];
    assert_eq!(
        type1.write(&mut type1_short),
        Err(WireError::BufferTooShort)
    );

    let type2 = WirePacketHeader {
        transport_id: Some(TransportId::new([0x11; TRUNCATED_HASH_BYTE_LEN])),
        address: WireAddress::new([0x22; TRUNCATED_HASH_BYTE_LEN]),
        ..type1
    };
    let mut type2_short = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN];
    assert_eq!(
        type2.write(&mut type2_short),
        Err(WireError::BufferTooShort)
    );
}

#[test]
fn decodes_a_real_rns_announce() {
    let raw = bytes_from_hex(
        "0100e4cd902bf205ffc02a4e1c667afa214e0002cd8c52db77603c33d2c8c11ea852\
         4f2c1caca0f5535b2462045b1b1a683501f8e9bc5442cfbae5e4ca8ec88942e84558\
         f790c0f5f99c78f08d3c0d9e7429f89ab8d12b5e2cafc834dc8d4301deda006a171b\
         768c52c1d010bc5c8c5163940c77c311def1f81e67995ef331edbd848e5cb869badf\
         d4cb7220ee688c3c2817ae0e851909b3afbffcc5a796362a944d1404708f0268656c\
         6c6f2d706572736f6e616c",
    );

    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();

    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(header.destination_type, DestinationType::Single);
    assert_eq!(header.propagation, PropagationType::Broadcast);
    assert_eq!(header.ifac_flag, IfacFlag::Open);
    assert_eq!(header.context_flag, ContextFlag::Unset);
    assert_eq!(header.context, WireContext::None);
    assert_eq!(header.hops, 0);
    assert_eq!(header.transport_id, None);
    assert_eq!(
        DestinationHash::from_address(header.address),
        DestinationHash::from_slice(&bytes_from_hex("e4cd902bf205ffc02a4e1c667afa214e")).unwrap()
    );
    assert_eq!(payload.len(), 162);

    let mut buf = [0u8; 64];
    let written = header.write(&mut buf).unwrap();
    assert_eq!(written, 19);
    assert_eq!(&buf[..written], &raw[..written]);
}

#[test]
fn every_flags_byte_round_trips_with_unknown_context_and_payload() {
    let payload = [0xDE, 0xAD, 0xBE, 0xEF];
    for meta in 0u8..=u8::MAX {
        let is_type_2 = meta & 0b0100_0000 != 0;
        let header_len =
            2 + usize::from(is_type_2) * TRUNCATED_HASH_BYTE_LEN + TRUNCATED_HASH_BYTE_LEN + 1;
        let mut raw = vec![0u8; header_len + payload.len()];

        raw[0] = meta;
        raw[1] = 0x7A;
        let mut offset = 2;
        if is_type_2 {
            raw[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(&[0x11; 16]);
            offset += TRUNCATED_HASH_BYTE_LEN;
        }
        raw[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(&[0x22; 16]);
        offset += TRUNCATED_HASH_BYTE_LEN;
        raw[offset] = 0xA5;
        offset += 1;
        raw[offset..].copy_from_slice(&payload);

        let (header, parsed_payload) = WirePacketHeader::parse(&raw).unwrap();
        assert_eq!(header.context, WireContext::Unknown(0xA5));
        assert_eq!(parsed_payload, payload);

        let mut encoded = [0u8; 64];
        let written = header.write(&mut encoded).unwrap();
        assert_eq!(written, header_len);
        assert_eq!(
            &encoded[..written],
            &raw[..header_len],
            "flags {meta:#04x} did not preserve the header bytes",
        );
    }
}

#[test]
fn parse_rejects_truncated_input() {
    assert_eq!(
        WirePacketHeader::parse(&[0x01]),
        Err(WireError::BufferTooShort)
    );
    assert_eq!(
        WirePacketHeader::parse(&[0u8; 18]),
        Err(WireError::BufferTooShort)
    );
    let mut type_2 = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN];
    type_2[0] = 0b0100_0000;
    assert_eq!(
        WirePacketHeader::parse(&type_2),
        Err(WireError::BufferTooShort)
    );
}

fn ifac_flags() -> impl Strategy<Value = IfacFlag> {
    prop_oneof![Just(IfacFlag::Open), Just(IfacFlag::Authenticated)]
}

fn context_flags() -> impl Strategy<Value = ContextFlag> {
    prop_oneof![Just(ContextFlag::Unset), Just(ContextFlag::Set)]
}

fn propagation_types() -> impl Strategy<Value = PropagationType> {
    prop_oneof![
        Just(PropagationType::Broadcast),
        Just(PropagationType::Transport)
    ]
}

fn destination_types() -> impl Strategy<Value = DestinationType> {
    prop_oneof![
        Just(DestinationType::Single),
        Just(DestinationType::Group),
        Just(DestinationType::Plain),
        Just(DestinationType::Link)
    ]
}

fn packet_types() -> impl Strategy<Value = PacketType> {
    prop_oneof![
        Just(PacketType::Data),
        Just(PacketType::Announce),
        Just(PacketType::LinkRequest),
        Just(PacketType::Proof)
    ]
}

fn contexts() -> impl Strategy<Value = WireContext> {
    any::<u8>().prop_map(WireContext::from_byte)
}

fn headers() -> impl Strategy<Value = WirePacketHeader> {
    (
        ifac_flags(),
        any::<bool>(),
        context_flags(),
        propagation_types(),
        destination_types(),
        packet_types(),
        any::<u8>(),
        any::<[u8; TRUNCATED_HASH_BYTE_LEN]>(),
        any::<[u8; TRUNCATED_HASH_BYTE_LEN]>(),
        contexts(),
    )
        .prop_map(
            |(
                ifac_flag,
                has_transport_id,
                context_flag,
                propagation,
                destination_type,
                packet_type,
                hops,
                transport_id,
                destination,
                context,
            )| WirePacketHeader {
                ifac_flag,
                context_flag,
                propagation,
                destination_type,
                packet_type,
                hops,
                transport_id: has_transport_id.then(|| TransportId::new(transport_id)),
                address: WireAddress::new(destination),
                context,
            },
        )
}

proptest! {
    #[test]
    fn arbitrary_headers_write_then_parse_back(header in headers()) {
        let mut buf = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN + 1];
        let written = header.write(&mut buf).unwrap();

        let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();

        prop_assert_eq!(parsed, header);
        prop_assert!(payload.is_empty());
    }
}
