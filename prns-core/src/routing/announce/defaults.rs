use crate::interfaces::InterfaceMode;

pub const DEFAULT_REBROADCAST_JITTER_WINDOW_MS: u64 = 500;
pub const MAX_OUR_EMISSIONS: u8 = 2;
pub const MAX_PEER_EMISSIONS: u8 = 2;
pub(crate) const ANNOUNCE_WITH_RETRY_INITIAL_EMISSION_COUNT: u8 = 0;
pub(crate) const ANNOUNCE_ONE_SHOT_INITIAL_EMISSION_COUNT: u8 = MAX_OUR_EMISSIONS - 1;
pub const REBROADCAST_RETRY_GRACE_MS: u64 = 5_000;
pub const REBROADCAST_RETRANSMIT_INTERVAL_MS: u64 =
    REBROADCAST_RETRY_GRACE_MS + DEFAULT_REBROADCAST_JITTER_WINDOW_MS;

pub const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = 64;

pub const DEFAULT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;
pub const ACCESS_POINT_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 1000;
pub const ROAMING_ROUTE_EXPIRY_MILLIS: u64 = 60 * 60 * 6 * 1000;

pub fn route_expiry_millis(mode: InterfaceMode) -> u64 {
    use InterfaceMode::{AccessPoint, Boundary, Full, Gateway, Internal, PointToPoint, Roaming};
    match mode {
        AccessPoint => ACCESS_POINT_ROUTE_EXPIRY_MILLIS,
        Roaming => ROAMING_ROUTE_EXPIRY_MILLIS,
        Full | PointToPoint | Boundary | Gateway | Internal => DEFAULT_ROUTE_EXPIRY_MILLIS,
    }
}

/// RNS 1.3.5 `Transport.PATH_REQUEST_GRACE` (0.4s): a transport node waits this long before answering a path request from cache, so directly reachable peers respond first.
///
/// `..._RG` is the extra delay when the answering interface roams.
pub const PATH_REQUEST_GRACE_MS: u64 = 400;
pub const PATH_REQUEST_ROAMING_GRACE_MS: u64 = 1_500;

pub(crate) fn jitter_offset(fill_entropy: &mut impl FnMut(&mut [u8]), window_ms: u64) -> u64 {
    if window_ms == 0 {
        return 0;
    }
    let mut bytes = [0u8; core::mem::size_of::<u64>()];
    fill_entropy(&mut bytes);
    u64::from_le_bytes(bytes) % window_ms
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
    fn the_emission_caps_match_the_reference() {
        assert_eq!(
            MAX_OUR_EMISSIONS, 2,
            "one emit plus PATHFINDER_R(1) retransmit"
        );
        assert_eq!(ANNOUNCE_WITH_RETRY_INITIAL_EMISSION_COUNT, 0);
        assert_eq!(ANNOUNCE_ONE_SHOT_INITIAL_EMISSION_COUNT, 1);
        assert_eq!(MAX_PEER_EMISSIONS, 2, "LOCAL_REBROADCASTS_MAX");
    }

    #[test]
    fn the_per_destination_announce_id_cap_matches_the_reference() {
        assert_eq!(
            MAX_ANNOUNCE_IDS_PER_DESTINATION, 64,
            "Transport.MAX_RANDOM_BLOBS",
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
            InterfaceMode::Internal,
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
        let mut draw = |out: &mut [u8]| out.copy_from_slice(&1_234u64.to_le_bytes());
        assert_eq!(jitter_offset(&mut draw, 500), 1_234 % 500);
        let mut wide = |out: &mut [u8]| out.copy_from_slice(&0xDEAD_BEEFu64.to_le_bytes());
        assert!(
            jitter_offset(&mut wide, 500) < 500,
            "the offset always lands inside the window",
        );
    }

    #[test]
    fn zero_window_draws_nothing_and_yields_zero_offset() {
        let mut unreached = |_: &mut [u8]| panic!("a zero window must not draw entropy");
        assert_eq!(jitter_offset(&mut unreached, 0), 0);
    }
}
