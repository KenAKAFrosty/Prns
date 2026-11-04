extern crate std;

use core::convert::Infallible;
use ed25519_dalek::{Signer, SigningKey};
use std::{format, string::String, vec, vec::Vec};

use super::*;

struct TestSigner(SigningKey);

impl ContactCardSigner for TestSigner {
    type Error = Infallible;

    fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_BYTES], Self::Error> {
        Ok(self.0.sign(message).to_bytes())
    }
}

fn identity() -> ([u8; PUBLIC_IDENTITY_BYTES], TestSigner) {
    let seed: [u8; 32] =
        hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap()
            .try_into()
            .unwrap();
    let signer = TestSigner(SigningKey::from_bytes(&seed));
    let mut public_identity = [9_u8; PUBLIC_IDENTITY_BYTES];
    public_identity[32..].copy_from_slice(&signer.0.verifying_key().to_bytes());
    (public_identity, signer)
}

fn card(name: &str) -> Vec<u8> {
    card_for(name, "lxmf.delivery")
}

fn card_for(name: &str, service: &str) -> Vec<u8> {
    let (public_identity, signer) = identity();
    let mut output = [0_u8; MAX_CARD_BYTES];
    let len = encode_signed(
        CardFields {
            public_identity: &public_identity,
            service_name_hash: &service_name_hash(service),
            display_name: name,
        },
        &signer,
        &mut output,
    )
    .unwrap();
    output[..len].to_vec()
}

fn token(wire: &[u8]) -> String {
    let mut output = [0_u8; MAX_TOKEN_BYTES];
    let len = encode_token(wire, &mut output).unwrap();
    String::from_utf8(output[..len].to_vec()).unwrap()
}

#[test]
fn signed_card_roundtrip_derives_reticulum_identity_and_service_destination() {
    let wire = card("Alice 🔭");
    let verified = verify(&wire).unwrap();
    assert_eq!(verified.wire(), wire);
    assert_eq!(verified.public_identity(), &identity().0);
    assert_eq!(verified.display_name(), "Alice 🔭");
    assert_eq!(
        hex::encode(verified.service_name_hash()),
        "6ec60bc318e2c0f0d908"
    );
    assert_eq!(
        hex::encode(verified.identity_hash()),
        "1188f2e7e138427ed42d155edfcb55f2"
    );
    assert_eq!(
        hex::encode(verified.destination_hash()),
        "4c94abc798ac918fce6a7c5fd359910f"
    );
}

#[test]
fn different_services_share_identity_but_have_distinct_signed_destinations() {
    let text = card("Alice");
    let telemetry = card_for("Alice", "example.telemetry");
    let text = verify(&text).unwrap();
    let telemetry = verify(&telemetry).unwrap();
    assert_eq!(text.identity_hash(), telemetry.identity_hash());
    assert_ne!(text.service_name_hash(), telemetry.service_name_hash());
    assert_ne!(text.destination_hash(), telemetry.destination_hash());
    assert_ne!(text.wire(), telemetry.wire());
}

#[test]
fn independent_python_openssl_vectors_verify_and_roundtrip() {
    let document: serde_json::Value =
        serde_json::from_str(include_str!("../tests/vectors/portable-v1.json")).unwrap();
    let vectors = document["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 4);
    for vector in vectors {
        let field = |name: &str| vector[name].as_str().unwrap();
        let wire = hex::decode(field("wire_hex")).unwrap();
        let verified = verify(&wire).unwrap();
        assert_eq!(verified.display_name(), field("display_name"));
        assert_eq!(
            hex::encode(verified.public_identity()),
            field("public_identity_hex")
        );
        assert_eq!(
            hex::encode(verified.service_name_hash()),
            field("service_name_hash_hex")
        );
        assert_eq!(
            *verified.service_name_hash(),
            service_name_hash(field("service"))
        );
        assert_eq!(
            hex::encode(verified.identity_hash()),
            field("identity_hash_hex")
        );
        assert_eq!(
            hex::encode(verified.destination_hash()),
            field("destination_hash_hex")
        );
        assert_eq!(token(&wire), field("token"));
        let mut uri = [0_u8; MAX_URI_BYTES];
        let len = encode_uri(&wire, &mut uri).unwrap();
        assert_eq!(str::from_utf8(&uri[..len]).unwrap(), field("uri"));
        let mut decoded = [0_u8; MAX_CARD_BYTES];
        let len = decode_token(field("token"), &mut decoded).unwrap();
        assert_eq!(&decoded[..len], &wire);
        let len = decode_uri(field("uri"), &mut decoded).unwrap();
        assert_eq!(&decoded[..len], &wire);
    }
}

#[test]
fn each_signed_field_rejects_tampering() {
    let original = card("Alice");
    // Encryption key, signing key, service hash, name and signature all bind.
    for index in [10, 42, 74, HEADER_BYTES, original.len() - 1] {
        let mut changed = original.clone();
        changed[index] ^= 1;
        assert!(verify(&changed).is_err(), "tamper index {index}");
    }
}

#[test]
fn framing_rejects_unknown_versions_truncation_and_trailing_data() {
    let original = card("Alice");
    for length in 0..original.len() {
        assert!(verify(&original[..length]).is_err(), "length {length}");
    }
    let mut changed = original.clone();
    changed[0] ^= 1;
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidMagic);
    changed = original.clone();
    changed[8] = 2;
    assert_eq!(verify(&changed).unwrap_err(), CardError::UnsupportedVersion);
    changed = original.clone();
    changed.push(0);
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidLength);
    changed = original;
    changed[9] += 1;
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidLength);
    assert_eq!(
        verify(&[0; MAX_CARD_BYTES + 1]).unwrap_err(),
        CardError::TooLarge
    );
}

#[test]
fn malformed_name_bytes_are_rejected_before_signature_verification() {
    let mut changed = card("Alice");
    changed[HEADER_BYTES] = 0xff;
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidName);
    changed = card("Alice");
    changed[HEADER_BYTES] = b'\n';
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidName);
    changed[9] = 129;
    assert_eq!(verify(&changed).unwrap_err(), CardError::InvalidName);
}

#[test]
fn empty_and_maximum_multibyte_names_are_supported_without_normalization() {
    assert_eq!(verify(&card("")).unwrap().display_name(), "");
    let name = "💬".repeat(32);
    let wire = card(&name);
    assert_eq!(wire.len(), MAX_CARD_BYTES);
    assert_eq!(verify(&wire).unwrap().display_name(), name);
    assert_ne!(card("é"), card("e\u{301}"));
}

#[test]
fn invalid_names_and_small_output_do_not_call_signer_or_change_output() {
    struct ForbiddenSigner;
    impl ContactCardSigner for ForbiddenSigner {
        type Error = Infallible;
        fn sign(&self, _: &[u8]) -> Result<[u8; SIGNATURE_BYTES], Self::Error> {
            panic!("invalid admission must not ask the identity owner to sign")
        }
    }
    let public_identity = identity().0;
    let service_name_hash = service_name_hash("lxmf.delivery");
    let mut output = [0xa5_u8; MAX_CARD_BYTES];
    for name in [
        "x".repeat(129),
        String::from("A\nB"),
        String::from("A\u{2028}B"),
        String::from("A\u{202e}B"),
        String::from("A\u{2066}B"),
    ] {
        assert_eq!(
            encode_signed(
                CardFields {
                    public_identity: &public_identity,
                    service_name_hash: &service_name_hash,
                    display_name: &name
                },
                &ForbiddenSigner,
                &mut output,
            ),
            Err(EncodeError::InvalidCard(CardError::InvalidName))
        );
    }
    assert_eq!(output, [0xa5; MAX_CARD_BYTES]);
    assert!(matches!(
        encode_signed(
            CardFields {
                public_identity: &public_identity,
                service_name_hash: &service_name_hash,
                display_name: "Alice"
            },
            &ForbiddenSigner,
            &mut output[..10],
        ),
        Err(EncodeError::OutputTooSmall { .. })
    ));
    assert_eq!(output, [0xa5; MAX_CARD_BYTES]);
}

#[test]
fn wrong_signer_and_signing_failure_never_export_partial_cards() {
    let (public_identity, _) = identity();
    let name_hash = service_name_hash("lxmf.delivery");
    let fields = CardFields {
        public_identity: &public_identity,
        service_name_hash: &name_hash,
        display_name: "Alice",
    };
    let mut output = [0xa5_u8; MAX_CARD_BYTES];
    let wrong = TestSigner(SigningKey::from_bytes(&[3; 32]));
    assert_eq!(
        encode_signed(fields, &wrong, &mut output),
        Err(EncodeError::InvalidCard(CardError::InvalidSignature))
    );
    struct Failing;
    impl ContactCardSigner for Failing {
        type Error = u8;
        fn sign(&self, _: &[u8]) -> Result<[u8; SIGNATURE_BYTES], u8> {
            Err(42)
        }
    }
    assert_eq!(
        encode_signed(fields, &Failing, &mut output),
        Err(EncodeError::Signer(42))
    );
    assert_eq!(output, [0xa5; MAX_CARD_BYTES]);
}

#[test]
fn signature_domain_is_mandatory() {
    let mut wire = card("Alice");
    let unsigned_len = wire.len() - SIGNATURE_BYTES;
    let plain_signature = identity().1 .0.sign(&wire[..unsigned_len]).to_bytes();
    wire[unsigned_len..].copy_from_slice(&plain_signature);
    assert_eq!(verify(&wire).unwrap_err(), CardError::InvalidSignature);
}

#[test]
fn weak_signing_keys_and_weak_signature_points_are_rejected() {
    let mut wire = card("Alice");
    wire[42..74].fill(0);
    wire[42] = 1;
    assert_eq!(verify(&wire).unwrap_err(), CardError::InvalidPublicIdentity);
    wire = card("Alice");
    let start = wire.len() - SIGNATURE_BYTES;
    wire[start..start + 32].fill(0);
    wire[start] = 1;
    assert_eq!(verify(&wire).unwrap_err(), CardError::InvalidSignature);
}

#[test]
fn canonical_uri_and_token_roundtrip_all_legal_lengths() {
    let mut decoded = [0_u8; MAX_CARD_BYTES];
    let mut uri = [0_u8; MAX_URI_BYTES];
    for name_len in 0..=MAX_DISPLAY_NAME_BYTES {
        let wire = card(&"x".repeat(name_len));
        let encoded = token(&wire);
        assert!(!encoded.contains('='));
        let len = decode_token(&encoded, &mut decoded).unwrap();
        assert_eq!(&decoded[..len], &wire);
        let uri_len = encode_uri(&wire, &mut uri).unwrap();
        let uri_text = str::from_utf8(&uri[..uri_len]).unwrap();
        assert_eq!(uri_text, format!("{URI_PREFIX}{encoded}"));
        let len = decode_uri(uri_text, &mut decoded).unwrap();
        assert_eq!(&decoded[..len], &wire);
        assert!(verify(&decoded[..len]).is_ok());
    }
}

#[test]
fn noncanonical_tokens_and_uri_suffixes_are_rejected_without_output_changes() {
    let wire = card(""); // 148 bytes: two trailing base64 digits.
    let encoded = token(&wire);
    assert_eq!(encoded.len() % 4, 2);
    let mut aliases = vec![
        format!("{encoded}="),
        format!(" {encoded}"),
        format!("{encoded}\n"),
    ];
    let mut altered = encoded.as_bytes().to_vec();
    // The final sextet must have four zero low bits; toggling one leaves decoded bytes unchanged.
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let last = altered.len() - 1;
    let digit = alphabet
        .iter()
        .position(|byte| *byte == altered[last])
        .unwrap();
    altered[last] = alphabet[digit | 1];
    aliases.push(String::from_utf8(altered).unwrap());
    let mut output = [0xa5_u8; MAX_CARD_BYTES];
    for alias in aliases {
        assert!(decode_token(&alias, &mut output).is_err());
        assert_eq!(output, [0xa5; MAX_CARD_BYTES]);
    }
    for uri in [
        format!("RNS-CONTACT://v1/{encoded}"),
        format!("rns-contact://v2/{encoded}"),
        format!("{URI_PREFIX}{encoded}/"),
        format!("{URI_PREFIX}{encoded}?x=1"),
        format!("{URI_PREFIX}{encoded}#x"),
        format!("{URI_PREFIX}%50{}", &encoded[1..]),
        format!("https://example.org/{encoded}"),
    ] {
        assert!(decode_uri(&uri, &mut output).is_err(), "{uri}");
        assert_eq!(output, [0xa5; MAX_CARD_BYTES]);
    }
}

#[test]
fn carrier_bounds_and_small_buffers_are_checked_before_writing() {
    let wire = card("Alice");
    let encoded = token(&wire);
    let mut output = [0xa5_u8; MAX_URI_BYTES];
    assert!(encode_token(&wire, &mut output[..1]).is_err());
    assert!(encode_uri(&wire, &mut output[..1]).is_err());
    assert!(decode_token(&encoded, &mut output[..1]).is_err());
    assert!(decode_uri(&format!("{URI_PREFIX}{encoded}"), &mut output[..1]).is_err());
    assert_eq!(output, [0xa5; MAX_URI_BYTES]);
    assert_eq!(
        decode_token("", &mut output),
        Err(TokenError::InvalidLength)
    );
    assert_eq!(
        decode_token(&"A".repeat(MAX_TOKEN_BYTES + 1), &mut output),
        Err(TokenError::TooLarge)
    );
    assert_eq!(
        decode_uri(&"A".repeat(MAX_URI_BYTES + 1), &mut output),
        Err(TokenError::TooLarge)
    );
    assert_eq!(
        encode_token(&[0; MIN_CARD_BYTES - 1], &mut output),
        Err(TokenError::InvalidLength)
    );
    assert_eq!(
        encode_token(&[0; MAX_CARD_BYTES + 1], &mut output),
        Err(TokenError::TooLarge)
    );
}

#[cfg(feature = "alloc")]
#[test]
fn owned_projection_reuses_verified_codec_and_exact_errors() {
    let wire = card_for("Sensor 🌦", "example.telemetry");
    let borrowed = verify(&wire).unwrap();
    let owned = api::verify_contact_card(wire.clone()).unwrap();
    assert_eq!(owned.public_identity, borrowed.public_identity());
    assert_eq!(owned.service_name_hash, borrowed.service_name_hash());
    assert_eq!(owned.display_name, borrowed.display_name());
    assert_eq!(owned.identity_hash, borrowed.identity_hash());
    assert_eq!(owned.destination_hash, borrowed.destination_hash());
    assert_eq!(
        api::service_name_hash("example.telemetry".into()),
        borrowed.service_name_hash()
    );
    let mut changed = wire;
    changed[HEADER_BYTES] ^= 1;
    assert_eq!(
        api::verify_contact_card(changed),
        Err(CardError::InvalidSignature)
    );
    assert_eq!(api::verify_contact_card(vec![]), Err(CardError::Truncated));
}

#[cfg(feature = "alloc")]
#[test]
fn owned_uri_carriers_preserve_canonical_encoding_and_separate_verification() {
    let wire = card("Alice");
    let uri = api::encode_contact_card_uri(wire.clone()).unwrap();
    assert_eq!(uri, format!("{URI_PREFIX}{}", token(&wire)));
    assert_eq!(api::decode_contact_card_uri(uri.clone()).unwrap(), wire);
    for alias in [
        format!("{uri}="),
        format!("{uri}#extra"),
        uri.to_uppercase(),
    ] {
        assert!(api::decode_contact_card_uri(alias).is_err());
    }
    assert_eq!(
        api::encode_contact_card_uri(vec![]),
        Err(TokenError::InvalidLength)
    );
    assert_eq!(
        api::decode_contact_card_uri("x".repeat(MAX_URI_BYTES + 1)),
        Err(TokenError::TooLarge)
    );
    let mut altered = wire;
    altered[HEADER_BYTES] ^= 1;
    let decoded =
        api::decode_contact_card_uri(api::encode_contact_card_uri(altered.clone()).unwrap())
            .unwrap();
    assert_eq!(decoded, altered);
    assert_eq!(
        api::verify_contact_card(decoded),
        Err(CardError::InvalidSignature)
    );
}
