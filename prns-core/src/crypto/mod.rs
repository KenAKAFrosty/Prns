//! Entropy is always an *input*. Nothing here generates randomness, so the engine stays deterministic.

mod exchange;
mod hash;
mod kdf;
mod mac;
pub mod ratchets;
mod sign;
mod token;

pub use exchange::{
    x25519_diffie_hellman, x25519_keys_for_seal, x25519_public_key, X25519PublicKey,
    X25519SecretKey, X25519SharedSecret,
};
pub use hash::{
    sha256, sha256_chunks, sha256_prefix_and_digest_suffix, Sha256PrefixState, SharedPrefixDigests,
};
pub use kdf::{hkdf_sha256, hkdf_sha256_into, HkdfOutputTooLong};
pub use mac::{hmac_sha256, hmac_sha256_verify, HmacSha256Stream, InvalidMac};
pub use sign::{
    ed25519_public_key, ed25519_sign, ed25519_verify, Ed25519PublicKey, Ed25519SecretKey,
    Ed25519Signature, Ed25519Verifier, InvalidPublicKey, InvalidSignature,
};
pub use token::{
    sealed_len, token_is_authentic, token_open, token_open_in_place, token_seal, token_seal_chunks,
    token_seal_in_place, BadKeyLength, BufferTooShort, TokenKey, TokenOpenError, TokenOpenStream,
    TOKEN_OVERHEAD,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_from_hex<const N: usize>(s: &str) -> [u8; N] {
        let mut bytes = [0u8; N];
        assert_eq!(s.len(), N * 2, "the vector spells exactly {N} bytes");
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        bytes
    }

    #[test]
    fn sha256_matches_rns_1_3_5() {
        assert_eq!(
            sha256(b"personal-reticulum-suite"),
            bytes_from_hex("fbf93abb74e7a87e0bb67364e3eddf7718e5f1d38eedf1b21b806a8e612e89d2"),
        );
    }

    #[test]
    fn hmac_sha256_matches_rns_1_3_5_and_verifies_constant_time() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let tag: [u8; 32] =
            bytes_from_hex("aa868925242368f32a02fef52ecf6fcdb07222647c9476e300e848ca886efe2e");
        assert_eq!(hmac_sha256(&key, b"announce-test"), tag);
        assert!(hmac_sha256_verify(&key, b"announce-test", &tag).is_ok());

        let mut bad = tag;
        bad[0] ^= 1;
        assert_eq!(
            hmac_sha256_verify(&key, b"announce-test", &bad),
            Err(InvalidMac),
        );
    }

    #[test]
    fn hkdf_sha256_matches_rns_1_3_5() {
        let ikm = [0x42u8; 32];
        let salt = [0x01u8; 16];
        assert_eq!(
            hkdf_sha256::<32>(&ikm, &salt, b"context"),
            bytes_from_hex("d3a68f6569700c188c5a7c2bcd22c37e9757d022658f06b59753f7c079dcdb3a"),
        );
        assert_eq!(
            hkdf_sha256::<64>(&ikm, &salt, b"context"),
            bytes_from_hex(
                "d3a68f6569700c188c5a7c2bcd22c37e9757d022658f06b59753f7c079dcdb3a\
                 82958b17892dbd30978719b5ba66787152ad0a0c7aeb4df49bce91d36c8915dd"
            ),
        );
    }

    #[test]
    fn ed25519_sign_verify_and_pubkey_match_rns_1_3_5() {
        let secret = Ed25519SecretKey::new([0x11u8; 32]);
        let public = Ed25519PublicKey(bytes_from_hex(
            "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
        ));
        let sig = Ed25519Signature(bytes_from_hex(
            "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a\
             36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
        ));

        assert_eq!(ed25519_public_key(&secret), public);
        assert_eq!(ed25519_sign(&secret, b"sign-this"), sig);
        assert!(ed25519_verify(&public, b"sign-this", &sig).is_ok());

        assert_eq!(
            ed25519_verify(&public, b"sign-thus", &sig),
            Err(InvalidSignature),
        );
        let mut bad = sig;
        bad.0[0] ^= 1;
        assert_eq!(
            ed25519_verify(&public, b"sign-this", &bad),
            Err(InvalidSignature),
        );
    }

    #[test]
    fn ed25519_verifier_matches_the_one_shot_verify() {
        let public = Ed25519PublicKey(bytes_from_hex(
            "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
        ));
        let sig = Ed25519Signature(bytes_from_hex(
            "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a\
             36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
        ));

        let verifier = Ed25519Verifier::new(&public).expect("canonical key");
        assert_eq!(verifier.public_key(), &public);
        assert!(verifier.verify(b"sign-this", &sig).is_ok());
        assert_eq!(verifier.verify(b"sign-thus", &sig), Err(InvalidSignature));
    }

    #[test]
    fn x25519_dh_and_pubkey_match_rns_1_3_5() {
        let a = X25519SecretKey::new([0x22u8; 32]);
        let a_pub = X25519PublicKey(bytes_from_hex(
            "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20",
        ));
        let b_pub = X25519PublicKey(bytes_from_hex(
            "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14",
        ));
        assert_eq!(x25519_public_key(&a), a_pub);
        assert_eq!(
            x25519_diffie_hellman(&a, &b_pub).as_bytes(),
            &bytes_from_hex("1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805"),
        );
    }

    #[test]
    fn token_round_trips_and_matches_rns_1_3_5_aes128() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let plaintext = b"secret payload one-twenty-eight";
        let rns_token = bytes_from_hex::<80>(
            "b0f6eebcf00a7c913d7ea7800390e775afb12b483b2379380d1b6fb5631e1add\
             75ce22bfbc301038bcd42dc15aac9f4be06264c618186e381dfe74e49ef4cb99\
             e861b0c85026daa336876e4b44410d32",
        );

        let mut out = [0u8; 256];
        let n = token_open(&TokenKey::from_aes128(&key), &rns_token, &mut out).unwrap();
        assert_eq!(&out[..n], plaintext);

        let iv = bytes_from_hex("b0f6eebcf00a7c913d7ea7800390e775");
        let mut sealed = [0u8; 256];
        let m = token_seal(&TokenKey::from_aes128(&key), &iv, plaintext, &mut sealed).unwrap();
        assert_eq!(&sealed[..m], &rns_token[..]);
    }

    #[test]
    fn token_matches_rns_1_3_5_aes256() {
        let key: [u8; 64] = core::array::from_fn(|i| i as u8);
        let plaintext = b"secret payload two-fifty-six!!";
        let rns_token = bytes_from_hex::<80>(
            "392b4018bb3fb568466bb35fbfede968eef72be093395e687c3e61c0df992093\
             06c92ab94e39cef8644c44b863cd1582e8bd0178939547a414c1669ee2fa3237\
             a4312dcc3fd9c1177597c454819ce6e7",
        );

        let mut out = [0u8; 256];
        let n = token_open(&TokenKey::from_aes256(&key), &rns_token, &mut out).unwrap();
        assert_eq!(&out[..n], plaintext);

        let iv = bytes_from_hex("392b4018bb3fb568466bb35fbfede968");
        let mut sealed = [0u8; 256];
        let m = token_seal(&TokenKey::from_aes256(&key), &iv, plaintext, &mut sealed).unwrap();
        assert_eq!(&sealed[..m], &rns_token[..]);
    }

    #[test]
    fn token_open_rejects_tampered_mac() {
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let mut token = bytes_from_hex::<80>(
            "b0f6eebcf00a7c913d7ea7800390e775afb12b483b2379380d1b6fb5631e1add\
             75ce22bfbc301038bcd42dc15aac9f4be06264c618186e381dfe74e49ef4cb99\
             e861b0c85026daa336876e4b44410d32",
        );
        *token.last_mut().unwrap() ^= 1;
        let mut out = [0u8; 256];
        assert_eq!(
            token_open(&TokenKey::from_derived(&key).unwrap(), &token, &mut out),
            Err(TokenOpenError::InvalidMac),
        );
    }

    #[test]
    fn a_derived_key_of_unsupported_length_is_refused() {
        assert!(matches!(
            TokenKey::from_derived(&[0u8; 33]),
            Err(BadKeyLength)
        ));
    }
}
