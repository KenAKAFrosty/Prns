//! RNS 1.4.2 `Resource.__init__`'s transmit path. Two deliberate divergences:
//! - on a collision re-roll, the reference recomputes its resource hash from a stale loop variable (a minor latent corruption); we recompute from the true plaintext. Wire identical.
//! - And where the reference re-rolls forever, [`SALT_REROLL_CAP`] bounds the loop.

use crate::crypto::{BufferTooShort, Sha256PrefixState};
use crate::routing::links::resources::{
    map_hash, map_hash_name_word, sealed_transfer_bytes, ResourceBody, ResourceBufferShape,
    ResourceBufferShapeError, ResourceCompression, ResourceHash, ResourceMetadata, ResourceProof,
    SaltNonce, COLLISION_GUARD_SIZE, MAP_HASH_LEN, MAX_EFFICIENT_SIZE, METADATA_MAX_SIZE,
    METADATA_PREFIX_LEN, RESOURCE_NONCE_LEN,
};
use crate::routing::links::LinkKey;

/// RNS 1.4.2 `Resource.__init__`'s keep-when-smaller rule, shared so a staging caller picks the same stream a build would.
pub fn winning_candidate(
    compressed_candidate: Option<&[u8]>,
    uncompressed_stream_len: usize,
) -> Option<&[u8]> {
    compressed_candidate.filter(|candidate| candidate.len() < uncompressed_stream_len)
}

fn uncompressed_stream_len(
    envelope_len: usize,
    body: &ResourceBody<'_>,
) -> Result<usize, BuildOutgoingResourceError> {
    let metadata_bytes = match body.metadata {
        ResourceMetadata::Packed(packed) if packed.len() > METADATA_MAX_SIZE => {
            return Err(BuildOutgoingResourceError::MetadataTooLarge);
        }
        ResourceMetadata::SentInFirstSegment { packed_len }
            if packed_len as usize > METADATA_MAX_SIZE =>
        {
            return Err(BuildOutgoingResourceError::MetadataTooLarge);
        }
        ResourceMetadata::Packed(packed) => METADATA_PREFIX_LEN + packed.len(),
        ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. } => 0,
    };
    metadata_bytes
        .checked_add(envelope_len)
        .and_then(|len| len.checked_add(body.data.len()))
        .filter(|len| *len <= MAX_EFFICIENT_SIZE)
        .ok_or(BuildOutgoingResourceError::DataTooLarge)
}

/// Calculates the exact bulk buffers the outgoing build will consume, using
/// the same compression winner and metadata rules as the build itself.
pub fn outgoing_resource_buffer_shape(
    envelope_len: usize,
    body: &ResourceBody<'_>,
    sdu: usize,
) -> Result<ResourceBufferShape, BuildOutgoingResourceError> {
    let uncompressed_len = uncompressed_stream_len(envelope_len, body)?;
    let stream_len = winning_candidate(body.compressed_candidate, uncompressed_len)
        .map_or(uncompressed_len, |candidate| candidate.len());
    let transfer_bytes = sealed_transfer_bytes(stream_len);
    ResourceBufferShape::try_for_transfer(transfer_bytes, sdu).map_err(|error| match error {
        ResourceBufferShapeError::EmptyTransfer => BuildOutgoingResourceError::Seal(BufferTooShort),
        ResourceBufferShapeError::SduTooSmall => BuildOutgoingResourceError::SduTooSmall,
        ResourceBufferShapeError::SizeOverflow => BuildOutgoingResourceError::DataTooLarge,
    })
}

/// Where a raw-staged stream waits inside its transfer region: past the seal IV and the stream nonce, exactly where [`seal_staged_resource`] pads and encrypts it in place.
pub const STAGED_STREAM_OFFSET: usize = 16 + RESOURCE_NONCE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealedStagedResource {
    pub sealed_transfer_bytes: usize,
    pub part_count: usize,
    pub hash: ResourceHash,
    pub salt_nonce: SaltNonce,
    pub expected_proof: ResourceProof,
}

/// The deferred half of a raw-staged build: the plaintext stream already sits at [`STAGED_STREAM_OFFSET`] with its nonce ahead of it, so this seals in place, then runs the same salt loop as [`build_outgoing_resource`].
/// Deferral moves the seal off the advertise path and into the live segment's transfer window; only an uncompressed metadata-free stream stages raw, so the digest prefix is the buffered stream itself.
pub fn seal_staged_resource(
    key: &LinkKey,
    seal_iv: &[u8; 16],
    mut fresh_salt: impl FnMut() -> [u8; RESOURCE_NONCE_LEN],
    sdu: usize,
    nonce_prefixed_bytes: usize,
    regions: BuildRegions<'_>,
) -> Result<SealedStagedResource, BuildOutgoingResourceError> {
    let BuildRegions { transfer, hashmap } = regions;
    if sdu == 0 {
        return Err(BuildOutgoingResourceError::SduTooSmall);
    }
    let stream_end = 16 + nonce_prefixed_bytes;
    let digest_prefix = Sha256PrefixState::absorb(&[&transfer[STAGED_STREAM_OFFSET..stream_end]]);
    let sealed_transfer_bytes = key
        .seal_in_place(seal_iv, transfer, nonce_prefixed_bytes)
        .map_err(BuildOutgoingResourceError::Seal)?;
    let part_count = sealed_transfer_bytes.div_ceil(sdu);
    let hashmap_len = part_count * MAP_HASH_LEN;
    if hashmap.len() < hashmap_len {
        return Err(BuildOutgoingResourceError::HashmapBufferTooShort);
    }

    let sealed = &transfer[..sealed_transfer_bytes];
    for _ in 0..SALT_REROLL_CAP {
        let salt_nonce = SaltNonce::new(fresh_salt());
        if matches!(
            write_hashmap_without_collision(sealed, sdu, &salt_nonce, &mut hashmap[..hashmap_len]),
            HashmapWriteOutcome::Collided,
        ) {
            continue;
        }
        let digests = digest_prefix.digests_with_suffix(salt_nonce.as_bytes());
        return Ok(SealedStagedResource {
            sealed_transfer_bytes,
            part_count,
            hash: ResourceHash::new(digests.with_suffix),
            salt_nonce,
            expected_proof: ResourceProof::new(digests.with_first_digest),
        });
    }
    Err(BuildOutgoingResourceError::SaltRerollsExhausted)
}

/// A real collision within the guard span is a ~5-in-a-million event per resource. Eight failures mean something is deeply wrong with the entropy source.
pub const SALT_REROLL_CAP: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutgoingResourceError {
    DataTooLarge,
    MetadataTooLarge,
    SduTooSmall,
    Seal(BufferTooShort),
    HashmapBufferTooShort,
    BufferShapeMismatch,
    SaltRerollsExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltResource {
    pub sealed_transfer_bytes: usize,
    pub part_count: usize,
    pub hash: ResourceHash,
    pub salt_nonce: SaltNonce,
    pub expected_proof: ResourceProof,
    pub compression: ResourceCompression,
    pub has_metadata: bool,
    pub uncompressed_data_bytes: u64,
}

/// The two slot regions a build writes: the sealed transfer stream and the flat map-hash names of its parts.
pub struct BuildRegions<'a> {
    pub transfer: &'a mut [u8],
    pub hashmap: &'a mut [u8],
}

/// `fresh_nonce` is drawn once for the stream nonce, then once per salt attempt (the same order the reference draws its `random_hash`es).
pub fn build_outgoing_resource(
    body: &ResourceBody<'_>,
    key: &LinkKey,
    seal_iv: &[u8; 16],
    fresh_nonce: impl FnMut() -> [u8; RESOURCE_NONCE_LEN],
    sdu: usize,
    regions: BuildRegions<'_>,
) -> Result<BuiltResource, BuildOutgoingResourceError> {
    build_outgoing_resource_enveloped(&[], body, key, seal_iv, fresh_nonce, sdu, regions)
}

pub fn build_outgoing_resource_enveloped(
    envelope: &[u8],
    body: &ResourceBody<'_>,
    key: &LinkKey,
    seal_iv: &[u8; 16],
    mut fresh_nonce: impl FnMut() -> [u8; RESOURCE_NONCE_LEN],
    sdu: usize,
    regions: BuildRegions<'_>,
) -> Result<BuiltResource, BuildOutgoingResourceError> {
    let BuildRegions { transfer, hashmap } = regions;
    let &ResourceBody {
        data: plaintext,
        compressed_candidate,
        metadata,
    } = body;
    let uncompressed_stream_len = uncompressed_stream_len(envelope.len(), body)?;
    let metadata_prefix = match metadata {
        ResourceMetadata::Packed(packed) => (packed.len() as u32).to_be_bytes(),
        ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. } => [0; 4],
    };
    let (block_prefix, block_packed): (&[u8], &[u8]) = match metadata {
        ResourceMetadata::Packed(packed) => (&metadata_prefix[1..], packed),
        ResourceMetadata::None | ResourceMetadata::SentInFirstSegment { .. } => (&[], &[]),
    };
    if sdu == 0 {
        return Err(BuildOutgoingResourceError::SduTooSmall);
    }
    let stream_nonce = fresh_nonce();
    let winner = winning_candidate(compressed_candidate, uncompressed_stream_len);
    let compressed_chunks: [&[u8]; 2];
    let uncompressed_chunks: [&[u8]; 5];
    let (stream_chunks, compression): (&[&[u8]], _) = match winner {
        Some(candidate) => {
            compressed_chunks = [&stream_nonce, candidate];
            (&compressed_chunks[..], ResourceCompression::Bz2)
        }
        None => {
            uncompressed_chunks = [
                &stream_nonce,
                block_prefix,
                block_packed,
                envelope,
                plaintext,
            ];
            (&uncompressed_chunks[..], ResourceCompression::Uncompressed)
        }
    };
    let sealed_transfer_bytes = key
        .seal_chunks(seal_iv, stream_chunks, transfer)
        .map_err(BuildOutgoingResourceError::Seal)?;
    let part_count = sealed_transfer_bytes.div_ceil(sdu);
    let hashmap_len = part_count * MAP_HASH_LEN;
    if hashmap.len() < hashmap_len {
        return Err(BuildOutgoingResourceError::HashmapBufferTooShort);
    }

    let sealed = &transfer[..sealed_transfer_bytes];
    let uncompressed_stream = [block_prefix, block_packed, envelope, plaintext];
    for _ in 0..SALT_REROLL_CAP {
        let salt_nonce = SaltNonce::new(fresh_nonce());
        let (hashmap_outcome, digests) = hashmap_and_digest(
            sealed,
            sdu,
            &salt_nonce,
            &mut hashmap[..hashmap_len],
            &uncompressed_stream,
        );
        if matches!(hashmap_outcome, HashmapWriteOutcome::Collided) {
            continue;
        }
        return Ok(BuiltResource {
            sealed_transfer_bytes,
            part_count,
            hash: ResourceHash::new(digests.with_suffix),
            salt_nonce,
            expected_proof: ResourceProof::new(digests.with_first_digest),
            compression,
            has_metadata: metadata.travels(),
            uncompressed_data_bytes: uncompressed_stream_len as u64,
        });
    }
    Err(BuildOutgoingResourceError::SaltRerollsExhausted)
}

pub enum HashmapWriteOutcome {
    Collided,
    DidNotCollide,
}

fn write_hashmap_without_collision(
    sealed: &[u8],
    sdu: usize,
    salt_nonce: &SaltNonce,
    hashmap: &mut [u8],
) -> HashmapWriteOutcome {
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
                return HashmapWriteOutcome::Collided;
            }
        }
        hashmap[offset..offset + MAP_HASH_LEN].copy_from_slice(&name);
    }
    HashmapWriteOutcome::DidNotCollide
}

/// Below this the join coordination outweighs the overlap: measured break-even ~64 KiB on an M4, ~1.24x at 1 MiB.
#[cfg(feature = "parallel-resource-hash")]
const PARALLEL_RESOURCE_MIN_BYTES: usize = 128 * 1024;

fn hashmap_and_digest(
    sealed: &[u8],
    sdu: usize,
    salt_nonce: &SaltNonce,
    hashmap: &mut [u8],
    uncompressed_stream: &[&[u8]],
) -> (HashmapWriteOutcome, crate::crypto::SharedPrefixDigests) {
    #[cfg(feature = "parallel-resource-hash")]
    if uncompressed_stream
        .iter()
        .map(|chunk| chunk.len())
        .sum::<usize>()
        >= PARALLEL_RESOURCE_MIN_BYTES
    {
        return rayon::join(
            || write_hashmap_without_collision(sealed, sdu, salt_nonce, hashmap),
            || {
                crate::crypto::sha256_prefix_and_digest_suffix(
                    uncompressed_stream,
                    salt_nonce.as_bytes(),
                )
            },
        );
    }
    (
        write_hashmap_without_collision(sealed, sdu, salt_nonce, hashmap),
        crate::crypto::sha256_prefix_and_digest_suffix(uncompressed_stream, salt_nonce.as_bytes()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{sha256, x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::routing::links::resources::resource_sdu;
    use crate::routing::links::LinkId;
    use crate::wire::BROADCAST_MTU;

    fn bytes_from_hex(s: &str) -> std::vec::Vec<u8> {
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
        let scalar: [u8; 32] = bytes_from_hex(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = bytes_from_hex(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        let id: [u8; 16] = bytes_from_hex(LINK_ID).try_into().unwrap();
        LinkKey::derive(&LinkId::new(id), &shared)
    }

    fn seal_iv() -> [u8; 16] {
        bytes_from_hex(SEAL_IV).try_into().unwrap()
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

    // The reference Resource.__init__ driven with the link-key fixture, IV a1..b0, stream nonce 51525354, salt nonce 61626364, sdu 464: b"reticulum resources ride the link " * 40 compresses 1360 -> 90, so the sealed transfer is one 144-byte part of bz2 stream.
    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";
    const CASE1_TRANSFER: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0defc0c57b1784ccf967b5ab8efcbe06b0b6c4fe844b2554e531ab7cbd377415a772be5265099b6b4d9102c0ca2b7184be789bb29d8617a35f08f0810171beb7b615ba3c5c60810ba046119b8ffe42de2218706a22d5d893b991b29be5a5b7788495f7d2c51e42654baa24f39299dd48a374478cabd51e2054adbfbc3eac545d8";
    const CASE1_HASH: &str = "cc19201919749bd48f17ff5c4fd3052bf4015fb4178c347e8fafa18c624e3c7f";
    const CASE1_PROOF: &str = "5492f2c5809189bfd9cd4efe9c57c78519234af697bc3201d3a777b73ad4673d";
    const CASE1_HASHMAP: &str = "973c9707";

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    // 1500 bytes of sha256 chain don't compress (bz2 expands them to 1894), so the reference keeps the plaintext: 4 parts of sealed stream.
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
        let candidate = bytes_from_hex(CASE1_BZ2);
        let mut transfer = [0u8; 512];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: Some(&candidate),
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Bz2);
        assert_eq!(built.sealed_transfer_bytes, 144);
        assert_eq!(built.part_count, 1);
        assert_eq!(built.uncompressed_data_bytes, 1_360);
        assert_eq!(
            &transfer[..built.sealed_transfer_bytes],
            &bytes_from_hex(CASE1_TRANSFER)[..]
        );
        assert_eq!(built.hash.as_bytes(), &bytes_from_hex(CASE1_HASH)[..]);
        assert_eq!(
            built.expected_proof.as_bytes(),
            &bytes_from_hex(CASE1_PROOF)[..]
        );
        assert_eq!(built.salt_nonce, SaltNonce::new(SALT_NONCE));
        assert_eq!(&hashmap[..MAP_HASH_LEN], &bytes_from_hex(CASE1_HASHMAP)[..]);
    }

    #[test]
    fn an_incompressible_resource_rejects_its_candidate_like_the_reference() {
        let plaintext = case2_plaintext();
        let expanding_candidate = std::vec![0u8; 1_894];
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: Some(&expanding_candidate),
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Uncompressed);
        assert_eq!(built.sealed_transfer_bytes, 1_568);
        assert_eq!(built.part_count, 4);
        assert_eq!(built.uncompressed_data_bytes, 1_500);
        assert_eq!(&transfer[..32], &bytes_from_hex(CASE2_TRANSFER_HEAD)[..]);
        assert_eq!(
            &transfer[built.sealed_transfer_bytes - 11..built.sealed_transfer_bytes],
            &bytes_from_hex(CASE2_TRANSFER_TAIL)[..]
        );
        assert_eq!(built.hash.as_bytes(), &bytes_from_hex(CASE2_HASH)[..]);
        assert_eq!(
            built.expected_proof.as_bytes(),
            &bytes_from_hex(CASE2_PROOF)[..]
        );
        assert_eq!(
            &hashmap[..4 * MAP_HASH_LEN],
            &bytes_from_hex(CASE2_HASHMAP)[..]
        );
    }

    #[test]
    fn planned_buffer_shapes_are_exact_for_both_compression_outcomes() {
        let sdu = resource_sdu(BROADCAST_MTU);
        let compressible = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);
        let compressed_body = ResourceBody {
            data: &compressible,
            compressed_candidate: Some(&candidate),
            metadata: ResourceMetadata::None,
        };
        let compressed_shape = outgoing_resource_buffer_shape(0, &compressed_body, sdu).unwrap();
        assert_eq!(
            compressed_shape,
            ResourceBufferShape::try_for_transfer(144, sdu).unwrap(),
        );

        let incompressible = case2_plaintext();
        let expanding_candidate = std::vec![0u8; 1_894];
        let uncompressed_body = ResourceBody {
            data: &incompressible,
            compressed_candidate: Some(&expanding_candidate),
            metadata: ResourceMetadata::None,
        };
        let uncompressed_shape =
            outgoing_resource_buffer_shape(0, &uncompressed_body, sdu).unwrap();
        assert_eq!(
            uncompressed_shape,
            ResourceBufferShape::try_for_transfer(1_568, sdu).unwrap(),
        );

        let packed = bytes_from_hex(META_PACKED);
        let metadata_body = ResourceBody {
            metadata: ResourceMetadata::Packed(&packed),
            ..uncompressed_body
        };
        assert_eq!(
            outgoing_resource_buffer_shape(20, &metadata_body, sdu).unwrap(),
            ResourceBufferShape::try_for_transfer(1_600, sdu).unwrap(),
        );
        let later_segment_body = ResourceBody {
            metadata: ResourceMetadata::SentInFirstSegment { packed_len: 21 },
            ..uncompressed_body
        };
        assert_eq!(
            outgoing_resource_buffer_shape(0, &later_segment_body, sdu).unwrap(),
            uncompressed_shape,
            "a later segment advertises metadata but does not store its block again",
        );

        let mut transfer = std::vec![0u8; compressed_shape.transfer_bytes()];
        let mut hashmap = std::vec![0u8; compressed_shape.part_count() * MAP_HASH_LEN];
        let built = build_outgoing_resource(
            &compressed_body,
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            sdu,
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(built.sealed_transfer_bytes, transfer.len());
        assert_eq!(built.part_count * MAP_HASH_LEN, hashmap.len());
    }

    #[test]
    fn the_resource_hash_ignores_compression_entirely() {
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let with = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: Some(&candidate),
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        let without = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: None,
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(without.compression, ResourceCompression::Uncompressed);
        assert_eq!(with.hash, without.hash);
        assert_eq!(with.expected_proof, without.expected_proof);
        assert_ne!(with.sealed_transfer_bytes, without.sealed_transfer_bytes);
    }

    #[test]
    fn single_byte_parts_collide_until_the_reroll_cap_gives_up() {
        let plaintext = case1_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 8_192];
        let mut drawn = 0u32;
        let result = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: None,
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            move || {
                drawn += 1;
                drawn.to_be_bytes()
            },
            1,
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        );
        assert_eq!(
            result.unwrap_err(),
            BuildOutgoingResourceError::SaltRerollsExhausted,
        );
    }

    #[test]
    fn buffer_and_size_guards_refuse() {
        let plaintext = case1_plaintext();
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        assert_eq!(
            build_outgoing_resource(
                &ResourceBody {
                    data: &plaintext,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                0,
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut hashmap,
                },
            )
            .unwrap_err(),
            BuildOutgoingResourceError::SduTooSmall,
        );
        assert_eq!(
            build_outgoing_resource(
                &ResourceBody {
                    data: &plaintext,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                BuildRegions {
                    transfer: &mut transfer[..64],
                    hashmap: &mut hashmap,
                },
            )
            .unwrap_err(),
            BuildOutgoingResourceError::Seal(BufferTooShort),
        );
        assert_eq!(
            build_outgoing_resource(
                &ResourceBody {
                    data: &plaintext,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut hashmap[..4],
                },
            )
            .unwrap_err(),
            BuildOutgoingResourceError::HashmapBufferTooShort,
        );
        let huge = std::vec![0u8; MAX_EFFICIENT_SIZE + 1];
        assert_eq!(
            build_outgoing_resource(
                &ResourceBody {
                    data: &huge,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                &link_key(),
                &seal_iv(),
                reference_nonces(),
                resource_sdu(BROADCAST_MTU),
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut hashmap,
                },
            )
            .unwrap_err(),
            BuildOutgoingResourceError::DataTooLarge,
        );
    }

    #[test]
    fn the_sdu_arithmetic_matches_the_reference() {
        assert_eq!(resource_sdu(BROADCAST_MTU), 464);
        assert_eq!(resource_sdu(BROADCAST_MTU), crate::wire::BROADCAST_MDU);
    }

    /// umsgpack.packb({"name": "case.bin", "flag": 7}). The reference prepends `struct.pack(">I", 21)[1:]` and feeds bz2 the whole 1384-byte composite.
    const META_PACKED: &str = "82a46e616d65a8636173652e62696ea4666c616707";
    /// bz2.compress(metadata block ‖ case1 plaintext): 133 bytes, so it wins the keep-if-smaller comparison.
    const META_CASE1_BZ2: &str ="425a6839314159265359c5bada7900000071d04080020040013fef9e00100004403000b8450000064c82800003264052a5008da684f227a37e3ae33ea278137546a26f89e7fb3cbe7a13509a89a09fbcc4e2132f9a7f84e027613d6627bd44d8274fcef13b09c04e3547bc09a09cf026e130277132136136b79ff177245385090c5bada790";
    /// Reference Resource.__init__ with metadata under the same key/IV/nonce fixture: sealed transfer 192 bytes, d = 1384 (data 1360 + block 24).
    const META_CASE1_TRANSFER: &str ="a1a2a3a4a5a6a7a8a9aaabacadaeafb083e32d824918ae111174b90353abe9b1f4150121fbc835819a26e4f28ec383bd0c75cd812136c926fc7d4f56a07b077d2ac3aacdeb07fadfa895e3b2ef7cb05004e9e1e7dbcb4666357854b0cf7d9a989126c4a1d92b0ea2c9b92db817874cffe7187f45b899e6499262e899fe31f3a0ad4167b35cc3a0138df04071f79574307f9ad8d8360ce43345bf9e586317c60ab77868e558620135810e3b231f5ffc52eda1e6c99b389bb24d2e901fa58e13ea";
    const META_CASE1_HASH: &str =
        "8020b58824c287c4a9fbf444c1c604bac627bf71711b817f8876c18f2d9bc2b1";
    const META_CASE1_PROOF: &str =
        "756188f8406848cdf266849095195bd3fb4fbbd52b4cf3c24157fa11b3e9596b";
    const META_CASE1_HASHMAP: &str = "3d445e0c";
    /// The sha-chain data with the same metadata, no compression: 4 parts, d = 1524 (data 1500 + block 24).
    const META_CASE2_HASH: &str =
        "a4e620b78799a5feca5d957234815ba445ad9067b81139bc819567673fe537eb";
    const META_CASE2_PROOF: &str =
        "701c59b86a691ae9def7657c314ef9f45a691430aadd4c675933a5abd9d37281";
    const META_CASE2_HASHMAP: &str = "570755e54d519dea99f9ca594420833d";
    const META_CASE2_TRANSFER_HEAD: &str =
        "a1a2a3a4a5a6a7a8a9aaabacadaeafb0c625cb4b4a77b4bb986c11315e2f420b";
    const META_CASE2_TRANSFER_TAIL: &str =
        "fb9edbb74b510af92da6382b9f54553a566844fd6abc7556fdc9123c";

    #[test]
    fn a_metadata_resource_builds_byte_identical_to_the_reference() {
        let plaintext = case1_plaintext();
        let packed = bytes_from_hex(META_PACKED);
        let candidate = bytes_from_hex(META_CASE1_BZ2);
        let mut transfer = [0u8; 512];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: Some(&candidate),
                metadata: ResourceMetadata::Packed(&packed),
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Bz2);
        assert_eq!(built.sealed_transfer_bytes, 192);
        assert_eq!(built.part_count, 1);
        assert_eq!(built.uncompressed_data_bytes, 1_384);
        assert_eq!(
            &transfer[..built.sealed_transfer_bytes],
            &bytes_from_hex(META_CASE1_TRANSFER)[..]
        );
        assert_eq!(built.hash.as_bytes(), &bytes_from_hex(META_CASE1_HASH)[..]);
        assert_eq!(
            built.expected_proof.as_bytes(),
            &bytes_from_hex(META_CASE1_PROOF)[..]
        );
        assert_eq!(
            &hashmap[..MAP_HASH_LEN],
            &bytes_from_hex(META_CASE1_HASHMAP)[..]
        );
    }

    #[test]
    fn an_uncompressed_metadata_resource_matches_the_reference() {
        let plaintext = case2_plaintext();
        let packed = bytes_from_hex(META_PACKED);
        let mut transfer = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let built = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: None,
                metadata: ResourceMetadata::Packed(&packed),
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert_eq!(built.compression, ResourceCompression::Uncompressed);
        assert_eq!(built.sealed_transfer_bytes, 1_584);
        assert_eq!(built.part_count, 4);
        assert_eq!(built.uncompressed_data_bytes, 1_524);
        assert_eq!(
            &transfer[..32],
            &bytes_from_hex(META_CASE2_TRANSFER_HEAD)[..]
        );
        assert_eq!(
            &transfer[built.sealed_transfer_bytes - 28..built.sealed_transfer_bytes],
            &bytes_from_hex(META_CASE2_TRANSFER_TAIL)[..]
        );
        assert_eq!(built.hash.as_bytes(), &bytes_from_hex(META_CASE2_HASH)[..]);
        assert_eq!(
            built.expected_proof.as_bytes(),
            &bytes_from_hex(META_CASE2_PROOF)[..]
        );
        assert_eq!(
            &hashmap[..4 * MAP_HASH_LEN],
            &bytes_from_hex(META_CASE2_HASHMAP)[..]
        );
    }

    #[test]
    fn a_later_split_segment_seals_without_the_block_it_still_accounts_for() {
        let plaintext = case1_plaintext();
        let mut with_marker = [0u8; 2_048];
        let mut without = [0u8; 2_048];
        let mut hashmap = [0u8; 64];
        let marked = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: None,
                metadata: ResourceMetadata::SentInFirstSegment { packed_len: 21 },
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut with_marker,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        let plain = build_outgoing_resource(
            &ResourceBody {
                data: &plaintext,
                compressed_candidate: None,
                metadata: ResourceMetadata::None,
            },
            &link_key(),
            &seal_iv(),
            reference_nonces(),
            resource_sdu(BROADCAST_MTU),
            BuildRegions {
                transfer: &mut without,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        assert!(marked.has_metadata);
        assert_eq!(
            BuiltResource {
                has_metadata: false,
                ..marked
            },
            plain
        );
        assert_eq!(with_marker, without);
    }

    #[test]
    fn oversize_metadata_refuses_on_both_faces() {
        let plaintext = case1_plaintext();
        let oversize = std::vec![0u8; METADATA_MAX_SIZE + 1];
        let mut transfer = [0u8; 512];
        let mut hashmap = [0u8; 64];
        for metadata in [
            ResourceMetadata::Packed(&oversize),
            ResourceMetadata::SentInFirstSegment {
                packed_len: (METADATA_MAX_SIZE + 1) as u32,
            },
        ] {
            assert_eq!(
                build_outgoing_resource(
                    &ResourceBody {
                        data: &plaintext,
                        compressed_candidate: None,
                        metadata,
                    },
                    &link_key(),
                    &seal_iv(),
                    reference_nonces(),
                    resource_sdu(BROADCAST_MTU),
                    BuildRegions {
                        transfer: &mut transfer,
                        hashmap: &mut hashmap,
                    },
                )
                .unwrap_err(),
                BuildOutgoingResourceError::MetadataTooLarge,
            );
        }
    }
}
