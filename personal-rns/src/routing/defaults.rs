//! Sizing defaults for the no_std routing-table preset.
//!
//! Each `DEFAULT_*` here is one knob of the default backend stack
//! (`FixedArrayRouteColumns`, `FixedArrayRetainedAnnounceColumns`,
//! `TieredAnnounceIdHistory`, `PackedAppDataArena`). Capable hosts widen each
//! independently. These are LOCAL policy choices — nothing here is on the
//! wire; peers do not need to agree on any of them. (The actual wire-protocol
//! invariants live in [`crate::wire`].)

/// How long a learned route stays valid without a refresh. RNS's
/// `Transport.PATHFINDER_E` uses one week; we mirror it as the default but
/// nothing on the wire enforces this — each node decides when its own cache
/// invalidates.
pub(crate) const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

/// How many destinations the table tracks. Fixed-capacity for the
/// no-allocator targets; a new destination arriving past this is dropped
/// (v1 policy).
pub const DEFAULT_MAX_TRACKED_DESTINATIONS: usize = 512;

/// Per-destination cap on remembered announce ids used by the replay-defence
/// predicate. RNS's `Transport.MAX_RANDOM_BLOBS` uses 64; this is a local
/// memory bound, not a wire-observable constant — each node decides its own
/// cap independently.
pub const DEFAULT_HISTORY_CAP_PER_DESTINATION: usize = 64;

/// Per-destination inline floor for `AnnounceIdHistory` retention. Every
/// tracked destination is guaranteed this many slots regardless of arena
/// pressure — covers the typical multipath dedup fan-in (interfaces × routes)
/// for small-to-medium mesh deployments. See
/// [`crate::routing::storage::TieredAnnounceIdHistory`] for the full reasoning.
pub const DEFAULT_HISTORY_FLOOR_PER_DESTINATION: usize = 4;

/// Average per-destination `app_data` budget the no_std preset sizes its
/// arena against. Real announces carry sub-50-byte app_data in practice
/// (Sideband / LXMF); the arena is sized as `MAX_TRACKED × AVG_PER_DEST`
/// rather than worst-case-per-destination so chatty destinations can borrow
/// against quiet ones' share.
pub const DEFAULT_AVG_APP_DATA_BYTES_PER_DESTINATION: usize = 64;

/// Average per-destination overflow draw for `AnnounceIdHistory`. The shared
/// overflow arena is sized as `MAX_TRACKED × AVG_OVERFLOW_PER_DEST`; chatty
/// destinations borrow up to `MAX_ANNOUNCE_IDS_PER_DESTINATION`, quiet ones
/// leave their share for the chatty ones.
pub const DEFAULT_AVG_HISTORY_OVERFLOW_PER_DESTINATION: usize = 8;

/// Total byte budget for retained announce `app_data`. The one variable,
/// genuinely-opaque tail of an announce — the rest is stored as structured
/// columns and serialized back to wire via `Announce::to_wire` on
/// re-emission. A capable host widens this independently of the destination
/// count.
pub const DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES: usize =
    DEFAULT_MAX_TRACKED_DESTINATIONS * DEFAULT_AVG_APP_DATA_BYTES_PER_DESTINATION;

/// Total shared overflow capacity for `AnnounceIdHistory`. A capable host
/// widens this independently of the floor.
pub const DEFAULT_HISTORY_OVERFLOW_CAPACITY: usize =
    DEFAULT_MAX_TRACKED_DESTINATIONS * DEFAULT_AVG_HISTORY_OVERFLOW_PER_DESTINATION;
