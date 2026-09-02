//! Standards vectors and verification-policy invariants for the curve wrappers.

use super::*;

struct Ed25519Vector {
    secret: [u8; Ed25519SecretKey::LEN],
    public: Ed25519PublicKey,
    message: Vec<u8>,
    signature: Ed25519Signature,
}

fn hex_vec(raw: &str) -> Vec<u8> {
    let hex: Vec<_> = raw
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    assert_eq!(hex.len() % 2, 0, "hex vector has complete bytes: {raw:?}");
    let (pairs, remainder) = hex.as_chunks::<2>();
    assert!(
        remainder.is_empty(),
        "hex vector has complete bytes: {raw:?}"
    );
    pairs
        .iter()
        .map(|pair| {
            let pair = core::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid hex")
        })
        .collect()
}

fn hex_array<const N: usize>(raw: &str) -> [u8; N] {
    hex_vec(raw).try_into().unwrap_or_else(|bytes: Vec<u8>| {
        panic!("vector spells {} bytes instead of {N}", bytes.len())
    })
}

fn rfc_8032_ed25519_vectors() -> Vec<Ed25519Vector> {
    // RFC 8032 section 7.1, the pure Ed25519 vectors whose messages fit compactly in source.
    // https://www.rfc-editor.org/rfc/rfc8032.html#section-7.1
    [
        (
            "9d61b19deffd5a60ba844af492ec2cc4\
             4449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a\
             0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a\
             84877f1eb8e5d974d873e06522490155\
             5fb8821590a33bacc61e39701cf9b46b\
             d25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f\
             5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc\
             9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540\
             a2b27b5416503f8fb3762223ebdb69da\
             085ac1e43e15996e458f3613d0f11d8c\
             387b2eaeb4302aeeb00d291612bb0c00",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b1\
             66d38535076f094b85ce3a2e0b4458f7",
            "fc51cd8e6218a1a38da47ed00230f058\
             0816ed13ba3303ac5deb911548908025",
            "af82",
            "6291d657deec24024827e69c3abe01a3\
             0ce548a284743a445e3680d7db5ac3ac\
             18ff9b538d16f290ae67f760984dc659\
             4a7c15e9716ed28dc027beceea1ec40a",
        ),
        (
            "833fe62409237b9d62ec77587520911e\
             9a759cec1d19755b7da901b96dca3d42",
            "ec172b93ad5e563bf4932c70e1245034\
             c35467ef2efd4d64ebf819683467e2bf",
            "ddaf35a193617abacc417349ae204131\
             12e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd\
             454d4423643ce80e2a9ac94fa54ca49f",
            "dc2a4459e7369633a52b1bf277839a00\
             201009a3efbf3ecb69bea2186c26b589\
             09351fc9ac90b3ecfdfbc7c66431e030\
             3dca179c138ac17ad9bef1177331a704",
        ),
    ]
    .into_iter()
    .map(|(secret, public, message, signature)| Ed25519Vector {
        secret: hex_array(secret),
        public: Ed25519PublicKey(hex_array(public)),
        message: hex_vec(message),
        signature: Ed25519Signature(hex_array(signature)),
    })
    .collect()
}

#[test]
fn ed25519_matches_rfc_8032_pure_signature_vectors() {
    for (index, vector) in rfc_8032_ed25519_vectors().into_iter().enumerate() {
        let secret = Ed25519SecretKey::new(vector.secret);
        assert_eq!(
            ed25519_public_key(&secret),
            vector.public,
            "RFC 8032 vector {index} public key"
        );
        assert_eq!(
            ed25519_sign(&secret, &vector.message),
            vector.signature,
            "RFC 8032 vector {index} signature"
        );
        assert_eq!(
            ed25519_verify(&vector.public, &vector.message, &vector.signature),
            Ok(()),
            "RFC 8032 vector {index} verifies"
        );
    }
}

#[test]
fn x25519_matches_rfc_7748_function_vectors() {
    // RFC 7748 section 5.2.
    // https://www.rfc-editor.org/rfc/rfc7748.html#section-5.2
    for (scalar, coordinate, expected) in [
        (
            "a546e36bf0527c9d3b16154b82465edd\
             62144c0ac1fc5a18506a2244ba449ac4",
            "e6db6867583030db3594c1a424b15f7c\
             726624ec26b3353b10a903a6d0ab1c4c",
            "c3da55379de9c6908e94ea4df28d084f\
             32eccf03491c71f754b4075577a28552",
        ),
        (
            "4b66e9d4d1b4673c5ad22691957d6af5\
             c11b6421e0ea01d42ca4169e7918ba0d",
            "e5210f12786811d3f4b7959d0538ae2c\
             31dbe7106fc03c3efc4cd549c715a493",
            "95cbde9476e8907d7aade45cb4b873f8\
             8b595a68799fa152e6f8f7647aac7957",
        ),
    ] {
        let secret = X25519SecretKey::new(hex_array(scalar));
        let public = X25519PublicKey(hex_array(coordinate));
        assert_eq!(
            x25519_diffie_hellman(&secret, &public).as_bytes(),
            &hex_array::<32>(expected)
        );
    }
}

#[test]
fn x25519_matches_rfc_7748_diffie_hellman_vector() {
    // RFC 7748 section 6.1.
    let alice = X25519SecretKey::new(hex_array(
        "77076d0a7318a57d3c16c17251b26645\
         df4c2f87ebc0992ab177fba51db92c2a",
    ));
    let bob = X25519SecretKey::new(hex_array(
        "5dab087e624a8a4b79e17f8b83800ee6\
         6f3bb1292618b6fd1c2f8b27ff88e0eb",
    ));
    let alice_public = X25519PublicKey(hex_array(
        "8520f0098930a754748b7ddcb43ef75a0\
         dbf3a0d26381af4eba4a98eaa9b4e6a",
    ));
    let bob_public = X25519PublicKey(hex_array(
        "de9edb7d7b7dc1b4d35b61c2ece4353\
         73f8343c85b78674dadfc7e146f882b4f",
    ));
    let shared = hex_array(
        "4a5d9d5ba4ce2de1728e3bf480350f25\
         e07e21c947d19e3376f09b3c1e161742",
    );

    assert_eq!(x25519_public_key(&alice), alice_public);
    assert_eq!(x25519_public_key(&bob), bob_public);
    assert_eq!(
        x25519_diffie_hellman(&alice, &bob_public).as_bytes(),
        &shared
    );
    assert_eq!(
        x25519_diffie_hellman(&bob, &alice_public).as_bytes(),
        &shared
    );
}

#[test]
fn x25519_matches_rfc_7748_iterated_vector() {
    let initial = {
        let mut bytes = [0u8; X25519SecretKey::LEN];
        bytes[0] = 9;
        bytes
    };
    let mut scalar = initial;
    let mut coordinate = initial;
    for iteration in 1..=1_000 {
        let prior_scalar = scalar;
        scalar =
            *x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(coordinate))
                .as_bytes();
        coordinate = prior_scalar;
        if iteration == 1 {
            assert_eq!(
                scalar,
                hex_array(
                    "422c8e7a6227d7bca1350b3e2bb7279f\
                     7897b87bb6854b783c60e80311ae3079"
                )
            );
        }
    }
    assert_eq!(
        scalar,
        hex_array(
            "684cf59ba83309552800ef566f2f4d3c1\
             c3887c49360e3875f2eb94d99532c51"
        )
    );
}

#[cfg(feature = "ed25519-batch")]
#[test]
fn batch_verification_matches_every_individual_rfc_8032_verdict() {
    let vectors = rfc_8032_ed25519_vectors();
    let verifiers: Vec<_> = vectors
        .iter()
        .map(|vector| Ed25519Verifier::new(&vector.public).expect("RFC public key"))
        .collect();
    let verifier_refs: Vec<_> = verifiers.iter().collect();
    let messages: Vec<_> = vectors
        .iter()
        .map(|vector| vector.message.as_slice())
        .collect();
    let signatures: Vec<_> = vectors.iter().map(|vector| vector.signature).collect();

    assert_eq!(
        ed25519_verify_batch(&messages, &signatures, &verifier_refs),
        Ok(())
    );
    assert!(verifiers.iter().all(|verifier| !verifier.is_weak()));

    for corrupted in 0..signatures.len() {
        let mut signatures = signatures.clone();
        signatures[corrupted].0[corrupted] ^= 1;
        assert_eq!(
            ed25519_verify_batch(&messages, &signatures, &verifier_refs),
            Err(InvalidSignature),
            "corruption at batch position {corrupted}"
        );
        for (index, ((message, signature), verifier)) in messages
            .iter()
            .zip(&signatures)
            .zip(&verifier_refs)
            .enumerate()
        {
            assert_eq!(
                verifier.verify(message, signature),
                if index == corrupted {
                    Err(InvalidSignature)
                } else {
                    Ok(())
                }
            );
        }
    }

    assert_eq!(
        ed25519_verify_batch(&messages[..3], &signatures, &verifier_refs),
        Err(InvalidSignature)
    );
    assert_eq!(
        ed25519_verify_batch(&messages, &signatures[..3], &verifier_refs),
        Err(InvalidSignature)
    );
    assert_eq!(
        ed25519_verify_batch(&messages, &signatures, &verifier_refs[..3]),
        Err(InvalidSignature)
    );
}

#[cfg(feature = "ed25519-batch")]
#[test]
fn low_order_public_keys_are_identified_before_batching() {
    let mut identity = [0u8; Ed25519PublicKey::LEN];
    identity[0] = 1;
    let verifier = Ed25519Verifier::new(&Ed25519PublicKey(identity)).expect("identity point");
    assert!(verifier.is_weak());
    assert_eq!(
        ed25519_verify_batch(
            &[b"weak-key input"],
            &[Ed25519Signature([0u8; Ed25519Signature::LEN])],
            &[&verifier],
        ),
        Err(InvalidSignature)
    );
}

#[cfg(feature = "ed25519-batch")]
#[test]
fn batch_verification_matches_individual_verdicts_across_deterministic_inputs() {
    for round in 0usize..96 {
        let batch_len = 2 + round % 7;
        let mut messages = Vec::with_capacity(batch_len);
        let mut publics = Vec::with_capacity(batch_len);
        let mut signatures = Vec::with_capacity(batch_len);
        for job in 0..batch_len {
            let seed = core::array::from_fn(|byte| {
                (round as u8)
                    .wrapping_mul(29)
                    .wrapping_add((job as u8).wrapping_mul(71))
                    .wrapping_add((byte as u8).wrapping_mul(13))
                    .wrapping_add(1)
            });
            let message_len = 1 + (round * 37 + job * 53) % 511;
            let message: Vec<_> = (0..message_len)
                .map(|byte| {
                    (byte as u8)
                        .wrapping_mul(17)
                        .wrapping_add(round as u8)
                        .wrapping_add(job as u8)
                })
                .collect();
            let secret = Ed25519SecretKey::new(seed);
            publics.push(ed25519_public_key(&secret));
            signatures.push(ed25519_sign(&secret, &message));
            messages.push(message);
        }
        let verifiers: Vec<_> = publics
            .iter()
            .map(|public| Ed25519Verifier::new(public).expect("generated public key"))
            .collect();
        let verifier_refs: Vec<_> = verifiers.iter().collect();
        let message_refs: Vec<_> = messages.iter().map(Vec::as_slice).collect();

        assert_eq!(
            ed25519_verify_batch(&message_refs, &signatures, &verifier_refs),
            Ok(()),
            "round {round} valid batch"
        );
        for corrupted in 0..batch_len {
            let mut corrupted_signatures = signatures.clone();
            let byte = (round + corrupted * 11) % Ed25519Signature::LEN;
            corrupted_signatures[corrupted].0[byte] ^= 1;
            assert_eq!(
                ed25519_verify_batch(&message_refs, &corrupted_signatures, &verifier_refs),
                Err(InvalidSignature),
                "round {round} corruption {corrupted}"
            );
            for (index, ((message, signature), verifier)) in message_refs
                .iter()
                .zip(&corrupted_signatures)
                .zip(&verifier_refs)
                .enumerate()
            {
                assert_eq!(
                    verifier.verify(message, signature),
                    if index == corrupted {
                        Err(InvalidSignature)
                    } else {
                        Ok(())
                    },
                    "round {round} individual verdict {index}"
                );
            }
        }
    }
}
