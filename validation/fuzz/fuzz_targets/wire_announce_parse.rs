#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::routing::announce::Announce;
use prns_core::wire::WirePacketHeader;

fn exercise_packet(bytes: &[u8]) {
    let Ok((header, payload)) = WirePacketHeader::parse(bytes) else {
        return;
    };

    let mut encoded_header = [0u8; 2 + 2 * prns_core::wire::TRUNCATED_HASH_BYTE_LEN + 1];
    let _ = header.write(&mut encoded_header);
    let _ = Announce::from_wire(&header, payload);
}

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

fuzz_target!(|data: &[u8]| {
    exercise_packet(data);
    if let Some(decoded) = decode_hex(data) {
        exercise_packet(&decoded);
    }
});
