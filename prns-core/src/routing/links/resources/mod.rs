//! RNS 1.4.2 Resources: the sender seals the whole stream once under the session key and slices the ciphertext into parts; the receiver pulls parts by 4-byte map hashes inside a sliding window. The link is the authentication; no signature rides the transfer.

pub mod advertisement;
pub mod assemble_incoming;
pub mod assembly;
pub mod build_outgoing;
pub mod control;
pub mod pending;
pub mod receive;
pub mod send;
mod send_plan;
pub mod serve_outgoing;
pub mod streamed_open;
pub mod table;

pub use send_plan::{ResourceSegmentPlan, ResourceSendPlan, ResourceSendPlanError};

use crate::crypto::SHA256_OUTPUT_LEN;
use crate::engine::CommandId;
use crate::routing::links::data::LINK_MDU;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::wire::{BROADCAST_MTU, HEADER_MAX_LEN, IFAC_MIN_LEN};
use sha2::{Digest, Sha256};

/// RNS 1.4.2 `Resource.MAPHASH_LEN`: a part is named by the first four bytes of `full_hash(part ‖ salt nonce)`.
pub const MAP_HASH_LEN: usize = 4;

/// RNS 1.4.2 `Resource.RANDOM_HASH_SIZE`.
///
/// It turns out the reference's `random_hash`es are not hashes after all. Instead, they are TWO distinct nonces of this size:
/// - the stream nonce (sealed ahead of the payload, discarded on assembly), and
/// - the salt nonce (the advertisement's `r`, salting every map hash).
///
/// Only the salt nonce re-rolls on collision.
pub const RESOURCE_NONCE_LEN: usize = 4;

/// A resource names itself by a full SHA-256. RNS 1.4.2 `Identity.full_hash(data + random_hash)`
pub const RESOURCE_HASH_LEN: usize = SHA256_OUTPUT_LEN;

/// RNS 1.4.2 `Resource.WINDOW_MAX` (= `WINDOW_MAX_FAST`). The widest part window either end will ever run. The collision guard is sized from it.
pub const WINDOW_MAX: usize = 75;

/// RNS 1.4.2 `Resource.WINDOW`
pub const WINDOW_START: usize = 4;

/// RNS 1.4.2 `Resource.WINDOW_MIN`.
pub const WINDOW_MIN: usize = 2;

/// RNS 1.4.2 `Resource.WINDOW_MAX_SLOW`. The ceiling a window grows toward until the link proves fast enough to lift it to [`WINDOW_MAX`].
pub const WINDOW_MAX_SLOW: usize = 10;

/// RNS 1.4.2 `Resource.WINDOW_FLEXIBILITY`. How far the window may run ahead of its floor before the floor follows it up.
pub const WINDOW_FLEXIBILITY: usize = 4;

/// RNS 1.4.2 `Resource.MAX_RETRIES`
pub const PART_REQUEST_MAX_RETRIES: u8 = 16;

/// RNS 1.4.2 `Resource.MAX_ADV_RETRIES`
pub const MAX_ADVERTISEMENT_RETRIES: u8 = 4;

/// RNS 1.4.2 `Resource.PART_TIMEOUT_FACTOR`. The rtt multiple a receiver waits on outstanding parts before retrying.
pub const PART_TIMEOUT_FACTOR: u64 = 4;

/// RNS 1.4.2 `Resource.PART_TIMEOUT_FACTOR_AFTER_RTT`.
pub const PART_TIMEOUT_FACTOR_AFTER_RTT: u64 = 2;

/// RNS 1.4.2 `Resource.RATE_FAST` (50 kbps as bytes/s). The measured rate past which a round counts toward lifting the window ceiling to  [`WINDOW_MAX`].
pub const RATE_FAST_BYTES_PER_SECOND: u64 = 50 * 1000 / 8;

/// RNS 1.4.2 `Resource.RATE_VERY_SLOW` (2 kbps as bytes/s): the measured rate below which a round counts toward dropping the ceiling to [`WINDOW_MAX_VERY_SLOW`].
pub const RATE_VERY_SLOW_BYTES_PER_SECOND: u64 = 2 * 1000 / 8;

/// RNS 1.4.2 `Resource.WINDOW_MAX_VERY_SLOW`.
pub const WINDOW_MAX_VERY_SLOW: usize = 4;

/// RNS 1.4.2 `Resource.FAST_RATE_THRESHOLD` (`WINDOW_MAX_SLOW - WINDOW - 2` = 4). How many fast rounds earn the lift.
pub const FAST_RATE_THRESHOLD: u8 = (WINDOW_MAX_SLOW - WINDOW_START - 2) as u8;

/// RNS 1.4.2 `Resource.VERY_SLOW_RATE_THRESHOLD`. How many very-slow rounds (with no fast round ever seen) drop the ceiling.
pub const VERY_SLOW_RATE_THRESHOLD: u8 = 2;

/// LINKREQUEST (86) + LRPROOF (118) + LRRTT (83) at the broadcast MTU.
/// The reference accumulates actual lengths (`Link.establishment_cost`); ours pins the deterministic total, which seeds only the first rate estimate and converges identically from round one.
pub const ESTABLISHMENT_COST_ESTIMATE_BYTES: u64 = 86 + 118 + 83;

/// RNS 1.4.2 `Resource.PROOF_TIMEOUT_FACTOR`. The smaller rtt multiple a sender waits on the proof.
pub const PROOF_TIMEOUT_FACTOR: u64 = 3;

/// RNS 1.4.2 `Resource.PROCESSING_GRACE`
pub const PROCESSING_GRACE_MS: u64 = 1_000;

/// RNS 1.4.2 `Resource.RETRY_GRACE_TIME`
pub const RETRY_GRACE_MS: u64 = 250;

/// RNS 1.4.2 `Resource.PER_RETRY_DELAY`
pub const PER_RETRY_DELAY_MS: u64 = 500;

/// RNS 1.4.2 `Resource.SENDER_GRACE_TIME`
pub const SENDER_GRACE_MS: u64 = 10_000;

/// Our seam, no reference analog: how long a transfer may sit at `AwaitingDecompression` before the receiver gives up on its host's inflate.
/// A host that can inflate answers in milliseconds; one that cannot would otherwise pin the table slot and the link's one-resource lane forever.
pub const DECOMPRESSION_GRACE_MS: u64 = 10_000;

/// Our seam, no reference analog: how long a complete transfer may sit at `AwaitingOpen` before the receiver gives up on its pool's span verdict.
/// A live worker answers in milliseconds; a dead pool would otherwise pin the slot and the link's one-resource lane forever.
pub const OPEN_VERDICT_GRACE_MS: u64 = 1_000;

/// RNS 1.4.2 `ResourceAdvertisement.OVERHEAD`: the byte budget the reference reserves for everything in a packed advertisement except the map hashes.
pub const ADVERTISEMENT_OVERHEAD: usize = 134;

/// RNS 1.4.2 `ResourceAdvertisement.HASHMAP_MAX_LEN` (74): how many map hashes ride one advertisement or one hashmap update.
///
/// Derived from the base link MDU (431), never the negotiated one, so every link lands on the same figure regardless of its MTU.
pub const HASHMAP_MAX_LEN: usize = (LINK_MDU - ADVERTISEMENT_OVERHEAD) / MAP_HASH_LEN;

/// RNS 1.4.2 `ResourceAdvertisement.COLLISION_GUARD_SIZE` (224): The sliding span of parts within which two map hashes must not collide.
/// The sender re-rolls its salt nonce until they don't.
pub const COLLISION_GUARD_SIZE: usize = 2 * WINDOW_MAX + HASHMAP_MAX_LEN;

/// RNS 1.4.2 `Resource.MAX_EFFICIENT_SIZE` (1 MiB − 1). The most one segment carries.
///
/// Anything larger splits into segments of this size, each transferred as its own resource sharing the first segment's hash.
pub const MAX_EFFICIENT_SIZE: usize = 1024 * 1024 - 1;

/// RNS 1.4.2 `Resource.METADATA_MAX_SIZE` (16 MiB − 1)
pub const METADATA_MAX_SIZE: usize = 16 * 1024 * 1024 - 1;

/// The length prefix ahead of the packed metadata in the stream: `struct.pack(">I", metadata_size)[1:]` (RNS 1.4.2 `Resource.__init__`).
pub const METADATA_PREFIX_LEN: usize = 3;

/// RNS 1.4.2 `Resource.sdu`
pub const fn resource_sdu(mtu: usize) -> usize {
    mtu - HEADER_MAX_LEN - IFAC_MIN_LEN
}

/// IV ‖ PKCS#7-padded(stream nonce ‖ stream) ‖ MAC.
pub const fn sealed_transfer_bytes(stream_len: usize) -> usize {
    let padded = ((stream_len + RESOURCE_NONCE_LEN) / 16 + 1) * 16;
    16 + padded + 32
}

/// The part count at the broadcast-MTU sdu. This is the floor every link clears, so the worst case a store must name.
pub const fn max_part_count(transfer_capacity: usize) -> usize {
    transfer_capacity.div_ceil(resource_sdu(BROADCAST_MTU))
}

/// Default active Resource bulk-buffer budget for each direction on heap hosts.
pub const DEFAULT_RESOURCE_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Heap-host limits for the transfer, part-name, and part-state buffers owned by
/// active Resources. Incoming and outgoing traffic have independent budgets;
/// zero disables Resource buffers in that direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceMemoryLimits {
    pub incoming_bytes: usize,
    pub outgoing_bytes: usize,
}

impl ResourceMemoryLimits {
    pub const DEFAULT_HOST: Self = Self {
        incoming_bytes: DEFAULT_RESOURCE_MEMORY_BYTES,
        outgoing_bytes: DEFAULT_RESOURCE_MEMORY_BYTES,
    };
}

impl Default for ResourceMemoryLimits {
    fn default() -> Self {
        Self::DEFAULT_HOST
    }
}

/// The bulk regions one active Resource row needs.
///
/// Heap-backed tables allocate these lengths exactly. Fixed tables retain their
/// compile-time storage and use the shape to reject rows that cannot fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBufferShape {
    transfer_bytes: usize,
    part_count: usize,
    buffer_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceBufferShapeError {
    EmptyTransfer,
    SduTooSmall,
    SizeOverflow,
}

pub(crate) const fn checked_resource_buffer_bytes(
    transfer_bytes: usize,
    part_count: usize,
) -> Option<usize> {
    let part_bytes = match part_count.checked_mul(MAP_HASH_LEN + core::mem::size_of::<bool>()) {
        Some(part_bytes) => part_bytes,
        None => return None,
    };
    transfer_bytes.checked_add(part_bytes)
}

impl ResourceBufferShape {
    pub const fn try_for_transfer(
        transfer_bytes: usize,
        sdu: usize,
    ) -> Result<Self, ResourceBufferShapeError> {
        if transfer_bytes == 0 {
            return Err(ResourceBufferShapeError::EmptyTransfer);
        }
        if sdu == 0 {
            return Err(ResourceBufferShapeError::SduTooSmall);
        }
        let part_count = transfer_bytes.div_ceil(sdu);
        let buffer_bytes = match checked_resource_buffer_bytes(transfer_bytes, part_count) {
            Some(buffer_bytes) => buffer_bytes,
            None => return Err(ResourceBufferShapeError::SizeOverflow),
        };
        Ok(Self {
            transfer_bytes,
            part_count,
            buffer_bytes,
        })
    }

    #[must_use]
    pub const fn transfer_bytes(self) -> usize {
        self.transfer_bytes
    }

    #[must_use]
    pub const fn part_count(self) -> usize {
        self.part_count
    }

    /// Transfer bytes plus one map hash and one byte of part state per part.
    #[must_use]
    pub const fn buffer_bytes(self) -> usize {
        self.buffer_bytes
    }
}

/// The most frames one inbound resource request can synchronously ask the engine to emit.
///
/// A request names at most [`WINDOW_MAX`] existing parts. When the receiver has exhausted its
/// current hashmap segment, the same reaction can append one hashmap update. Storage recipes that
/// cannot hold a full window need capacity only for every part they can actually materialize plus
/// that update.
pub const fn max_outgoing_resource_reaction_frames(transfer_capacity: usize) -> usize {
    let parts = max_part_count(transfer_capacity);
    if parts < WINDOW_MAX {
        parts + 1
    } else {
        WINDOW_MAX + 1
    }
}

#[cfg(test)]
mod reaction_capacity_tests {
    use super::*;

    #[test]
    fn resource_buffer_shape_derives_parts_and_rejects_invalid_inputs() {
        let shape = ResourceBufferShape::try_for_transfer(928, 464).unwrap();
        assert_eq!(shape.transfer_bytes(), 928);
        assert_eq!(shape.part_count(), 2);
        assert_eq!(
            ResourceBufferShape::try_for_transfer(929, 464)
                .unwrap()
                .part_count(),
            3,
        );
        assert_eq!(
            ResourceBufferShape::try_for_transfer(0, 464),
            Err(ResourceBufferShapeError::EmptyTransfer),
        );
        assert_eq!(
            ResourceBufferShape::try_for_transfer(928, 0),
            Err(ResourceBufferShapeError::SduTooSmall),
        );
        assert_eq!(
            ResourceBufferShape::try_for_transfer(usize::MAX, 1),
            Err(ResourceBufferShapeError::SizeOverflow),
        );
    }

    #[test]
    fn outbound_reaction_capacity_tracks_small_stores_and_caps_at_one_full_window() {
        let part = resource_sdu(BROADCAST_MTU);
        assert_eq!(max_outgoing_resource_reaction_frames(0), 1);
        assert_eq!(max_outgoing_resource_reaction_frames(18 * part), 19);
        assert_eq!(max_outgoing_resource_reaction_frames(WINDOW_MAX * part), 76);
        assert_eq!(max_outgoing_resource_reaction_frames(100 * part), 76);
    }
}

/// RNS 1.4.2 `Resource.get_map_hash`; the four-byte name a part is requested by `full_hash(part ‖ salt nonce)` truncated.
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

/// The advertisement's `r`; the reference calls it `random_hash` but it's truly a nonce, not a hash.
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

/// RNS 1.4.2 `Link.resource_strategy`, engine-gated: the reference's unbounded `ACCEPT_ALL` becomes an accept with enforced bounds, refused at the advertisement gate before a single part moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceStrategy {
    /// Every link is born refusing resources; RNS 1.4.2 `Link.__init__` sets `ACCEPT_NONE`.
    #[default]
    AcceptNone,
    Accept {
        max_uncompressed_bytes: u64,
        accept_compressed: bool,
    },
    /// RNS 1.4.2 `ACCEPT_APP`: the host's decider judges each unsolicited advertisement from its [`ResourceOffer`].
    /// A declined offer answers with a receiver-cancel — the reference's `Resource.reject` — so the sender settles instead of timing out.
    AcceptIf,
}

/// RNS 1.4.2 hands the `ACCEPT_APP` callback the parsed advertisement; this is that view: one unsolicited segment's facts, judged before a single part moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceOffer {
    pub link_id: LinkId,
    pub remote_identity: Option<crate::identity::IdentityHash>,
    pub hash: ResourceHash,
    /// The advertised `d`: on a split transfer this is the WHOLE transfer's uncompressed length, on every segment.
    pub uncompressed_data_bytes: u64,
    /// This segment's sealed stream length on the wire.
    pub sealed_transfer_bytes: usize,
    pub part_count: usize,
    pub segment_index: u64,
    pub total_segment_count: u64,
    pub compression: ResourceCompression,
    pub has_metadata: bool,
}

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

    /// bz2 is the only compression RNS 1.4.2 can mean by the `c` flag.
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

/// The reference's keep-only-if-smaller rule picks between payload and precompressed attempt at buildup time; a host that links no compressor just passes `None`.
///
/// `metadata` rides ahead of `data` in the stream (so a `compressed_candidate` must be compressed over `metadata_block ‖ data`, exactly the composite the reference feeds bz2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBody<'a> {
    pub data: &'a [u8],
    pub compressed_candidate: Option<&'a [u8]>,
    pub metadata: ResourceMetadata<'a>,
}

/// RNS 1.4.2 resource metadata on the send side: packed msgpack bytes the engine never unpacks,  carried at the head of the stream as `3-byte-BE-length ‖ packed` and covered by the resource hash, the advertised `d`, and (when one wins) the compressed stream.
///
/// On a split transfer the block rides segment one only, but every segment advertises the metadata flag and a `d` that includes the block.
/// The reference threads this through `sent_metadata_size`; [`ResourceMetadata::SentInFirstSegment`] is that parameter by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceMetadata<'a> {
    #[default]
    None,
    /// This (first or only) segment carries the block in-stream.
    Packed(&'a [u8]),
    /// A later segment of a split whose first segment carried the block: the flag and the `d` accounting still travel, the bytes do not.
    SentInFirstSegment { packed_len: u32 },
}

impl<'a> ResourceMetadata<'a> {
    /// Prefix plus packed bytes: what the block adds to the advertised `d` on every segment.
    #[must_use]
    pub const fn block_len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Packed(packed) => METADATA_PREFIX_LEN + packed.len(),
            Self::SentInFirstSegment { packed_len } => METADATA_PREFIX_LEN + *packed_len as usize,
        }
    }

    /// Whether the advertisement's metadata flag travels.
    #[must_use]
    pub const fn travels(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// RNS 1.4.2 advertises `(segment_index, total_segments)` plus the whole transfer's uncompressed length (the `d` field) on every segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSegment {
    pub index: u64,
    pub total_segments: u64,
    pub total_data_bytes: u64,
}

impl ResourceSegment {
    #[must_use]
    pub fn whole(data_len: u64) -> Self {
        Self {
            index: 1,
            total_segments: 1,
            total_data_bytes: data_len,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePartRequest<'a> {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub requested: &'a [u8],
    pub last_known_map_hash: Option<[u8; MAP_HASH_LEN]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceCorrelation {
    #[default]
    Unsolicited,
    Request {
        id: RequestId,
        response_timeout: crate::engine::RequestResponseTimeout,
        maximum_response_bytes: crate::units::ByteLimit,
    },
    Response(RequestId),
}

impl ResourceCorrelation {
    #[must_use]
    pub fn request_id(self) -> Option<RequestId> {
        match self {
            Self::Unsolicited => None,
            Self::Request { id, .. } | Self::Response(id) => Some(id),
        }
    }

    #[must_use]
    pub const fn is_request(self) -> bool {
        matches!(self, Self::Request { .. })
    }

    #[must_use]
    pub const fn is_response(self) -> bool {
        matches!(self, Self::Response(_))
    }
}

/// Why an incoming transfer died.
/// The reference has no analogous signal. A stock RNS receiver's failures surface only in its own logs; ours ride the failure event by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFailureCause {
    CancelledBySender,
    RefusedHashmapUpdate(table::ApplyHashmapUpdateError),
    RetriesExhausted,
    LinkVanished,
    TransferUnopenable,
    TransferCorrupt,
    ProofUnsendable,
    DecompressionFailed,
    DecompressionTimedOut,
    OpenTimedOut,
    /// The verified stream's own metadata prefix declares more bytes than the stream holds.
    /// Intentional deviation: the reference silently delivers truncated metadata and empty data when the declared length overruns (Python slice leniency); we fail the transfer by name.
    MetadataOverrun,
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

/// RNS 1.4.2 `expected_proof = Identity.full_hash(data + hash)`.
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
