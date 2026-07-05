use crate::interfaces::InterfaceMode;

pub const DEFAULT_REBROADCAST_JITTER_WINDOW_MS: u64 = 500;
pub const MAX_ANNOUNCE_REBROADCASTS: u8 = 2;
pub const REBROADCAST_RETRY_GRACE_MS: u64 = 5_000;
pub const REBROADCAST_RETRANSMIT_INTERVAL_MS: u64 =
    REBROADCAST_RETRY_GRACE_MS + DEFAULT_REBROADCAST_JITTER_WINDOW_MS;

pub const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;
pub const ACCESS_POINT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 1000;
pub const ROAMING_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 6 * 1000;

pub fn route_expiry_millis(mode: InterfaceMode) -> u64 {
    use InterfaceMode::{AccessPoint, Boundary, Full, Gateway, PointToPoint, Roaming};
    match mode {
        AccessPoint => ACCESS_POINT_ROUTE_EXPIRY_MILLIS,
        Roaming => ROAMING_ROUTE_EXPIRY_MILLIS,
        Full | PointToPoint | Boundary | Gateway => DEFAULT_ROUTE_EXPIRY_MILLIS,
    }
}

/// RNS 1.3.5 `Transport.PATH_REQUEST_GRACE` (0.4s): a transport node waits this long before answering a path request from cache, so directly reachable peers respond first.
///
/// `..._RG` is the extra delay when the answering interface roams.
pub const PATH_REQUEST_GRACE_MS: u64 = 400;
pub const PATH_REQUEST_ROAMING_GRACE_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy)]
pub struct JitterSeed(pub u64);

pub(crate) fn jitter_offset(seed: JitterSeed, window_ms: u64) -> u64 {
    if window_ms == 0 {
        0
    } else {
        seed.0 % window_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_route_expiry_is_seven_days() {
        assert_eq!(DEFAULT_ROUTE_EXPIRY_MILLIS, 604_800_000);
    }

    #[test]
    fn the_retransmit_interval_is_the_grace_plus_the_jitter_window() {
        assert_eq!(REBROADCAST_RETRY_GRACE_MS, 5_000, "PATHFINDER_G");
        assert_eq!(DEFAULT_REBROADCAST_JITTER_WINDOW_MS, 500, "PATHFINDER_RW");
        assert_eq!(
            REBROADCAST_RETRANSMIT_INTERVAL_MS, 5_500,
            "PATHFINDER_G + PATHFINDER_RW",
        );
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
    fn jitter_offset_is_the_host_draw_within_the_window() {
        assert_eq!(jitter_offset(JitterSeed(1_234), 500), 1_234 % 500);
        assert!(
            jitter_offset(JitterSeed(0xDEAD_BEEF), 500) < 500,
            "the offset always lands inside the window",
        );
    }

    #[test]
    fn distinct_draws_land_on_distinct_offsets() {
        assert_ne!(
            jitter_offset(JitterSeed(0), 500),
            jitter_offset(JitterSeed(u64::MAX), 500),
            "a different host draw should not collapse to the same offset",
        );
    }

    #[test]
    fn zero_window_yields_zero_offset() {
        assert_eq!(jitter_offset(JitterSeed(0xCAFE), 0), 0);
    }
}
