//! The sender's preparation — RNS 1.3.1 `Resource.__init__`'s transmit path.
//! Choose the smaller of the plaintext and its compressed candidate, seal
//! stream nonce ‖ stream under the session key in one token, slice the
//! ciphertext at the link's SDU, and name every part by its salted map hash,
//! re-rolling the salt nonce until no two names collide within the guard
//! span. Compression itself stays outside the engine: the host hands in a
//! bz2 candidate (or nothing) and this module applies the reference's
//! keep-only-if-smaller rule. One deliberate divergence: on a collision
//! re-roll the reference recomputes its resource hash from a stale loop
//! variable (a latent corruption); we recompute from the true plaintext —
//! same wire shape, correct bytes. And where the reference re-rolls forever,
//! [`SALT_REROLL_CAP`] bounds the loop.

use crate::crypto::CryptoError;
use crate::routing::links::resources::{
    map_hash, map_hash_name_word, ResourceCompression, ResourceHash, ResourceProof, SaltNonce,
    COLLISION_GUARD_SIZE, MAP_HASH_LEN, MAX_EFFICIENT_SIZE, RESOURCE_NONCE_LEN,
};
use crate::routing::links::LinkKey;

/// How many salt nonces the build will try before giving up. A real
/// collision within the guard span is a ~5-in-a-million event per resource,
/// so two iterations are already rare; eight failures mean something is
/// deeply wrong with the entropy source.
pub const SALT_REROLL_CAP: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutgoingResourceError {
    DataTooLarge,
    SduTooSmall,
    Seal(CryptoError),
    HashmapBufferTooShort,
    SaltRerollsExhausted,
}

/// Everything the advertisement and the serving state machine need to know
/// about a freshly sealed transfer: the ciphertext sits in the caller's
/// transfer buffer, its part names in the caller's hashmap buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltResource {
    pub sealed_transfer_len: usize,
    pub part_count: usize,
    pub hash: ResourceHash,
    pub salt_nonce: SaltNonce,
    pub expected_proof: ResourceProof,
    pub compression: ResourceCompression,
    pub uncompressed_data_len: u64,
}

/// `fresh_nonce` is drawn once for the stream nonce, then once per salt
/// attempt — the order the reference draws its `random_hash`es.
#[allow(clippy::too_many_arguments)]
pub fn build_outgoing_resource(
    plaintext: &[u8],
    compressed_candidate: Option<&[u8]>,
    key: &LinkKey,
    seal_iv: &[u8; 16],
    mut fresh_nonce: impl FnMut() -> [u8; RESOURCE_NONCE_LEN],
    sdu: usize,
    transfer: &mut [u8],
    hashmap: &mut [u8],
) -> Result<BuiltResource, BuildOutgoingResourceError> {
    if plaintext.len() > MAX_EFFICIENT_SIZE {
        return Err(BuildOutgoingResourceError::DataTooLarge);
    }
    if sdu == 0 {
        return Err(BuildOutgoingResourceError::SduTooSmall);
    }
    let (stream, compression) = match compressed_candidate {
        Some(candidate) if candidate.len() < plaintext.len() => {
            (candidate, ResourceCompression::Bz2)
        }
        _ => (plaintext, ResourceCompression::Uncompressed),
    };
    let stream_nonce = fresh_nonce();
    let sealed_transfer_len = key
        .seal_chunks(seal_iv, &[&stream_nonce, stream], transfer)
        .map_err(BuildOutgoingResourceError::Seal)?;
    let part_count = sealed_transfer_len.div_ceil(sdu);
    let hashmap_len = part_count * MAP_HASH_LEN;
    if hashmap.len() < hashmap_len {
        return Err(BuildOutgoingResourceError::HashmapBufferTooShort);
    }

    let sealed = &transfer[..sealed_transfer_len];
    for _ in 0..SALT_REROLL_CAP {
        let salt_nonce = SaltNonce::new(fresh_nonce());
        if !write_hashmap_without_collision(sealed, sdu, &salt_nonce, &mut hashmap[..hashmap_len]) {
            continue;
        }
        let (hash, expected_proof) =
            crate::crypto::sha256_prefix_and_digest_suffix(plaintext, salt_nonce.as_bytes());
        let hash = ResourceHash::new(hash);
        let expected_proof = ResourceProof::new(expected_proof);
        return Ok(BuiltResource {
            sealed_transfer_len,
            part_count,
            hash,
            salt_nonce,
            expected_proof,
            compression,
            uncompressed_data_len: plaintext.len() as u64,
        });
    }
    Err(BuildOutgoingResourceError::SaltRerollsExhausted)
}

fn write_hashmap_without_collision(
    sealed: &[u8],
    sdu: usize,
    salt_nonce: &SaltNonce,
    hashmap: &mut [u8],
) -> bool {
    for (index, part) in sealed.chunks(sdu).enumerate() {
        let name = map_hash(part, salt_nonce);
        let name_word = u32::from_ne_bytes(name);
        let offset = index * MAP_HASH_LEN;
        let guard_start = index.saturating_sub(COLLISION_GUARD_SIZE);
        for previous in guard_start..index {
            let previous_offset = previous * MAP_HASH_LEN;
            if map_hash_name_word(&hashmap[previous_offset..previous_offset + MAP_HASH_LEN])
                == name_word
            {
                return false;
            }
        }
        hashmap[offset..offset + MAP_HASH_LEN].copy_from_slice(&name);
    }
    true
}

/// RNS 1.3.1's collision guard: within any [`COLLISION_GUARD_SIZE`]-wide run
/// of consecutive parts, every map hash must be unique — that is the span a
/// part request's search scope covers on the serving side.
pub fn hashmap_has_collision(hashmap: &[u8]) -> bool {
    let count = hashmap.len() / MAP_HASH_LEN;
    for i in 1..count {
        let name = map_hash_name_word(&hashmap[i * MAP_HASH_LEN..(i + 1) * MAP_HASH_LEN]);
        for j in i.saturating_sub(COLLISION_GUARD_SIZE)..i {
            if map_hash_name_word(&hashmap[j * MAP_HASH_LEN..(j + 1) * MAP_HASH_LEN]) == name {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{sha256, x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::routing::links::resources::resource_sdu;
    use crate::routing::links::LinkId;
    use crate::wire::BROADCAST_MTU;

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const SEAL_IV: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
    const STREAM_NONCE: [u8; 4] = [0x51, 0x52, 0x53, 0x54];
    const SALT_NONCE: [u8; 4] = [0x61, 0x62, 0x63, 0x64];

    fn link_key() -> LinkKey {
        let scalar: [u8; 32] = hx(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = hx(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        let id: [u8; 16] = hx(LINK_ID).try_into().unwrap();
        LinkKey::derive(&LinkId::new(id), &shared)
    }

    fn seal_iv() -> [u8; 16] {
        hx(SEAL_IV).try_into().unwrap()
    }

    fn reference_nonces() -> impl FnMut() -> [u8; RESOURCE_NONCE_LEN] {
        let mut drawn = 0;
        move || {
            drawn += 1;
            if drawn == 1 {
                STREAM_NONCE
            } else {
                SALT_NONCE
            }
        }
    }

    // The reference Resource.__init__ driven with the link-key fixture, IV
    // a1..b0, stream nonce 51525354, salt nonce 61626364, sdu 464:
    // b"reticulum resources ride the link " * 40 compresses 1360 -> 90, so
    // the sealed transfer is one 144-byte part of bz2 stream.
    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";
    const CASE1_TRANSFER: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0defc0c57b1784ccf967b5ab8efcbe06b0b6c4fe844b2554e531ab7cbd377415a772be5265099b6b4d9102c0ca2b7184be789bb29d8617a35f08f0810171beb7b615ba3c5c60810ba046119b8ffe42de2218706a22d5d893b991b29be5a5b7788495f7d2c51e42654baa24f39299dd48a374478cabd51e2054adbfbc3eac545d8";
    const CASE1_HASH: &str = "cc19201919749bd48f17ff5c4fd3052bf4015fb4178c347e8fafa18c624e3c7f";
    const CASE1_PROOF: &str = "5492f2c5809189bfd9cd4efe9c57c78519234af697bc3201d3a777b73ad4673d";
    const CASE1_HASHMAP: &str = "973c9707";

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    // 1500 bytes of sha256 chain don't compress (bz2 expands them to 1894),
    // so the reference keeps the plaintext: 4 parts of sealed stream.
    const CASE2_HASH: &str = "16803340bc7814bb85782757a9536707e001721c35388473af520c96593c7e02";
    const CASE2_PROOF: &str = "3b77466441207be41b72281df866f4dd3780ff2a8ff68c4c22aabd35975070ae";
    const CASE2_HASHMAP: &str = "527829e4e1709b939061f04341a61956";
    const CASE2_TRANSFER_HEAD: &str =
        "a1a2a3a4a5a6a7a8a9aaabacadaeafb085bc2785fad9630734af94fab15b1aff";
    const CASE2_TRANSFER_TAIL: &str = "656b0e6646cc8227cc94da";

    fn case2_plaintext() -> std::vec::Vec<u8> {
        let mut seed = sha256(b"prns-resources");
        let mut data = std::vec::Vec::new();
        for _ in 0..47 {
            data.extend_from_slice(&seed);
            seed = sha256(&seed);
        }
        data.truncate(1_500);
        data
    }

    #[test]
    fn a_compressible_resource_builds_byte_identical_to_the_reference() {
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);
        let mut transfer = [0u8; 512];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &plaintext,
            Some(&candidate),
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Bz2);
        assert_eq!(built.sealed_transfer_len, 144);
        assert_eq!(built.part_count, 1);
        assert_eq!(built.uncompressed_data_len, 1_360);
        assert_eq!(
            &transfer[..built.sealed_transfer_len],
            &hx(CASE1_TRANSFER)[..]
        );
        assert_eq!(built.hash.as_bytes(), &hx(CASE1_HASH)[..]);
        assert_eq!(built.expected_proof.as_bytes(), &hx(CASE1_PROOF)[..]);
        assert_eq!(built.salt_nonce, SaltNonce::new(SALT_NONCE));
        assert_eq!(&hashmap[..MAP_HASH_LEN], &hx(CASE1_HASHMAP)[..]);
    }

    #[test]
    fn an_incompressible_resource_rejects_its_candidate_like_the_reference() {
        let plaintext = case2_plaintext();
        let expanding_candidate = std::vec![0u8; 1_894];
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &plaintext,
            Some(&expanding_candidate),
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Uncompressed);
        assert_eq!(built.sealed_transfer_len, 1_568);
        assert_eq!(built.part_count, 4);
        assert_eq!(built.uncompressed_data_len, 1_500);
        assert_eq!(&transfer[..32], &hx(CASE2_TRANSFER_HEAD)[..]);
        assert_eq!(
            &transfer[built.sealed_transfer_len - 11..built.sealed_transfer_len],
            &hx(CASE2_TRANSFER_TAIL)[..]
        );
        assert_eq!(built.hash.as_bytes(), &hx(CASE2_HASH)[..]);
        assert_eq!(built.expected_proof.as_bytes(), &hx(CASE2_PROOF)[..]);
        assert_eq!(&hashmap[..4 * MAP_HASH_LEN], &hx(CASE2_HASHMAP)[..]);
    }

    #[test]
    fn the_resource_hash_ignores_compression_entirely() {
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let with = build_outgoing_resource(
            &plaintext,
            Some(&candidate),
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        let without = build_outgoing_resource(
            &plaintext,
            None,
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        assert_eq!(without.compression, ResourceCompression::Uncompressed);
        assert_eq!(with.hash, without.hash);
        assert_eq!(with.expected_proof, without.expected_proof);
        assert_ne!(with.sealed_transfer_len, without.sealed_transfer_len);
    }

    #[test]
    fn single_byte_parts_collide_until_the_reroll_cap_gives_up() {
        let plaintext = case1_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 8_192];
        let mut drawn = 0u32;
        let result = build_outgoing_resource(
            &plaintext,
            None,
            &link_key(),
            &seal_iv(),
            move || {
                drawn += 1;
                drawn.to_be_bytes()
            },
            1,
            &mut transfer,
            &mut hashmap,
        );
        assert_eq!(
            result.unwrap_err(),
            BuildOutgoingResourceError::SaltRerollsExhausted,
        );
    }

    #[test]
    fn the_collision_guard_sees_the_reference_span_and_no_further() {
        let mut clean = std::vec::Vec::new();
        for i in 0u32..300 {
            clean.extend_from_slice(&i.to_be_bytes());
        }
        assert!(!hashmap_has_collision(&clean));

        let mut near = clean.clone();
        near[8..12].copy_from_slice(&0u32.to_be_bytes());
        assert!(hashmap_has_collision(&near));

        let mut past_guard = clean.clone();
        let far = (COLLISION_GUARD_SIZE + 1) * MAP_HASH_LEN;
        past_guard[far..far + 4].copy_from_slice(&0u32.to_be_bytes());
        assert!(!hashmap_has_collision(&past_guard));

        let mut at_guard_edge = clean;
        let edge = COLLISION_GUARD_SIZE * MAP_HASH_LEN;
        at_guard_edge[edge..edge + 4].copy_from_slice(&0u32.to_be_bytes());
        assert!(hashmap_has_collision(&at_guard_edge));
    }

    #[test]
    fn buffer_and_size_guards_refuse() {
        let plaintext = case1_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        assert_eq!(
            build_outgoing_resource(
                &plaintext,
                None,
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                0,
                &mut transfer,
                &mut hashmap,
            )
            .unwrap_err(),
            BuildOutgoingResourceError::SduTooSmall,
        );
        assert_eq!(
            build_outgoing_resource(
                &plaintext,
                None,
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                &mut transfer[..64],
                &mut hashmap,
            )
            .unwrap_err(),
            BuildOutgoingResourceError::Seal(CryptoError::BufferTooShort),
        );
        assert_eq!(
            build_outgoing_resource(
                &plaintext,
                None,
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                &mut transfer,
                &mut hashmap[..4],
            )
            .unwrap_err(),
            BuildOutgoingResourceError::HashmapBufferTooShort,
        );
        let huge = std::vec![0u8; MAX_EFFICIENT_SIZE + 1];
        assert_eq!(
            build_outgoing_resource(
                &huge,
                None,
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                &mut transfer,
                &mut hashmap,
            )
            .unwrap_err(),
            BuildOutgoingResourceError::DataTooLarge,
        );
    }

    #[test]
    fn the_sdu_arithmetic_matches_the_reference() {
        assert_eq!(resource_sdu(BROADCAST_MTU), 464);
        assert_eq!(resource_sdu(BROADCAST_MTU), crate::wire::MDU);
    }
}
