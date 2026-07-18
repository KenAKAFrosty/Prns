use super::*;
use crate::crypto::{ed25519_public_key, ed25519_sign, sha256, Ed25519SecretKey};
use crate::wire::{WireContext, HEADER_MIN_LEN};

const RAW: &str = "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
                   59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
                   0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
                   7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
                   4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

fn bytes_from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}
fn a<const N: usize>(s: &str) -> [u8; N] {
    bytes_from_hex(s).try_into().expect("expected length")
}

#[test]
fn from_wire_validates_real_rns_announce() {
    let raw = bytes_from_hex(RAW);
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    let announce = Announce::from_wire(&header, payload).unwrap();

    assert_eq!(
        announce.destination,
        DestinationHash::new(a("16f8a6d3f7d7c5b6f106d293804d7314")),
    );
    assert_eq!(
        announce.public_keys.encryption,
        IdentityEncryptionPublicKey::new(X25519PublicKey(a(
            "02281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d25"
        ))),
    );
    assert_eq!(
        announce.public_keys.signing,
        IdentitySigningPublicKey::new(Ed25519PublicKey(a(
            "83a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f66700"
        ))),
    );
    assert_eq!(
        announce.dotted_name_hash,
        DottedNameHash::new(a("9618ccc8f5ebce060084"))
    );
    assert_eq!(
        announce.announce_id,
        AnnounceId::from_wire(a("5468d9b829006a172e83"))
    );
    assert_eq!(announce.ratchet, None);
    assert_eq!(announce.app_data, b"hello-personal");
}

#[test]
fn to_wire_reproduces_the_real_payload_exactly() {
    let raw = bytes_from_hex(RAW);
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    let announce = Announce::from_wire(&header, payload).unwrap();

    let mut buf = [0u8; BROADCAST_MTU];
    let n = announce.to_wire(&mut buf).unwrap();
    assert_eq!(n, payload.len());
    assert_eq!(&buf[..n], payload);
    assert_eq!(n, announce.wire_len());
}

#[test]
fn a_path_response_is_a_normal_announce_with_only_the_context_byte_flipped() {
    let raw = bytes_from_hex(RAW);
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    let announce = Announce::from_wire(&header, payload).unwrap();

    let mut normal = [0u8; 500];
    let n = write_announce_wire_packet(&announce, 0, &mut normal).unwrap();
    let mut response = [0u8; 500];
    let m = write_path_response_announce_wire_packet(&announce, 0, &mut response).unwrap();
    assert_eq!(n, m);

    let context_offset = HEADER_MIN_LEN - 1;
    assert_eq!(normal[context_offset], WireContext::None.to_byte());
    assert_eq!(
        response[context_offset],
        WireContext::PathResponse.to_byte()
    );

    let mut patched = response;
    patched[context_offset] = WireContext::None.to_byte();
    assert_eq!(
        &patched[..m],
        &normal[..n],
        "the only difference from a normal announce is the context byte",
    );

    let (re_header, re_payload) = WirePacketHeader::parse(&response[..m]).unwrap();
    assert_eq!(re_header.context, WireContext::PathResponse);
    assert_eq!(re_header.packet_type, PacketType::Announce);
    assert_eq!(
        Announce::from_wire(&re_header, re_payload).unwrap(),
        announce
    );
}

#[test]
fn rejects_tampered_signature() {
    let mut raw = bytes_from_hex(RAW);
    raw[103] ^= 1;
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    assert_eq!(
        Announce::from_wire(&header, payload),
        Err(AnnounceValidationError::InvalidSignature),
    );
}

#[test]
fn rejects_non_single_destination() {
    let mut raw = bytes_from_hex(RAW);
    raw[0] |= 0b0000_0100;
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    assert_eq!(
        Announce::from_wire(&header, payload),
        Err(AnnounceValidationError::NotSingleDestination),
    );
}

#[test]
fn rejects_truncated_payload() {
    let raw = bytes_from_hex(RAW);
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    assert_eq!(
        Announce::from_wire(&header, &payload[..100]),
        Err(AnnounceValidationError::PayloadTooSmall),
    );
}

#[test]
fn rejects_oversized_payload() {
    let raw = bytes_from_hex(RAW);
    let (header, _) = WirePacketHeader::parse(&raw).unwrap();
    let oversized = [0u8; 600];
    assert_eq!(
        Announce::from_wire(&header, &oversized),
        Err(AnnounceValidationError::PayloadTooBig),
    );
}

fn synthetic_announce(
    signed_destination: [u8; 16],
    ratchet: Option<[u8; RATCHET_BYTE_LEN]>,
    app_data: &[u8],
) -> Vec<u8> {
    let secret = Ed25519SecretKey::new([0x11u8; 32]);
    let signing = ed25519_public_key(&secret).0;
    let mut pubkey = [0u8; 64];
    pubkey[..32].copy_from_slice(&[0x22u8; 32]);
    pubkey[32..].copy_from_slice(&signing);
    let name_hash = [0x33u8; 10];
    let announce_id = [0x44u8; 10];

    let mut signed = Vec::new();
    signed.extend_from_slice(&signed_destination);
    signed.extend_from_slice(&pubkey);
    signed.extend_from_slice(&name_hash);
    signed.extend_from_slice(&announce_id);
    if let Some(ratchet) = ratchet {
        signed.extend_from_slice(&ratchet);
    }
    signed.extend_from_slice(app_data);
    let sig = ed25519_sign(&secret, &signed).0;

    let mut payload = Vec::new();
    payload.extend_from_slice(&pubkey);
    payload.extend_from_slice(&name_hash);
    payload.extend_from_slice(&announce_id);
    if let Some(ratchet) = ratchet {
        payload.extend_from_slice(&ratchet);
    }
    payload.extend_from_slice(&sig);
    payload.extend_from_slice(app_data);

    let flags = if ratchet.is_some() { 0x21 } else { 0x01 };
    let mut raw = vec![flags, 0x00];
    raw.extend_from_slice(&signed_destination);
    raw.push(0x00);
    raw.extend_from_slice(&payload);
    raw
}

fn bound_destination() -> [u8; 16] {
    let signing = ed25519_public_key(&Ed25519SecretKey::new([0x11u8; 32])).0;
    let mut pubkey = [0u8; 64];
    pubkey[..32].copy_from_slice(&[0x22u8; 32]);
    pubkey[32..].copy_from_slice(&signing);
    let mut idh = [0u8; 16];
    idh.copy_from_slice(&sha256(&pubkey)[..16]);
    *derive_destination_hash(&IdentityHash::new(idh), &DottedNameHash::new([0x33u8; 10])).as_bytes()
}

#[test]
fn synthetic_announce_with_correct_binding_validates() {
    let raw = synthetic_announce(bound_destination(), None, b"app");
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    let announce = Announce::from_wire(&header, payload).unwrap();
    assert_eq!(announce.app_data, b"app");
    assert_eq!(
        announce.destination,
        DestinationHash::new(bound_destination())
    );
}

#[test]
fn synthetic_ratchet_announce_validates_and_round_trips() {
    let ratchet = [0x55u8; RATCHET_BYTE_LEN];
    let raw = synthetic_announce(bound_destination(), Some(ratchet), b"ratchet-app");
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    assert_eq!(header.context_flag, ContextFlag::Set);

    let announce = Announce::from_wire(&header, payload).unwrap();
    assert_eq!(announce.ratchet, Some(RatchetKey::new(ratchet)));
    assert_eq!(announce.app_data, b"ratchet-app");

    let mut buf = [0u8; BROADCAST_MTU];
    let n = announce.to_wire(&mut buf).unwrap();
    assert_eq!(n, payload.len());
    assert_eq!(&buf[..n], payload);
}

#[test]
fn rejects_destination_mismatch() {
    let raw = synthetic_announce([0x99u8; 16], None, b"app");
    let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
    assert_eq!(
        Announce::from_wire(&header, payload),
        Err(AnnounceValidationError::DestinationMismatch),
    );
}

#[test]
fn derive_destination_hash_matches_rns_1_3_5() {
    let identity_hash = IdentityHash::new(a("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
    let dotted_name_hash = DottedNameHash::new(a("8794b70072dbf251144b"));
    assert_eq!(
        derive_destination_hash(&identity_hash, &dotted_name_hash),
        DestinationHash::new(a("33d610d1d6a7f4f809ebfe62c0ce7d43")),
    );

    let real_pubkey = a::<64>(
        "02281f6d21232cbba9d12e516183197f08e59b7afba27e99e4fe39f01b0d4d25\
         83a5920220253970a16861e82e52e955a05ee39e2b6d20a2331f515512f66700",
    );
    let mut real_identity_hash = [0u8; 16];
    real_identity_hash.copy_from_slice(&sha256(&real_pubkey)[..16]);
    assert_eq!(
        derive_destination_hash(
            &IdentityHash::new(real_identity_hash),
            &DottedNameHash::new(a("9618ccc8f5ebce060084")),
        ),
        DestinationHash::new(a("16f8a6d3f7d7c5b6f106d293804d7314")),
    );
}

#[test]
fn derive_plain_destination_hash_matches_rns_1_3_5() {
    let name = expand_name("rnstransport", &["path", "request"]).unwrap();
    assert_eq!(name, DottedNameHash::new(a("7926bbe7dd7f9aba88b0")));
    assert_eq!(
        derive_plain_destination_hash(&name),
        DestinationHash::new(a("6b9f66014d9853faab220fba47d02761")),
    );
}

#[test]
fn derive_single_destination_hash_composes_the_rns_1_3_5_address_from_name_parts() {
    let identity_hash = IdentityHash::new(a("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
    assert_eq!(
        derive_single_destination_hash(&identity_hash, "personal", &["node"]),
        Ok(DestinationHash::new(a("c3cfae69b36bb6e3bbfd96a3b5867a59"))),
    );
    assert_eq!(
        derive_single_destination_hash(&identity_hash, "per.sonal", &["node"]),
        Err(ExpandNameError::DotInComponent),
    );
}

#[test]
fn expand_name_matches_rns_1_3_5() {
    assert_eq!(
        expand_name("personal", &["announce"]).unwrap(),
        DottedNameHash::new(a("8794b70072dbf251144b")),
    );
    assert_eq!(
        expand_name("personal", &["node"]).unwrap(),
        DottedNameHash::new(a("ab49baa826f122c1437f")),
    );
    assert_eq!(
        expand_name("personal", &[]).unwrap(),
        DottedNameHash::new(a("4a0a339b0c6d05538977")),
    );
}

#[test]
fn expand_name_rejects_dots_in_components_like_rns() {
    assert_eq!(
        expand_name("per.sonal", &["node"]),
        Err(ExpandNameError::DotInComponent),
    );
    assert_eq!(
        expand_name("personal", &["no.de"]),
        Err(ExpandNameError::DotInComponent),
    );
}

#[test]
fn expand_name_rejects_names_past_the_bound() {
    let overlong = "x".repeat(MAX_DOTTED_NAME_LEN + 1);
    assert_eq!(
        expand_name(&overlong, &[]),
        Err(ExpandNameError::NameTooLong),
    );
}

#[test]
fn build_signed_matches_rns_1_3_5() {
    use crate::identity::in_memory::InMemoryNodeIdentity;
    let mut secret_key_bytes = [0u8; 64];
    secret_key_bytes[..32].fill(0x22);
    secret_key_bytes[32..].fill(0x11);
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes);

    let announce = Announce::build_signed(
        &identity,
        DottedNameHash::new(a("8794b70072dbf251144b")),
        AnnounceId::from_wire([0x44; ANNOUNCE_ID_WIRE_LEN]),
        None,
        b"hello-personal",
    )
    .unwrap();

    assert_eq!(
        announce.destination,
        DestinationHash::new(a("33d610d1d6a7f4f809ebfe62c0ce7d43")),
    );
    let mut buf = [0u8; BROADCAST_MTU];
    let n = announce.to_wire(&mut buf).unwrap();
    assert_eq!(
        &buf[..n],
        bytes_from_hex(
            "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
            d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737\
            8794b70072dbf251144b\
            44444444444444444444\
            77000516a77f83f26b6fd0abc4b9b8a0de0fd8bc51f82fe55e14b75628b41955\
            c895395870fe4cd0b69afc85e4969cc3b70dbeb14d8c3c7ddc08692e0968010e\
            68656c6c6f2d706572736f6e616c"
        )
        .as_slice(),
    );
}

#[test]
fn build_signed_round_trips_through_the_validator() {
    use crate::identity::in_memory::InMemoryNodeIdentity;
    let mut secret_key_bytes = [0u8; 64];
    secret_key_bytes[..32].fill(0x07);
    secret_key_bytes[32..].fill(0x09);
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes);

    let built = Announce::build_signed(
        &identity,
        DottedNameHash::new([0xAB; DOTTED_NAME_HASH_BYTE_LEN]),
        AnnounceId::from_wire([0x01; ANNOUNCE_ID_WIRE_LEN]),
        Some(RatchetKey::new([0x55; RATCHET_BYTE_LEN])),
        b"round-trip",
    )
    .unwrap();

    let mut payload = [0u8; BROADCAST_MTU];
    let n = built.to_wire(&mut payload).unwrap();
    let mut raw = vec![0x21u8, 0x00];
    raw.extend_from_slice(built.destination.as_bytes());
    raw.push(0x00);
    raw.extend_from_slice(&payload[..n]);

    let (header, parsed_payload) = WirePacketHeader::parse(&raw).unwrap();
    let parsed = Announce::from_wire(&header, parsed_payload).unwrap();
    assert_eq!(parsed, built);
}
