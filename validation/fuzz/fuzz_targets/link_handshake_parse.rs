#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use prns_core::crypto::{
    x25519_diffie_hellman, Ed25519PublicKey, X25519PublicKey, X25519SecretKey,
};
use prns_core::routing::links::handshake::{
    parse_link_request, parse_link_rtt, validate_link_proof,
};
use prns_core::routing::links::{LinkId, LinkKey};

fn link_key() -> &'static LinkKey {
    static KEY: OnceLock<LinkKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x21; 32]),
            &X25519PublicKey([0x63; 32]),
        );
        LinkKey::derive(&LinkId::new([0x42; 16]), &shared)
    })
}

fn exercise_handshake(bytes: &[u8]) {
    let _ = parse_link_request(bytes);
    let _ = validate_link_proof(bytes, &Ed25519PublicKey([0x5A; 32]));
    let _ = parse_link_rtt(bytes, link_key());
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
    exercise_handshake(data);
    if let Some(decoded) = decode_hex(data) {
        exercise_handshake(&decoded);
    }
});
