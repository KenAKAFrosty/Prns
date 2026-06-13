use std::hint::black_box;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use personal_rns::crypto::{
    ed25519_public_key, ed25519_sign, ed25519_verify, token_open, token_seal,
    x25519_diffie_hellman, x25519_public_key, Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature,
    TokenKey, X25519PublicKey, X25519SecretKey,
};
use personal_rns::identity::ENCRYPTION_IV_LEN;

const PAYLOAD_LEN: usize = 300;

#[library_benchmark]
fn ed25519_sign_300b() {
    let signer = Ed25519SecretKey::new([0x42; 32]);
    let message = [0xAB_u8; 32];
    black_box(ed25519_sign(black_box(&signer), black_box(&message)));
}

fn verify_inputs() -> (Ed25519PublicKey, [u8; 32], Ed25519Signature) {
    let signer = Ed25519SecretKey::new([0x42; 32]);
    let message = [0xAB_u8; 32];
    let signature = ed25519_sign(&signer, &message);
    (ed25519_public_key(&signer), message, signature)
}

#[library_benchmark]
#[bench::authentic(setup = verify_inputs)]
fn ed25519_verify_300b(input: (Ed25519PublicKey, [u8; 32], Ed25519Signature)) {
    let (verifier, message, signature) = input;
    black_box(
        ed25519_verify(black_box(&verifier), black_box(&message), black_box(&signature))
            .expect("authentic"),
    );
}

#[library_benchmark]
fn x25519_pubkey() {
    let ours = X25519SecretKey::new([0x11; 32]);
    black_box(x25519_public_key(black_box(&ours)));
}

fn dh_inputs() -> (X25519SecretKey, X25519PublicKey) {
    let ours = X25519SecretKey::new([0x11; 32]);
    let theirs = x25519_public_key(&X25519SecretKey::new([0x33; 32]));
    (ours, theirs)
}

#[library_benchmark]
#[bench::pair(setup = dh_inputs)]
fn x25519_dh(input: (X25519SecretKey, X25519PublicKey)) {
    let (ours, theirs) = input;
    black_box(x25519_diffie_hellman(black_box(&ours), black_box(&theirs)));
}

#[library_benchmark]
fn token_seal_300b() {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let iv = [0x77_u8; ENCRYPTION_IV_LEN];
    let plaintext = [0xAB_u8; PAYLOAD_LEN];
    let mut out = [0u8; 512];
    black_box(
        token_seal(black_box(&key), black_box(&iv), black_box(&plaintext), &mut out)
            .expect("seals"),
    );
}

fn sealed_token() -> Vec<u8> {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let iv = [0x77_u8; ENCRYPTION_IV_LEN];
    let plaintext = [0xAB_u8; PAYLOAD_LEN];
    let mut sealed = [0u8; 512];
    let n = token_seal(&key, &iv, &plaintext, &mut sealed).expect("seals");
    sealed[..n].to_vec()
}

#[library_benchmark]
#[bench::b300(setup = sealed_token)]
fn token_open_300b(sealed: Vec<u8>) {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let mut out = [0u8; 512];
    black_box(token_open(black_box(&key), black_box(&sealed), &mut out).expect("opens"));
}

library_benchmark_group!(
    name = primitives;
    benchmarks =
        ed25519_sign_300b,
        ed25519_verify_300b,
        x25519_pubkey,
        x25519_dh,
        token_seal_300b,
        token_open_300b
);

main!(library_benchmark_groups = primitives);
