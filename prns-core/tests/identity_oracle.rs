#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use prns_core::crypto::{sealed_len, Ed25519Signature, X25519SecretKey};
use prns_core::identity::{PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use prns_core::routing::announce::{derive_destination_hash, expand_name};
use prns_core::wire::BROADCAST_MDU;

mod oracle_support;

const SEED: u64 = 0x1de7_71a5_cafe_f00d;

struct Generator {
    state: u64,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn byte(&mut self) -> u8 {
        self.state = self
            .state
            .wrapping_mul(2_862_933_555_777_941_757)
            .wrapping_add(3_037_000_493);
        self.state.to_be_bytes()[1]
    }

    fn array<const N: usize>(&mut self) -> [u8; N] {
        core::array::from_fn(|_| self.byte())
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.byte()).collect()
    }
}

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/identity_oracle.py")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert!(hex.len().is_multiple_of(2));
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("valid oracle hex"))
        .collect()
}

fn token_mutations(token: &[u8]) -> Vec<Vec<u8>> {
    let mut mutations = Vec::new();
    for index in [0, 32, 48, token.len() - 1] {
        if index < token.len() {
            let mut corrupted = token.to_vec();
            corrupted[index] ^= 1;
            mutations.push(corrupted);
        }
    }
    for length in [
        0,
        1,
        31,
        32,
        33,
        47,
        48,
        token.len().saturating_sub(33),
        token.len().saturating_sub(32),
        token.len().saturating_sub(1),
    ] {
        if length < token.len() && !mutations.iter().any(|mutation| mutation.len() == length) {
            mutations.push(token[..length].to_vec());
        }
    }
    mutations
}

#[test]
fn stock_rns_and_prns_agree_on_identity_crypto_boundaries() {
    let Some(python) = oracle_support::reference_python(
        "SMOKE_PYTHON",
        "../benchmarks/reference/.venv/bin/python",
    ) else {
        return;
    };
    let mut generator = Generator::new(SEED);
    let secret = generator.array::<IDENTITY_SECRET_KEY_LEN>();
    let wrong_secret = generator.array::<IDENTITY_SECRET_KEY_LEN>();
    let private = PrivateIdentityMaterial::from_bytes(secret);
    let wrong = PrivateIdentityMaterial::from_bytes(wrong_secret);
    let public = private.public();
    let max_plaintext = (0..=BROADCAST_MDU)
        .rev()
        .find(|length| 32 + sealed_len(*length) <= BROADCAST_MDU)
        .expect("one encrypted byte fits the broadcast MDU");
    let lengths = [0, 1, 15, 16, 17, 255, max_plaintext];
    let messages = lengths
        .into_iter()
        .map(|length| generator.bytes(length))
        .collect::<Vec<_>>();
    let ephemeral_secrets = messages
        .iter()
        .map(|_| X25519SecretKey::new(generator.array::<32>()))
        .collect::<Vec<_>>();
    let ivs = messages
        .iter()
        .map(|_| generator.array::<16>())
        .collect::<Vec<_>>();
    let mut rust_tokens = Vec::new();
    let mut request_cases = Vec::new();
    for (index, ((message, ephemeral), iv)) in messages
        .iter()
        .zip(&ephemeral_secrets)
        .zip(&ivs)
        .enumerate()
    {
        let mut token = vec![0; message.len() + 128];
        let written = public
            .encrypt(ephemeral, iv, message, &mut token)
            .expect("oracle token buffer fits");
        token.truncate(written);
        let mutations = token_mutations(&token);
        request_cases.push(serde_json::json!({
            "index": index,
            "message": hex(message),
            "rust_token": hex(&token),
            "mutations": mutations.iter().map(|mutation| hex(mutation)).collect::<Vec<_>>(),
        }));
        rust_tokens.push(token);
    }
    let names = ["personal.node", "rnstransport.remote.management", "mesh.🛰️"];
    let request = serde_json::json!({
        "secret": hex(&secret),
        "wrong_secret": hex(&wrong_secret),
        "names": names,
        "cases": request_cases,
    });
    let response = oracle_support::run_json_oracle(&python, &oracle_script(), &request);
    assert_eq!(response["version"], "1.4.0");
    assert_eq!(response["public"], hex(public.as_bytes()));
    assert_eq!(
        response["identity_hash"],
        hex(private.identity_hash().as_bytes())
    );

    for (index, name) in names.iter().enumerate() {
        let mut parts = name.split('.');
        let app_name = parts.next().expect("name has an app component");
        let aspects = parts.collect::<Vec<_>>();
        let name_hash = expand_name(app_name, &aspects).expect("oracle name is valid");
        let destination = derive_destination_hash(&private.identity_hash(), &name_hash);
        assert_eq!(
            response["names"][index]["name_hash"],
            hex(name_hash.as_bytes())
        );
        assert_eq!(
            response["names"][index]["destination_hash"],
            hex(destination.as_bytes())
        );
    }

    let cases = response["cases"]
        .as_array()
        .expect("identity cases are an array");
    assert_eq!(cases.len(), messages.len());
    for (index, (((message, rust_token), expected), length)) in messages
        .iter()
        .zip(&rust_tokens)
        .zip(cases)
        .zip(lengths)
        .enumerate()
    {
        let signature = private.sign(message);
        assert_eq!(
            expected["signature"],
            hex(&signature.0),
            "seed {SEED:#018x}, case {index}, length {length}"
        );
        assert_eq!(expected["valid"], true);
        assert_eq!(expected["corrupted_valid"], false);
        assert_eq!(expected["wrong_valid"], false);
        assert_eq!(expected["rust_plaintext"], hex(message));
        assert!(expected["rust_mutations_rejected"]
            .as_array()
            .expect("mutation verdicts are an array")
            .iter()
            .all(|verdict| verdict == true));
        assert!(public.verify(message, &signature).is_ok());
        assert!(wrong.public().verify(message, &signature).is_err());

        let python_token = decode_hex(
            expected["python_token"]
                .as_str()
                .expect("Python token is hex"),
        );
        let mut plaintext = vec![0; message.len() + 16];
        let opened = private
            .decrypt(&python_token, &mut plaintext)
            .expect("Rust opens Python token");
        assert_eq!(&plaintext[..opened], message);
        let mut wrong_plaintext = vec![0xa5; message.len() + 32];
        assert!(wrong.decrypt(&python_token, &mut wrong_plaintext).is_err());
        assert!(wrong_plaintext.iter().all(|byte| *byte == 0xa5));
        for mutation in token_mutations(&python_token) {
            let mut partial_plaintext = vec![0x5a; message.len() + 32];
            assert!(private.decrypt(&mutation, &mut partial_plaintext).is_err());
            assert!(partial_plaintext.iter().all(|byte| *byte == 0x5a));
        }

        let mut rust_plaintext = vec![0; message.len() + 16];
        let opened = private
            .decrypt(rust_token, &mut rust_plaintext)
            .expect("Rust opens its deterministic token");
        assert_eq!(&rust_plaintext[..opened], message);
        let decoded_signature: [u8; 64] = decode_hex(
            expected["signature"]
                .as_str()
                .expect("Python signature is hex"),
        )
        .try_into()
        .expect("Python signature has 64 bytes");
        assert!(public
            .verify(message, &Ed25519Signature(decoded_signature))
            .is_ok());
    }
}
