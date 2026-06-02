//! Default sizing and timing knobs for the no_std routing preset.
//!
//! These are local policy values, not wire-format constants.

use crate::wire::DestinationHash;

/// How long a learned route stays valid without a refresh.
pub(crate) const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

/// How many destinations the default table tracks.
pub const DEFAULT_MAX_TRACKED_DESTINATIONS: usize = 64;

/// Per-destination cap on remembered announce ids.
pub const DEFAULT_ANNOUNCE_ID_HISTORY_CAP_PER_DESTINATION: usize = 64;

/// Per-destination guaranteed inline history slots.
pub const DEFAULT_HISTORY_FLOOR_PER_DESTINATION: usize = 4;

/// Average per-destination `app_data` budget used to size the default arena.
pub const DEFAULT_AVG_APP_DATA_BYTES_PER_DESTINATION: usize = 64;

/// Average per-destination history overflow budget.
pub const DEFAULT_AVG_HISTORY_OVERFLOW_PER_DESTINATION: usize = 8;

/// Total byte budget for retained announce `app_data`.
pub const DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES: usize =
    DEFAULT_MAX_TRACKED_DESTINATIONS * DEFAULT_AVG_APP_DATA_BYTES_PER_DESTINATION;

/// Total shared overflow capacity for announce-id history.
pub const DEFAULT_HISTORY_OVERFLOW_CAPACITY: usize =
    DEFAULT_MAX_TRACKED_DESTINATIONS * DEFAULT_AVG_HISTORY_OVERFLOW_PER_DESTINATION;

/// Width of the rebroadcast jitter window.
pub const DEFAULT_REBROADCAST_JITTER_WINDOW_MS: u64 = 500;

/// Seed for deterministic rebroadcast spreading.
#[derive(Debug, Clone, Copy)]
pub struct JitterSeed(pub u64);

/// Deterministically spread a destination into the rebroadcast window.
pub(crate) fn jitter_offset_for(
    seed: JitterSeed,
    destination: &DestinationHash,
    window_ms: u64,
) -> u64 {
    let mut h = seed.0;
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
        let a = jitter_offset_for(JitterSeed(0xCAFE_F00D), &dest(0x11), window);
        let b = jitter_offset_for(JitterSeed(0xCAFE_F00D), &dest(0x11), window);
        assert_eq!(a, b, "same inputs must produce the same offset");
        assert!(a < window, "offset must be inside the jitter window");
    }

    #[test]
    fn distinct_destinations_typically_get_distinct_offsets() {
        let window = 500;
        let entropy = JitterSeed(0xDEAD_BEEF);
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
        let lo = jitter_offset_for(JitterSeed(0), &d, window);
        let hi = jitter_offset_for(JitterSeed(u64::MAX), &d, window);
        assert_ne!(
            lo, hi,
            "different entropy should not collapse to the same offset"
        );
    }

    #[test]
    fn zero_window_yields_zero_offset() {
        assert_eq!(jitter_offset_for(JitterSeed(0xCAFE), &dest(0xAA), 0), 0);
    }
}
