//! RNS 1.3.1 Resources: bulk data over an established link. The sender
//! compresses the payload, seals the whole stream under the session key in
//! one pass, and slices the ciphertext into parts; the receiver pulls parts
//! by 4-byte map hashes inside a sliding window, then proves the assembled
//! whole with a hash. The link is the authentication, so no signature rides
//! the transfer itself. This module holds the protocol arithmetic the family shares;
//! the two msgpack wire shapes live in [`advertisement`].

pub mod advertisement;
pub mod control;

use crate::routing::links::data::LINK_MDU;

/// RNS 1.3.1 `Resource.MAPHASH_LEN`: a part is named by the first four bytes
/// of `full_hash(part ‖ nonce)`.
pub const MAP_HASH_LEN: usize = 4;

/// RNS 1.3.1 `Resource.RANDOM_HASH_SIZE`. The reference's `random_hash` is
/// no hash at all, so it carries its honest name here: a per-resource nonce,
/// prepended to the plaintext stream and salted into every map hash.
///
/// This is done so part names never repeat across transfers of the same data.
pub const RESOURCE_NONCE_LEN: usize = 4;

/// A resource names itself by a full SHA-256. RNS 1.3.1
/// `Identity.full_hash(data + random_hash)`: the uncompressed plaintext
/// salted with the resource nonce.
pub const RESOURCE_HASH_LEN: usize = 32;

/// RNS 1.3.1 `Resource.WINDOW_MAX` (= `WINDOW_MAX_FAST`): the widest part
/// window either end will ever run. The collision guard is sized from it.
pub const WINDOW_MAX: usize = 75;

/// RNS 1.3.1 `ResourceAdvertisement.OVERHEAD`: the byte budget the reference reserves for everything in a packed advertisement except the map hashes.
pub const ADVERTISEMENT_OVERHEAD: usize = 134;

/// RNS 1.3.1 `ResourceAdvertisement.HASHMAP_MAX_LEN` (74): how many map
/// hashes ride one advertisement or one hashmap update.
///
/// Derived from the base link MDU (431), never the negotiated one, so every link lands on the
/// same figure regardless of its MTU.
pub const HASHMAP_MAX_LEN: usize = (LINK_MDU - ADVERTISEMENT_OVERHEAD) / MAP_HASH_LEN;

/// RNS 1.3.1 `ResourceAdvertisement.COLLISION_GUARD_SIZE` (224): the sliding
/// span of parts within which two map hashes must not collide.
/// The sender re-rolls its nonce until they don't.
pub const COLLISION_GUARD_SIZE: usize = 2 * WINDOW_MAX + HASHMAP_MAX_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceHash([u8; RESOURCE_HASH_LEN]);

impl ResourceHash {
    #[must_use]
    pub const fn new(bytes: [u8; RESOURCE_HASH_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; RESOURCE_HASH_LEN] {
        &self.0
    }
}

/// What the receiver sends back when the assembled plaintext checks out —
/// RNS 1.3.1 `expected_proof = Identity.full_hash(data + hash)`. A hash, not
/// a signature: the link itself is the authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceProof([u8; RESOURCE_HASH_LEN]);

impl ResourceProof {
    #[must_use]
    pub const fn new(bytes: [u8; RESOURCE_HASH_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; RESOURCE_HASH_LEN] {
        &self.0
    }
}
