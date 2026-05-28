//! Sizing defaults for the no_std routing-table preset.
//!
//! Each constant here sizes one knob of the default backend stack
//! (`FixedArrayRouteColumns`, `TieredAnnounceIdHistory`, `PackedAppDataArena`).
//! Capable hosts widen each independently. The two RNS-derived protocol
//! constants live alongside as `pub` so consumers can see what's a hard
//! protocol value vs what's a preset sizing choice.

/// RNS's `RNS.Transport.PATHFINDER_E` — how long a learned route stays valid
/// without a refresh. Protocol value, not a sizing knob.
pub(crate) const DEFAULT_PATH_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

/// RNS's `RNS.Transport.MAX_RANDOM_BLOBS` — the per-destination cap on
/// remembered announce ids the predicate consults for replay defence.
/// Protocol value, not a sizing knob.
pub const DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = 64;

/// How many destinations the table tracks. Fixed-capacity for the
/// no-allocator targets; a new destination arriving past this is dropped
/// (v1 policy).
pub const DEFAULT_MAX_TRACKED_DESTINATIONS: usize = 32;

/// Total byte budget for retained announce `app_data` (the one variable,
/// genuinely-opaque tail of an announce — the rest is stored as structured
/// columns and serialized back to wire via `Announce::to_wire` on
/// re-emission). Sized as an average per destination rather than worst-case
/// × destination count, so a `PackedAppDataArena` backs a full table at a
/// fraction of the worst-case footprint. A capable host widens this
/// independently of the destination count.
pub const DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_MAX_TRACKED_DESTINATIONS * 256;

/// Per-destination inline floor for `AnnounceIdHistory` retention. Every
/// tracked destination is guaranteed this many slots regardless of arena
/// pressure — covers the typical multipath dedup fan-in (interfaces × routes)
/// for small-to-medium mesh deployments. See
/// [`crate::routing::storage::TieredAnnounceIdHistory`] for the full reasoning.
pub const DEFAULT_HISTORY_FLOOR_PER_DESTINATION: usize = 4;

/// Total shared overflow capacity for `AnnounceIdHistory`. Sized as the
/// destination count × an average per-destination overflow draw rather than
/// worst-case. Chatty destinations borrow against the budget up to
/// `MAX_ANNOUNCE_IDS_PER_DESTINATION`; quiet ones leave it for the chatty
/// ones. A capable host widens this independently of the floor.
pub const DEFAULT_HISTORY_OVERFLOW_CAPACITY: usize = DEFAULT_MAX_TRACKED_DESTINATIONS * 8;
