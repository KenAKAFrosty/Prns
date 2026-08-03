use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WireError, WirePacketHeader, HEADER_MIN_LEN, TRUNCATED_HASH_BYTE_LEN,
};

/// RNS derives `rnstransport.path.request` from the name alone; [`crate::routing::announce::derive_plain_destination_hash`] reproduces that derivation.
pub const PATH_REQUEST_DESTINATION: DestinationHash = DestinationHash::new([
    0x6b, 0x9f, 0x66, 0x01, 0x4d, 0x98, 0x53, 0xfa, 0xab, 0x22, 0x0f, 0xba, 0x47, 0xd0, 0x27, 0x61,
]);

pub const PATH_REQUEST_PAYLOAD_LEN: usize = TRUNCATED_HASH_BYTE_LEN * 2;

/// RNS 1.4.2 `Transport.request_path`
pub fn write_path_request_wire_packet(
    destination: DestinationHash,
    requester_transport_id: Option<TransportId>,
    id: &[u8; TRUNCATED_HASH_BYTE_LEN],
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: PATH_REQUEST_DESTINATION.to_address(),
        context: WireContext::None,
    };
    let payload_len = match requester_transport_id {
        Some(_) => PATH_REQUEST_PAYLOAD_LEN + TRUNCATED_HASH_BYTE_LEN,
        None => PATH_REQUEST_PAYLOAD_LEN,
    };
    let total_len = HEADER_MIN_LEN + payload_len;
    if buf.len() < total_len {
        return Err(WireError::BufferTooShort);
    }
    header.write(&mut buf[..HEADER_MIN_LEN])?;
    let payload = &mut buf[HEADER_MIN_LEN..total_len];
    payload[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
    let id_offset = match requester_transport_id {
        Some(via) => {
            payload[TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN * 2]
                .copy_from_slice(via.as_bytes());
            TRUNCATED_HASH_BYTE_LEN * 2
        }
        None => TRUNCATED_HASH_BYTE_LEN,
    };
    payload[id_offset..].copy_from_slice(id);
    Ok(total_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RNS_1_4_2_PATH_REQUEST: &str = "08006b9f66014d9853faab220fba47d02761002222222222\
                                          2222222222222222222222abababababababababababababababab";

    const RNS_1_4_2_PATH_REQUEST_TRANSPORT: &str =
        "08006b9f66014d9853faab220fba47d027610022222222222222222222222222222222\
         7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7aabababababababababababababababab";

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn path_request_destination_matches_rns_1_4_2() {
        assert_eq!(
            PATH_REQUEST_DESTINATION,
            DestinationHash::new(
                bytes_from_hex("6b9f66014d9853faab220fba47d02761")
                    .try_into()
                    .unwrap()
            ),
        );
    }

    #[test]
    fn write_path_request_reproduces_the_rns_1_4_2_wire() {
        let mut buf = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN];
        let n = write_path_request_wire_packet(
            DestinationHash::new([0x22; 16]),
            None,
            &[0xAB; 16],
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], bytes_from_hex(RNS_1_4_2_PATH_REQUEST).as_slice());
    }

    #[test]
    fn write_transport_path_request_reproduces_the_rns_1_4_2_wire() {
        let mut buf = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN + TRUNCATED_HASH_BYTE_LEN];
        let n = write_path_request_wire_packet(
            DestinationHash::new([0x22; 16]),
            Some(TransportId::new([0x7a; 16])),
            &[0xAB; 16],
            &mut buf,
        )
        .unwrap();
        assert_eq!(
            &buf[..n],
            bytes_from_hex(RNS_1_4_2_PATH_REQUEST_TRANSPORT).as_slice()
        );
    }

    #[test]
    fn write_path_request_into_a_short_buffer_is_rejected() {
        let mut tiny = [0u8; HEADER_MIN_LEN + PATH_REQUEST_PAYLOAD_LEN - 1];
        assert_eq!(
            write_path_request_wire_packet(
                DestinationHash::new([0x22; 16]),
                None,
                &[0xAB; 16],
                &mut tiny
            ),
            Err(WireError::BufferTooShort),
        );
    }
}
