//! Storage traits and impls for the routing table.
//!
//! Each routing concern has its own substitutable storage trait:
//!
//! - [`RouteColumns`] — the struct-of-arrays columns that hold one row
//!   per tracked destination.
//! - [`AnnounceIdHistory`] — the per-destination accumulated history of
//!   announce ids heard (replay defence; tiered today, heap-backed on
//!   capable hosts later).
//! - [`RetainedAppData`] — the variable-length `app_data` tail of each
//!   retained announce, kept behind opaque [`AppDataHandle`]s.
//!
//! A `RoutingTable<C, S, P>` composes one impl of each. Today's defaults
//! are the no_std backends (`FixedArrayRouteColumns`,
//! `TieredAnnounceIdHistory`, `PackedAppDataArena`). The std-host backends
//! slot in behind a feature flag later, mixing-and-matching with the
//! no_std ones as needed (e.g. an mmap columns impl + RAM history +
//! journaled app_data).
//!
//! ## SoA all the way down
//!
//! [`RouteColumns`] exposes each column as a `&[T]` slice so consumers'
//! hot scans walk one dense column at a time and SoA cache wins are
//! preserved. Writes are row-coherent
//! ([`set_row`](RouteColumns::set_row),
//! [`push`](RouteColumns::push)) so backends that need transactional
//! writes (journaled, disk-backed) have one atomicity boundary per cold-field
//! refresh. There is intentionally no `row()` reader: that would invite
//! AoS-shaped reads against SoA storage and defeat the layout.

mod fixed_array_retained_announce_columns;
mod fixed_array_route_columns;
mod packed_app_data_arena;
mod tiered_announce_id_history;

pub use fixed_array_retained_announce_columns::FixedArrayRetainedAnnounceColumns;
pub use fixed_array_route_columns::FixedArrayRouteColumns;
pub use packed_app_data_arena::{AppDataHandle, PackedAppDataArena, RetainedAppDataError};
pub use tiered_announce_id_history::{AnnounceIdHistoryView, TieredAnnounceIdHistory};

use crate::crypto::Ed25519Signature;
use crate::engine::InstantMillis;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::routing::RouteResponsiveness;
use crate::wire::DestinationHash;

/// The routing-coherent row written to [`RouteColumns`] — fields whose
/// updates are independent of the announce payload (hops/expires update on
/// every accept; responsiveness flips on path probes; future next_hop +
/// interface land here too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub hops: u8,
    pub expires: InstantMillis,
    pub responsiveness: RouteResponsiveness,
}

/// The announce-coherent row written to [`RetainedAnnounceColumns`] — fields
/// the signature couples and that must refresh as a unit (signature signs
/// the rest; replacing any one without the others would leave the retained
/// announce unverifiable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedAnnounceEntry {
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub retained_announce_id: AnnounceId,
    pub signature: Ed25519Signature,
    pub maybe_ratchet: Option<RatchetKey>,
    pub maybe_app_data_handle: Option<AppDataHandle>,
}

/// Raised by [`RouteColumns::push`] when the backend can't accept
/// another row. Bounded backends (fixed-array, bounded-heap) produce this on
/// the capacity boundary; growable backends never produce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFull;

/// What [`AnnounceIdHistory::remember`] reports back from one insert.
/// Mirrors the (then-private) outcome enum from the tiered store; promoted to
/// public on the trait so any backend reports the same vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredEvictingOldest,
}

/// SoA columns for the routing table — the routing-coherent fields,
/// keyed by destination. This is the primary destination index in the
/// crate: its `destinations()` slice is the linear-scan ground truth and
/// its slot index is what the other routing-storage traits join against.
///
/// Reads are per-column slices so SoA scans stay cache-friendly. Writes go
/// through `set_row`/`push` as a coherent unit so backends that need
/// transactional writes have one atomicity boundary per cold-field refresh.
/// Partial-write methods (e.g. flipping responsiveness) are deliberately
/// absent for now; they'll land when a real consumer demands them.
pub trait RouteColumns {
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
    fn responsiveness(&self) -> &[RouteResponsiveness];

    /// Row-coherent overwrite of slot `i`'s routing fields. Backends that
    /// need transactional writes anchor atomicity here.
    fn set_row(&mut self, i: usize, row: RouteEntry);

    /// Append a new row. Bounded backends return [`ColumnsFull`] when
    /// `len == capacity`; growable backends never error.
    fn push(&mut self, destination: DestinationHash, row: RouteEntry)
        -> Result<usize, ColumnsFull>;
}

/// SoA columns for the retained-announces table — the announce-coherent
/// fields the signature couples. Slot-indexed by the [`RouteColumns`] slot
/// for the same destination; no destination column of its own. Writes are
/// whole-row (signature signs the rest of the row, so partial updates
/// would leave the retained announce unverifiable).
pub trait RetainedAnnounceColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn public_keys(&self) -> &[IdentityPublicKeys];
    fn dotted_name_hash(&self) -> &[DottedNameHash];
    fn retained_announce_id(&self) -> &[AnnounceId];
    fn ratchet(&self) -> &[Option<RatchetKey>];
    fn signature(&self) -> &[Ed25519Signature];
    fn app_data_handle(&self) -> &[Option<AppDataHandle>];

    /// Row-coherent overwrite of slot `i`'s announce fields.
    fn set_row(&mut self, i: usize, row: RetainedAnnounceEntry);

    /// Append a new row. The caller (engine) is responsible for keeping the
    /// retained-announces slot in lockstep with the routes-columns slot —
    /// they share the destination index.
    fn push(&mut self, row: RetainedAnnounceEntry) -> Result<usize, ColumnsFull>;
}

/// Per-destination accumulated history of announce ids we've heard (RNS's
/// `random_blobs`) — used by the acceptance predicate for replay defence.
/// The trait keeps the per-destination cap inside the backend so consumers
/// don't pass it on every call; backends choose the cap as part of their
/// construction.
pub trait AnnounceIdHistory {
    fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_>;
    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome;
}

/// Variable-length retained-announce `app_data` storage behind opaque handles.
/// The same shape today's [`PackedAppDataArena`] exposes; lifted to a trait so a
/// future Vec-backed or mmap-backed arena can substitute.
pub trait RetainedAppData {
    fn get(&self, handle: AppDataHandle) -> &[u8];
    fn insert(&mut self, bytes: &[u8]) -> Result<AppDataHandle, RetainedAppDataError>;
    fn replace(&mut self, handle: AppDataHandle, bytes: &[u8]) -> Result<(), RetainedAppDataError>;
}
