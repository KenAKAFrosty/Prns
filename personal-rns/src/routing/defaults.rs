//! Sizing defaults for the no_std routing-table preset, plus the small
//! emission-policy knobs that derive from them.
//!
//! Each `DEFAULT_*` here is one knob of the default backend stack
//! (`FixedArrayRouteColumns`, `FixedArrayRetainedAnnounceColumns`,
//! `TieredAnnounceIdHistory`, `PackedAppDataArena`). Capable hosts widen each
//! independently. These are LOCAL policy choices — nothing here is on the
//! wire; peers do not need to agree on any of them. (The actual wire-protocol
//! invariants live in [`crate::wire`].)

use crate::wire::DestinationHash;

/// How long a learned route stays valid without a refresh. RNS's
/// `Transport.PATHFINDER_E` uses one week; we mirror it as the default but
/// nothing on the wire enforces this — each node decides when its own cache
/// invalidates.
pub(crate) const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

/// How many destinations the table tracks. Fixed-capacity for the
/// no-allocator targets; a new destination arriving past this is dropped
/// (v1 policy).
pub const DEFAULT_MAX_TRACKED_DESTINATIONS: usize = 64;

/// Per-destination cap on remembered announce ids used by the replay-defence
/// predicate. RNS's `Transport.MAX_RANDOM_BLOBS` uses 64; this is a local
/// memory bound, not a wire-observable constant — each node decides its own
/// cap independently.
pub const DEFAULT_ANNOUNCE_ID_HISTORY_CAP_PER_DESTINATION: usize = 64;

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

/// How wide a window (in milliseconds) to spread re-emissions of accepted
/// announces across. Each accepted destination gets a per-(entropy,
/// destination) jitter offset into `[0, JITTER_WINDOW_MS)` so a flood of
/// simultaneous receives doesn't turn into a synchronised re-broadcast burst.
/// Mirrors RNS's `Transport.PATHFINDER_RW` (500 ms); the value is RNS-derived
/// but local — peers don't enforce any particular timing.
pub const DEFAULT_REBROADCAST_JITTER_WINDOW_MS: u64 = 500;

/// Derive a deterministic per-destination jitter offset from a `(entropy,
/// destination)` pair. Returned value is `< window_ms`, so callers add it
/// directly to an arrival instant to get a due-time inside the jitter window.
///
///  REVIEW please be more specific here in this comment
/// Not crypto — just enough mixing that two destinations accepted in the same
/// batch land at different offsets. Determinism is the contract: same
/// `(entropy, destination, window_ms)` always returns the same value.
pub(crate) fn jitter_offset_for(
    entropy: u64,
    destination: &DestinationHash,
    window_ms: u64,
) -> u64 {
    // A wyhash-style splitmix step per 8 bytes of the destination is
    // sufficient mixing here; the goal is "spread out", not "indistinguishable
    // from random". The 16-byte destination is exactly two u64 chunks.
    let mut h = entropy;
    for chunk in destination.as_bytes().chunks_exact(8) {
        let word = u64::from_le_bytes(chunk.try_into().expect("chunks_exact(8) yields 8 bytes"));
        h ^= word;
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h = h.rotate_left(27);
    }
    if window_ms == 0 {
        0
    } else {
        h % window_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn jitter_is_deterministic_and_inside_the_window() {
        let window = 500;
        let a = jitter_offset_for(0xCAFE_F00D, &dest(0x11), window);
        let b = jitter_offset_for(0xCAFE_F00D, &dest(0x11), window);
        assert_eq!(a, b, "same inputs must produce the same offset");
        assert!(a < window, "offset must be inside the jitter window");
    }

    #[test]
    fn distinct_destinations_typically_get_distinct_offsets() {
        // The mix is not crypto, but two destinations under the same entropy
        // must land at different points in a 500ms window or there's no point
        // jittering. Spot-check a handful — they should not all collide.
        let window = 500;
        let entropy = 0xDEAD_BEEF;
        let offsets: std::vec::Vec<u64> = (0u8..8)
            .map(|b| jitter_offset_for(entropy, &dest(b), window))
            .collect();
        let unique: std::collections::BTreeSet<_> = offsets.iter().collect();
        assert!(unique.len() >= 7, "expected ~all distinct, got {offsets:?}");
    }

    #[test]
    fn varying_entropy_moves_the_offset() {
        let window = 500;
        let d = dest(0x42);
        let lo = jitter_offset_for(0, &d, window);
        let hi = jitter_offset_for(u64::MAX, &d, window);
        assert_ne!(
            lo, hi,
            "different entropy should not collapse to the same offset"
        );
    }

    #[test]
    fn zero_window_yields_zero_offset() {
        assert_eq!(jitter_offset_for(0xCAFE, &dest(0xAA), 0), 0);
    }
}
