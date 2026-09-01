// A malformed pinned corpus is a broken test artifact, not a recoverable production condition.
#![allow(clippy::expect_used, clippy::panic)]

use prns_core::crypto::{
    x25519_diffie_hellman, Ed25519PublicKey, Ed25519Signature, Ed25519Verifier, X25519PublicKey,
    X25519SecretKey,
};
use serde_json::Value;

const ED25519_CORPUS: &str = include_str!("vectors/wycheproof_ed25519_dac1dd47.json");
const X25519_CORPUS: &str = include_str!("vectors/wycheproof_x25519_dac1dd47.json");

fn hex_vec(raw: &str) -> Vec<u8> {
    assert_eq!(raw.len() % 2, 0, "complete hex bytes");
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = core::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid vector hex")
        })
        .collect()
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .get(name)
        .unwrap_or_else(|| panic!("Wycheproof value has {name}"))
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    field(value, name)
        .as_str()
        .unwrap_or_else(|| panic!("Wycheproof {name} is text"))
}

fn test_id(test: &Value) -> u64 {
    field(test, "tcId")
        .as_u64()
        .expect("Wycheproof tcId is an integer")
}

#[test]
fn ed25519_verification_matches_pinned_wycheproof_corpus() {
    let corpus: Value = serde_json::from_str(ED25519_CORPUS).expect("pinned corpus is JSON");
    assert_eq!(field(&corpus, "numberOfTests").as_u64(), Some(151));
    assert_eq!(text(&corpus, "schema"), "eddsa_verify_schema_v1.json");

    let mut valid = 0usize;
    let mut invalid = 0usize;
    for group in field(&corpus, "testGroups")
        .as_array()
        .expect("Wycheproof testGroups is an array")
    {
        let public = text(field(group, "publicKey"), "pk");
        let verifier = <[u8; Ed25519PublicKey::LEN]>::try_from(hex_vec(public))
            .ok()
            .and_then(|public| Ed25519Verifier::new(&Ed25519PublicKey(public)).ok());

        for test in field(group, "tests")
            .as_array()
            .expect("Wycheproof tests is an array")
        {
            let message = hex_vec(text(test, "msg"));
            let signature = <[u8; Ed25519Signature::LEN]>::try_from(hex_vec(text(test, "sig")))
                .ok()
                .map(Ed25519Signature);
            let accepted = verifier
                .as_ref()
                .zip(signature.as_ref())
                .is_some_and(|(verifier, signature)| verifier.verify(&message, signature).is_ok());

            match text(test, "result") {
                "valid" => {
                    valid += 1;
                    assert!(accepted, "Wycheproof Ed25519 tcId {}", test_id(test));
                }
                "invalid" => {
                    invalid += 1;
                    assert!(!accepted, "Wycheproof Ed25519 tcId {}", test_id(test));
                }
                result => panic!("unsupported Ed25519 result {result}"),
            }
        }
    }
    assert_eq!((valid, invalid), (88, 63));
}

#[test]
fn x25519_agreement_matches_pinned_wycheproof_corpus() {
    let corpus: Value = serde_json::from_str(X25519_CORPUS).expect("pinned corpus is JSON");
    assert_eq!(field(&corpus, "numberOfTests").as_u64(), Some(518));
    assert_eq!(text(&corpus, "schema"), "xdh_comp_schema_v1.json");

    let mut valid = 0usize;
    let mut acceptable = 0usize;
    for group in field(&corpus, "testGroups")
        .as_array()
        .expect("Wycheproof testGroups is an array")
    {
        assert_eq!(text(group, "curve"), "curve25519");
        for test in field(group, "tests")
            .as_array()
            .expect("Wycheproof tests is an array")
        {
            let secret = X25519SecretKey::new(
                hex_vec(text(test, "private"))
                    .try_into()
                    .expect("X25519 private input is 32 bytes"),
            );
            let public = X25519PublicKey(
                hex_vec(text(test, "public"))
                    .try_into()
                    .expect("X25519 public input is 32 bytes"),
            );
            let expected: [u8; 32] = hex_vec(text(test, "shared"))
                .try_into()
                .expect("X25519 shared output is 32 bytes");
            assert_eq!(
                x25519_diffie_hellman(&secret, &public).as_bytes(),
                &expected,
                "Wycheproof X25519 tcId {}",
                test_id(test)
            );

            match text(test, "result") {
                "valid" => valid += 1,
                "acceptable" => acceptable += 1,
                result => panic!("unsupported X25519 result {result}"),
            }
        }
    }
    assert_eq!((valid, acceptable), (264, 254));
}
