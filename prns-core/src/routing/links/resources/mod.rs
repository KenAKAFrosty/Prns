//! RNS 1.3.5 Resources: the sender seals the whole stream once under the session
//! key and slices the ciphertext into parts; the receiver pulls parts by 4-byte map
//! hashes inside a sliding window. The link is the authentication; no signature
//! rides the transfer.

pub mod advertisement;
pub mod assemble_incoming;
pub mod assembly;
pub mod build_outgoing;
pub mod control;
pub mod receive;
pub mod send;
pub mod serve_outgoing;
pub mod table;

use crate::engine::CommandId;
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN, IFAC_MIN_LEN};
use sha2::{Digest, Sha256};

/// RNS 1.3.5 `Resource.MAPHASH_LEN`: a part is named by the first four bytes
/// of `full_hash(part ‖ salt nonce)`.
pub const MAP_HASH_LEN: usize = 4;

/// RNS 1.3.5 `Resource.RANDOM_HASH_SIZE`; the reference's `random_hash`es are no
/// hashes at all. TWO distinct nonces this size: the stream nonce (sealed ahead of
/// the payload, discarded on assembly) and the salt nonce (the advertisement's `r`,
/// salting every map hash); only the salt nonce re-rolls on collision.
pub const RESOURCE_NONCE_LEN: usize = 4;

/// A resource names itself by a full SHA-256. RNS 1.3.5
/// `Identity.full_hash(data + random_hash)`: the uncompressed plaintext
/// salted with the salt nonce.
pub const RESOURCE_HASH_LEN: usize = 32;

/// RNS 1.3.5 `Resource.WINDOW_MAX` (= `WINDOW_MAX_FAST`): the widest part
/// window either end will ever run. The collision guard is sized from it.
pub const WINDOW_MAX: usize = 75;

/// RNS 1.3.5 `Resource.WINDOW`: where the receiver's part window starts.
pub const WINDOW: usize = 4;

/// RNS 1.3.5 `Resource.WINDOW_MIN`: the floor the window never shrinks past.
pub const WINDOW_MIN: usize = 2;

/// RNS 1.3.5 `Resource.WINDOW_MAX_SLOW`: the ceiling a window grows toward
/// until the link proves fast enough to lift it to [`WINDOW_MAX`].
pub const WINDOW_MAX_SLOW: usize = 10;

/// RNS 1.3.5 `Resource.WINDOW_FLEXIBILITY`: how far the window may run ahead
/// of its floor before the floor follows it up.
pub const WINDOW_FLEXIBILITY: usize = 4;

/// RNS 1.3.5 `Resource.MAX_RETRIES`: how many part-request retries a
/// receiver spends before giving up on a transfer.
pub const MAX_RETRIES: u8 = 16;

/// RNS 1.3.5 `Resource.MAX_ADV_RETRIES`: how many times a sender re-sends an
/// unanswered advertisement.
pub const MAX_ADV_RETRIES: u8 = 4;

/// RNS 1.3.5 `Resource.PART_TIMEOUT_FACTOR`: the rtt multiple a receiver
/// waits on outstanding parts before retrying.
pub const PART_TIMEOUT_FACTOR: u64 = 4;

/// RNS 1.3.5 `Resource.PART_TIMEOUT_FACTOR_AFTER_RTT`: once a round trip has
/// actually been measured, the wait tightens to this multiple.
pub const PART_TIMEOUT_FACTOR_AFTER_RTT: u64 = 2;

/// RNS 1.3.5 `Resource.RATE_FAST` (50 kbps as bytes/s): the measured rate
/// past which a round counts toward lifting the window ceiling to
/// [`WINDOW_MAX`].
pub const RATE_FAST_BYTES_PER_SECOND: u64 = 50 * 1000 / 8;

/// RNS 1.3.5 `Resource.RATE_VERY_SLOW` (2 kbps as bytes/s): the measured
/// rate below which a round counts toward dropping the ceiling to
/// [`WINDOW_MAX_VERY_SLOW`].
pub const RATE_VERY_SLOW_BYTES_PER_SECOND: u64 = 2 * 1000 / 8;

/// RNS 1.3.5 `Resource.WINDOW_MAX_VERY_SLOW`.
pub const WINDOW_MAX_VERY_SLOW: usize = 4;

/// RNS 1.3.5 `Resource.FAST_RATE_THRESHOLD` (`WINDOW_MAX_SLOW - WINDOW - 2`
/// = 4): how many fast rounds earn the lift.
pub const FAST_RATE_THRESHOLD: u8 = (WINDOW_MAX_SLOW - WINDOW - 2) as u8;

/// RNS 1.3.5 `Resource.VERY_SLOW_RATE_THRESHOLD`: how many very-slow rounds
/// (with no fast round ever seen) drop the ceiling.
pub const VERY_SLOW_RATE_THRESHOLD: u8 = 2;

/// LINKREQUEST (86) + LRPROOF (118) + LRRTT (83) at the broadcast MTU. The reference
/// accumulates actual lengths (`Link.establishment_cost`); ours pins the
/// deterministic total, which seeds only the first rate estimate and converges
/// identically from round one.
pub const ESTABLISHMENT_COST_ESTIMATE_BYTES: u64 = 86 + 118 + 83;

/// RNS 1.3.5 `Resource.PROOF_TIMEOUT_FACTOR`: the smaller rtt multiple a
/// sender waits on the proof — proof packets are far smaller than a full
/// request round trip.
pub const PROOF_TIMEOUT_FACTOR: u64 = 3;

/// RNS 1.3.5 `Resource.PROCESSING_GRACE` (1 s), in the engine's millis.
pub const PROCESSING_GRACE_MS: u64 = 1_000;

/// RNS 1.3.5 `Resource.RETRY_GRACE_TIME` (0.25 s), in the engine's millis.
pub const RETRY_GRACE_MS: u64 = 250;

/// RNS 1.3.5 `Resource.PER_RETRY_DELAY` (0.5 s), in the engine's millis:
/// every retry already spent stretches the next deadline by this much.
pub const PER_RETRY_DELAY_MS: u64 = 500;

/// RNS 1.3.5 `Resource.SENDER_GRACE_TIME` (10 s), in the engine's millis.
pub const SENDER_GRACE_MS: u64 = 10_000;

/// RNS 1.3.5 `ResourceAdvertisement.OVERHEAD`: the byte budget the reference reserves for everything in a packed advertisement except the map hashes.
pub const ADVERTISEMENT_OVERHEAD: usize = 134;

/// RNS 1.3.5 `ResourceAdvertisement.HASHMAP_MAX_LEN` (74): how many map
/// hashes ride one advertisement or one hashmap update.
///
/// Derived from the base link MDU (431), never the negotiated one, so every link lands on the
/// same figure regardless of its MTU.
pub const HASHMAP_MAX_LEN: usize = (LINK_MDU - ADVERTISEMENT_OVERHEAD) / MAP_HASH_LEN;

/// RNS 1.3.5 `ResourceAdvertisement.COLLISION_GUARD_SIZE` (224): the sliding
/// span of parts within which two map hashes must not collide.
/// The sender re-rolls its salt nonce until they don't.
pub const COLLISION_GUARD_SIZE: usize = 2 * WINDOW_MAX + HASHMAP_MAX_LEN;

/// RNS 1.3.5 `Resource.MAX_EFFICIENT_SIZE` (1 MiB − 1): the most one segment
/// carries. Anything larger splits into segments of this size, each
/// transferred as its own resource sharing the first segment's hash — the
/// constant that bounds every buffer in the family.
pub const MAX_EFFICIENT_SIZE: usize = 1024 * 1024 - 1;

/// RNS 1.3.5 `Resource.sdu`: parts ride as raw ciphertext chunks behind a
/// plain data header — no per-part token overhead — so a part carries the
/// link MTU less the two-address header and minimum IFAC
/// (`mtu - HEADER_MAXSIZE - IFAC_MIN_SIZE`, 464 at the broadcast MTU).
pub const fn resource_sdu(mtu: usize) -> usize {
    mtu - HEADER_MAX_LEN - IFAC_MIN_LEN
}

/// IV ‖ PKCS#7-padded(stream nonce ‖ stream) ‖ MAC.
pub const fn sealed_transfer_len(stream_len: usize) -> usize {
    let padded = ((stream_len + RESOURCE_NONCE_LEN) / 16 + 1) * 16;
    16 + padded + 32
}

/// The part count at the broadcast-MTU sdu: the floor every link clears, so the
/// worst case a store must name.
pub const fn max_part_count(transfer_capacity: usize) -> usize {
    transfer_capacity.div_ceil(resource_sdu(BROADCAST_MTU))
}

/// RNS 1.3.5 `Resource.get_map_hash`: the four-byte name a part is requested
/// by — `full_hash(part ‖ salt nonce)` truncated.
pub fn map_hash(part: &[u8], salt_nonce: &SaltNonce) -> [u8; MAP_HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(part);
    hasher.update(salt_nonce.as_bytes());
    let digest = hasher.finalize();
    [digest[0], digest[1], digest[2], digest[3]]
}

pub(crate) fn map_hash_name_word(name: &[u8]) -> u32 {
    u32::from_ne_bytes([name[0], name[1], name[2], name[3]])
}

/// The advertisement's `r`; the reference calls it `random_hash` and it is no hash at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaltNonce([u8; RESOURCE_NONCE_LEN]);

impl SaltNonce {
    #[must_use]
    pub const fn new(bytes: [u8; RESOURCE_NONCE_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; RESOURCE_NONCE_LEN] {
        &self.0
    }
}

/// RNS 1.3.5 `Link.resource_strategy`, engine-gated: the reference's unbounded
/// `ACCEPT_ALL` becomes an accept with enforced bounds, refused at the
/// advertisement gate before a single part moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceStrategy {
    /// Every link is born refusing resources; RNS 1.3.5 `Link.__init__` sets `ACCEPT_NONE`.
    #[default]
    AcceptNone,
    Accept {
        max_uncompressed_len: u64,
        accept_compressed: bool,
    },
}

/// RNS 1.3.5 knows exactly one algorithm behind the advertisement's `c` flag: bz2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCompression {
    Uncompressed,
    Bz2,
}

impl ResourceCompression {
    #[must_use]
    pub const fn wire_flag(self) -> bool {
        match self {
            Self::Uncompressed => false,
            Self::Bz2 => true,
        }
    }

    /// bz2 is the only compression RNS 1.3.5 can mean by the `c` flag.
    #[must_use]
    pub const fn from_wire_flag(compressed: bool) -> Self {
        if compressed {
            Self::Bz2
        } else {
            Self::Uncompressed
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSend<'a> {
    pub id: CommandId,
    pub link_id: LinkId,
    pub body: ResourceBody<'a>,
    pub correlation: ResourceCorrelation,
}

/// The reference's keep-only-if-smaller rule picks between payload and precompressed
/// attempt at build time; a host that links no compressor passes `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBody<'a> {
    pub data: &'a [u8],
    pub compressed_candidate: Option<&'a [u8]>,
}

/// RNS 1.3.5 advertises `(segment_index, total_segments)` plus the whole transfer's
/// uncompressed length (the `d` field) on every segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSegment {
    pub index: u64,
    pub total: u64,
    pub total_data_size: u64,
}

impl ResourceSegment {
    #[must_use]
    pub fn whole(data_len: u64) -> Self {
        Self {
            index: 1,
            total: 1,
            total_data_size: data_len,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePartRequest<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub requested: &'a [u8],
    pub exhausted_at: Option<[u8; MAP_HASH_LEN]>,
}

/// RNS 1.3.5 `Resource.is_request` / `is_response` + `request_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceCorrelation {
    #[default]
    Unsolicited,
    Request(RequestId),
    Response(RequestId),
}

impl ResourceCorrelation {
    #[must_use]
    pub fn request_id(self) -> Option<RequestId> {
        match self {
            Self::Unsolicited => None,
            Self::Request(id) | Self::Response(id) => Some(id),
        }
    }

    #[must_use]
    pub const fn is_request(self) -> bool {
        matches!(self, Self::Request(_))
    }

    #[must_use]
    pub const fn is_response(self) -> bool {
        matches!(self, Self::Response(_))
    }
}

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

/// RNS 1.3.5 `expected_proof = Identity.full_hash(data + hash)`. A hash, not a
/// signature: the link itself is the authentication.
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
