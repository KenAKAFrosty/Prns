#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::engine::{EgressSerializeError, ReemitAnnounce};
use prns_core::interfaces::InterfaceId;
use prns_core::routing::announce::Announce;
use prns_core::wire::{
    DestinationType, PacketType, PropagationType, TransportId, WirePacketHeader, HEADER_MAX_LEN,
};

const RAW_ANNOUNCE_HEX: &[u8] =
    include_bytes!("../corpus/wire_announce_parse/real_rns_announce.hex");

fn decode_hex(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    let (chunks, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }

    for chunk in chunks {
        let high = (chunk[0] as char).to_digit(16)? as u8;
        let low = (chunk[1] as char).to_digit(16)? as u8;
        decoded.push((high << 4) | low);
    }
    Some(decoded)
}

fn hash_bytes<const N: usize>(data: &[u8], offset: usize, fallback: u8) -> [u8; N] {
    let mut bytes = [fallback; N];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        if let Some(input) = data.get(offset + idx) {
            *byte = *input;
        }
    }
    bytes
}

fn exercise_reemit(data: &[u8]) {
    let Some(raw) = decode_hex(RAW_ANNOUNCE_HEX) else {
        return;
    };
    let Ok((orig_header, orig_payload)) = WirePacketHeader::parse(&raw) else {
        return;
    };
    let Ok(announce) = Announce::from_wire(&orig_header, orig_payload) else {
        return;
    };

    let via = TransportId::new(hash_bytes(data, 1, 0xA1));
    let target = InterfaceId::new(hash_bytes(data, 17, 0xB2));
    let emit_hops = data
        .first()
        .copied()
        .unwrap_or(orig_header.hops.saturating_add(1));
    let directive = ReemitAnnounce {
        announce: announce.clone(),
        emit_hops,
        via,
        target,
        is_path_response: false,
    };

    let total_len = HEADER_MAX_LEN + announce.wire_bytes();
    let mut short_buf = vec![0u8; total_len - 1];
    assert_eq!(
        directive.to_wire(&mut short_buf),
        Err(EgressSerializeError::BufferTooShort)
    );

    let extra_capacity = data.get(33).map_or(0, |byte| usize::from(*byte % 8));
    let mut out = vec![0u8; total_len + extra_capacity];
    let written = directive.to_wire(&mut out).expect("egress serializes");
    assert_eq!(written, total_len);

    let (header, payload) = WirePacketHeader::parse(&out[..written]).expect("egress parses");
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(header.destination_type, DestinationType::Single);
    assert_eq!(header.propagation, PropagationType::Transport);
    assert_eq!(header.transport_id, Some(via));
    assert_eq!(header.hops, emit_hops);
    assert_eq!(header.address, orig_header.address);
    assert_eq!(payload, orig_payload);
    assert_eq!(directive.target, target);
}

fuzz_target!(|data: &[u8]| {
    exercise_reemit(data);
});
