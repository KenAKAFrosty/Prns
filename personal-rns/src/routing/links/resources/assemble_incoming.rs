//! The receiver's mirror of [`build_outgoing`](super::build_outgoing).
//! RNS 1.3.1 `Resource.receive_part`/`assemble`/`prove`

use crate::crypto::{sha256_prefix_and_digest_suffix, CryptoError};
use crate::routing::links::resources::{
    map_hash, ResourceHash, ResourceProof, SaltNonce, MAP_HASH_LEN, RESOURCE_NONCE_LEN,
};
use crate::routing::links::LinkKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTransferError {
    Open(CryptoError),
    StreamTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyResourceError {
    HashMismatch,
}

/// RNS 1.3.1 `Resource.assemble`, the half the engine owns: open the joined
/// parts with the link key in place and strip the stream nonce. What remains
/// is the stream the sender sealed — the bz2 stream when the advertisement's
/// compressed flag is set, the plaintext otherwise. A stream too short to
/// carry even its nonce is refused by name where the reference would slice
/// it empty and fail the hash check later.
pub fn open_transfer<'t>(
    key: &LinkKey,
    transfer: &'t mut [u8],
) -> Result<&'t [u8], OpenTransferError> {
    let stream = key
        .open_in_place(transfer)
        .map_err(OpenTransferError::Open)?;
    if stream.len() < RESOURCE_NONCE_LEN {
        return Err(OpenTransferError::StreamTooShort);
    }
    Ok(&stream[RESOURCE_NONCE_LEN..])
}

/// The hash check closing RNS 1.3.1 `Resource.assemble`, and `prove` in the
/// same breath: the assembled plaintext is genuine when
/// `full_hash(data ‖ salt nonce)` equals the advertised hash, and the receipt
/// the receiver sends back is `full_hash(data ‖ hash)`. A mismatch is the
/// reference's CORRUPT verdict.
pub fn verify_and_prove(
    plaintext: &[u8],
    salt_nonce: &SaltNonce,
    advertised: &ResourceHash,
) -> Result<ResourceProof, VerifyResourceError> {
    let (hash, proof) = sha256_prefix_and_digest_suffix(plaintext, salt_nonce.as_bytes());
    if &hash != advertised.as_bytes() {
        return Err(VerifyResourceError::HashMismatch);
    }
    Ok(ResourceProof::new(proof))
}

/// RNS 1.3.1 `Resource.receive_part`: a part arrives carrying no index, so
/// the receiver names it with [`map_hash`] and scans the known part names
/// from the consecutive-completed height (clamped to zero) through the
/// current window. `None` is a part that matches nothing there — dropped
/// silently, exactly as the reference drops it.
pub fn match_part_in_window(
    part: &[u8],
    salt_nonce: &SaltNonce,
    hashmap: &[u8],
    scan_from: usize,
    window: usize,
) -> Option<usize> {
    let name = map_hash(part, salt_nonce);
    let known = hashmap.len() / MAP_HASH_LEN;
    let end = scan_from.saturating_add(window).min(known);
    (scan_from..end).find(|&i| hashmap[i * MAP_HASH_LEN..(i + 1) * MAP_HASH_LEN] == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{sha256, x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::routing::links::resources::build_outgoing::build_outgoing_resource;
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

    // The same reference-generated vectors build_outgoing proves itself
    // against: the sealed transfer must open back to exactly the bz2 stream
    // the reference compressed, and the decompressed plaintext must verify
    // to the advertised hash and yield the reference proof.
    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";
    const CASE1_TRANSFER: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0defc0c57b1784ccf967b5ab8efcbe06b0b6c4fe844b2554e531ab7cbd377415a772be5265099b6b4d9102c0ca2b7184be789bb29d8617a35f08f0810171beb7b615ba3c5c60810ba046119b8ffe42de2218706a22d5d893b991b29be5a5b7788495f7d2c51e42654baa24f39299dd48a374478cabd51e2054adbfbc3eac545d8";
    const CASE1_HASH: &str = "cc19201919749bd48f17ff5c4fd3052bf4015fb4178c347e8fafa18c624e3c7f";
    const CASE1_PROOF: &str = "5492f2c5809189bfd9cd4efe9c57c78519234af697bc3201d3a777b73ad4673d";

    const CASE2_HASH: &str = "16803340bc7814bb85782757a9536707e001721c35388473af520c96593c7e02";
    const CASE2_PROOF: &str = "3b77466441207be41b72281df866f4dd3780ff2a8ff68c4c22aabd35975070ae";

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

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

    fn resource_hash(s: &str) -> ResourceHash {
        ResourceHash::new(hx(s).try_into().unwrap())
    }

    #[test]
    fn the_reference_transfer_opens_to_exactly_the_stream_the_host_must_decompress() {
        let mut transfer = hx(CASE1_TRANSFER);
        let stream = open_transfer(&link_key(), &mut transfer).unwrap();
        assert_eq!(stream, &hx(CASE1_BZ2)[..]);
    }

    #[test]
    fn the_decompressed_plaintext_verifies_and_yields_the_reference_proof() {
        let proof = verify_and_prove(
            &case1_plaintext(),
            &SaltNonce::new(SALT_NONCE),
            &resource_hash(CASE1_HASH),
        )
        .unwrap();
        assert_eq!(proof.as_bytes(), &hx(CASE1_PROOF)[..]);
    }

    #[test]
    fn an_uncompressed_plaintext_verifies_against_its_reference_vectors_too() {
        let proof = verify_and_prove(
            &case2_plaintext(),
            &SaltNonce::new(SALT_NONCE),
            &resource_hash(CASE2_HASH),
        )
        .unwrap();
        assert_eq!(proof.as_bytes(), &hx(CASE2_PROOF)[..]);
    }

    #[test]
    fn the_mirror_reassembles_a_shuffled_transfer_built_by_its_own_sender() {
        let plaintext = case2_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let sdu = resource_sdu(BROADCAST_MTU);
        let built = build_outgoing_resource(
            &plaintext,
            None,
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            sdu,
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        let sealed = &transfer[..built.sealed_transfer_len];
        let names = &hashmap[..built.part_count * MAP_HASH_LEN];

        let mut reassembled = [0u8; 2_048];
        for index in [2usize, 0, 3, 1] {
            let part = &sealed[index * sdu..((index + 1) * sdu).min(sealed.len())];
            let at = match_part_in_window(part, &built.salt_nonce, names, 0, 4).unwrap();
            assert_eq!(at, index);
            reassembled[at * sdu..at * sdu + part.len()].copy_from_slice(part);
        }

        let opened =
            open_transfer(&link_key(), &mut reassembled[..built.sealed_transfer_len]).unwrap();
        assert_eq!(opened, &plaintext[..]);
        let proof = verify_and_prove(opened, &built.salt_nonce, &built.hash).unwrap();
        assert_eq!(proof, built.expected_proof);
    }

    #[test]
    fn the_window_bounds_the_scan_and_clips_at_the_known_names() {
        let plaintext = case2_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let sdu = resource_sdu(BROADCAST_MTU);
        let built = build_outgoing_resource(
            &plaintext,
            None,
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            sdu,
            &mut transfer,
            &mut hashmap,
        )
        .unwrap();
        let sealed = &transfer[..built.sealed_transfer_len];
        let names = &hashmap[..built.part_count * MAP_HASH_LEN];
        let part = |index: usize| &sealed[index * sdu..((index + 1) * sdu).min(sealed.len())];

        assert_eq!(
            match_part_in_window(part(3), &built.salt_nonce, names, 0, 2),
            None,
        );
        assert_eq!(
            match_part_in_window(part(0), &built.salt_nonce, names, 1, 3),
            None,
        );
        assert_eq!(
            match_part_in_window(part(3), &built.salt_nonce, names, 2, 75),
            Some(3),
        );

        let mut corrupted = part(1).to_vec();
        corrupted[0] ^= 1;
        assert_eq!(
            match_part_in_window(&corrupted, &built.salt_nonce, names, 0, 4),
            None,
        );
    }

    #[test]
    fn a_tampered_transfer_refuses_to_open() {
        let mut transfer = hx(CASE1_TRANSFER);
        *transfer.last_mut().unwrap() ^= 1;
        assert_eq!(
            open_transfer(&link_key(), &mut transfer).unwrap_err(),
            OpenTransferError::Open(CryptoError::InvalidMac),
        );
    }

    #[test]
    fn a_stream_too_short_for_its_nonce_is_refused_by_name() {
        let key = link_key();
        let mut sealed = [0u8; 64];
        let sealed_len = key.seal(&seal_iv(), b"abc", &mut sealed).unwrap();
        assert_eq!(
            open_transfer(&key, &mut sealed[..sealed_len]).unwrap_err(),
            OpenTransferError::StreamTooShort,
        );
    }

    #[test]
    fn a_corrupted_plaintext_or_wrong_salt_fails_the_hash_check() {
        let mut corrupted = case1_plaintext();
        corrupted[0] ^= 1;
        assert_eq!(
            verify_and_prove(
                &corrupted,
                &SaltNonce::new(SALT_NONCE),
                &resource_hash(CASE1_HASH),
            )
            .unwrap_err(),
            VerifyResourceError::HashMismatch,
        );
        assert_eq!(
            verify_and_prove(
                &case1_plaintext(),
                &SaltNonce::new([0x61, 0x62, 0x63, 0x65]),
                &resource_hash(CASE1_HASH),
            )
            .unwrap_err(),
            VerifyResourceError::HashMismatch,
        );
    }
}
