//! Storage traits and impls for the destination table.
//!
//! The destination table is composed of three independently-substitutable
//! storage concerns, each behind its own trait:
//!
//! - [`DestinationColumns`] — the struct-of-arrays columns that hold one row
//!   per tracked destination.
//! - [`SeenAnnounceIdsStorage`] — the per-path set of recently-seen announce
//!   ids (replay defence; tiered today, flat on capable hosts later).
//! - [`AppDataBackend`] — the variable-length retained announce `app_data`
//!   arena.
//!
//! A `DestinationTable<C, S, P>` composes one impl of each. Today's defaults
//! are the no_std backends (`FixedArrayColumns`, `TieredSeenAnnounceIds`,
//! `PayloadStore`). The std-host backends slot in behind a feature flag
//! later, mixing-and-matching with the no_std ones as needed (e.g. an mmap
//! columns impl + RAM seen-ids + journaled app_data).
//!
//! ## SoA all the way down
//!
//! [`DestinationColumns`] exposes each column as a `&[T]` slice so consumers'
//! hot scans walk one dense column at a time and SoA cache wins are
//! preserved. Writes are row-coherent
//! ([`set_row`](DestinationColumns::set_row),
//! [`push`](DestinationColumns::push)) so backends that need transactional
//! writes (journaled, disk-backed) have one atomicity boundary per cold-field
//! refresh. There is intentionally no `row()` reader: that would invite
//! AoS-shaped reads against SoA storage and defeat the layout.

pub mod fixed_array_columns;
pub mod payload_store;
pub mod tiered_seen_ids;

pub use fixed_array_columns::FixedArrayColumns;
pub use payload_store::{PayloadHandle, PayloadStore, PayloadStoreError};
pub use tiered_seen_ids::{SeenAnnounceIds, TieredSeenAnnounceIds};

use crate::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::crypto::Ed25519Signature;
use crate::engine::InstantMillis;
use crate::path::PathResponsiveness;
use crate::wire::DestinationHash;

/// The cold per-path fields gathered into one row. Writes happen row-coherent
/// (via [`DestinationColumns::set_row`] / [`DestinationColumns::push`]); reads
/// are per-column through the slice accessors, so this struct only appears on
/// the write side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathRow {
    pub hops: u8,
    pub expires: InstantMillis,
    pub responsiveness: PathResponsiveness,
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub retained_announce_id: AnnounceId,
    pub signature: Ed25519Signature,
    pub maybe_ratchet: Option<RatchetKey>,
    pub maybe_app_data_handle: Option<PayloadHandle>,
}

/// Raised by [`DestinationColumns::push`] when the backend can't accept
/// another row. Bounded backends (fixed-array, bounded-heap) produce this on
/// the capacity boundary; growable backends never produce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFull;

/// What [`SeenAnnounceIdsStorage::remember`] reports back from one insert.
/// Mirrors the (then-private) outcome enum from the tiered store; promoted to
/// public on the trait so any backend reports the same vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredEvictingOldest,
}

/// Struct-of-arrays storage of the path table's per-destination columns.
///
/// Reads are per-column slices so SoA scans stay cache-friendly. Writes go
/// through `set_row`/`push` as a coherent unit so backends that need
/// transactional writes have one atomicity boundary per cold-field refresh.
/// Partial-write methods (e.g. flipping responsiveness) are deliberately
/// absent for now; they'll land when a real consumer demands them.
pub trait DestinationColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    /// Default to `len() == 0` — backends should override only if they have a
    /// cheaper way to answer (most don't).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn hops(&self) -> &[u8];
    fn expires(&self) -> &[InstantMillis];
    fn responsiveness(&self) -> &[PathResponsiveness];
    fn public_keys(&self) -> &[IdentityPublicKeys];
    fn dotted_name_hash(&self) -> &[DottedNameHash];
    fn retained_announce_id(&self) -> &[AnnounceId];
    fn ratchet(&self) -> &[Option<RatchetKey>];
    fn signature(&self) -> &[Ed25519Signature];
    fn app_data_handle(&self) -> &[Option<PayloadHandle>];

    /// Row-coherent overwrite of slot `i`'s cold fields. Backends that need
    /// transactional writes anchor atomicity here.
    fn set_row(&mut self, i: usize, row: PathRow);

    /// Append a new row. Bounded backends return [`ColumnsFull`] when
    /// `len == capacity`; growable backends never error.
    fn push(&mut self, destination: DestinationHash, row: PathRow) -> Result<usize, ColumnsFull>;
}

/// Per-path set of recently-seen announce ids (RNS's `random_blobs`) for
/// replay defence. The trait keeps the per-path cap inside the backend so
/// consumers don't pass it on every call; backends choose the cap as part of
/// their construction.
pub trait SeenAnnounceIdsStorage {
    fn seen_ids(&self, slot: usize) -> SeenAnnounceIds<'_>;
    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome;
}

/// Variable-length retained-announce `app_data` storage behind opaque handles.
/// The same shape today's [`PayloadStore`] exposes; lifted to a trait so a
/// future Vec-backed or mmap-backed arena can substitute.
pub trait AppDataBackend {
    fn get(&self, handle: PayloadHandle) -> &[u8];
    fn insert(&mut self, bytes: &[u8]) -> Result<PayloadHandle, PayloadStoreError>;
    fn replace(&mut self, handle: PayloadHandle, bytes: &[u8]) -> Result<(), PayloadStoreError>;
}
