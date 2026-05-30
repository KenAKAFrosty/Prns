#![no_main]

use libfuzzer_sys::fuzz_target;
use personal_rns::engine::egress::{EgressDirective, EgressSerializeError};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::announce::Announce;
use personal_rns::wire::{
    DestinationType, PacketType, PropagationType, WirePacketHeader, HEADER_LEN,
};

const RAW_ANNOUNCE_HEX: &[u8] =
    include_bytes!("../corpus/wire_announce_parse/real_rns_announce.hex");

fn decode_hex(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    let chunks = bytes.chunks_exact(2);
    if !chunks.remainder().is_empty() {
        return None;
    }

    for chunk in chunks {
        let high = (chunk[0] as char).to_digit(16)? as u8;
        let low = (chunk[1] as char).to_digit(16)? as u8;
        decoded.push((high << 4) | low);
    }
    Some(decoded)
}

fn interface_id(data: &[u8], offset: usize, fallback: u8) -> InterfaceId {
    let mut bytes = [fallback; personal_rns::wire::TRUNCATED_HASH_BYTE_LEN];
    for (idx, byte) in bytes.iter_mut().enumerate() {
        if let Some(input) = data.get(offset + idx) {
            *byte = *input;
        }
    }
    InterfaceId::new(bytes)
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

    let targets = [interface_id(data, 1, 0xA1), interface_id(data, 17, 0xB2)];
    let target_count = 1 + data.get(33).map_or(0, |byte| usize::from(*byte & 0x01));
    let fire_on = &targets[..target_count];
    let emit_hops = data
        .first()
        .copied()
        .unwrap_or(orig_header.hops.saturating_add(1));
    let directive = EgressDirective::ReemitAnnounce {
        announce: announce.clone(),
        emit_hops,
        fire_on,
    };

    let total_len = HEADER_LEN + announce.wire_len();
    let mut short_buf = vec![0u8; total_len - 1];
    assert_eq!(
        directive.to_wire(&mut short_buf),
        Err(EgressSerializeError::BufferTooShort)
    );

    let extra_capacity = data.get(34).map_or(0, |byte| usize::from(*byte % 8));
    let mut out = vec![0u8; total_len + extra_capacity];
    let written = directive.to_wire(&mut out).expect("egress serializes");
    assert_eq!(written, total_len);

    let (header, payload) = WirePacketHeader::parse(&out[..written]).expect("egress parses");
    assert_eq!(header.packet_type, PacketType::Announce);
    assert_eq!(header.destination_type, DestinationType::Single);
    assert_eq!(header.propagation, PropagationType::Broadcast);
    assert_eq!(header.transport_id, None);
    assert_eq!(header.hops, emit_hops);
    assert_eq!(header.destination, orig_header.destination);
    assert_eq!(payload, orig_payload);
    assert_eq!(directive.fire_on(), fire_on);
}

fuzz_target!(|data: &[u8]| {
    exercise_reemit(data);
});
