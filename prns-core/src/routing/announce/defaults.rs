//! Timing knobs for the no_std routing preset: local policy values, not wire-format
//! constants. Storage sizing has no defaults; every consumer picks its own.

use crate::interfaces::InterfaceMode;
use crate::wire::DestinationHash;

pub const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

pub const ACCESS_POINT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 1000;

pub const ROAMING_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 6 * 1000;

/// RNS 1.3.5 mode-keyed path lifetimes (Transport.py:1875-1880): an access-point
/// path lives a day, a roaming path six hours, everything else the full week.
pub fn route_expiry_millis(mode: InterfaceMode) -> u64 {
    use InterfaceMode::{AccessPoint, Boundary, Full, Gateway, PointToPoint, Roaming};
    match mode {
        AccessPoint => ACCESS_POINT_ROUTE_EXPIRY_MILLIS,
        Roaming => ROAMING_ROUTE_EXPIRY_MILLIS,
        Full | PointToPoint | Boundary | Gateway => DEFAULT_ROUTE_EXPIRY_MILLIS,
    }
}

pub const DEFAULT_REBROADCAST_JITTER_WINDOW_MS: u64 = 500;

pub const MAX_ANNOUNCE_REBROADCASTS: u8 = 2;

pub const REBROADCAST_RETRANSMIT_INTERVAL_MS: u64 = 5_500;

/// RNS 1.3.5 `Transport.PATH_REQUEST_GRACE` (0.4s): a transport node waits this
/// long before answering a path request from cache, so directly reachable peers
/// respond first; `..._RG` is the extra delay when the answering interface roams.
pub const PATH_REQUEST_GRACE_MS: u64 = 400;
pub const PATH_REQUEST_ROAMING_GRACE_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy)]
pub struct JitterSeed(pub u64);

#[allow(clippy::expect_used)]
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
    fn default_route_expiry_is_seven_days() {
        assert_eq!(DEFAULT_ROUTE_EXPIRY_MILLIS, 604_800_000);
    }

    #[test]
    fn mode_keyed_route_expiries_match_the_reference_lifetimes() {
        use crate::interfaces::InterfaceMode;
        assert_eq!(
            route_expiry_millis(InterfaceMode::AccessPoint),
            86_400_000,
            "AP_PATH_TIME: one day",
        );
        assert_eq!(
            route_expiry_millis(InterfaceMode::Roaming),
            21_600_000,
            "ROAMING_PATH_TIME: six hours",
        );
        for mode in [
            InterfaceMode::Full,
            InterfaceMode::PointToPoint,
            InterfaceMode::Boundary,
            InterfaceMode::Gateway,
        ] {
            assert_eq!(
                route_expiry_millis(mode),
                DEFAULT_ROUTE_EXPIRY_MILLIS,
                "PATHFINDER_E: the full week for {mode:?}",
            );
        }
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
    fn jitter_vector_pins_the_destination_word_mix() {
        let offset = jitter_offset_for(JitterSeed(0xCAFE_F00D), &dest(0x11), 500);
        assert_eq!(offset, 347);
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
