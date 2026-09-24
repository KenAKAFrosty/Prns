//! When a headless node announces its node page.
//!
//! Display boards announce from a menu action and the MeshTower from its button; a board with
//! neither would otherwise never announce and stay invisible to every peer even while it relays
//! their traffic. The schedule is one announce shortly after boot, once the interfaces have come
//! up, then one every six hours: prnsd's NomadNet page default and the interval stock NomadNet
//! nodes use.

/// How long after boot the first announce goes out, so it is not lost while the radio and USB
/// links are still coming up.
pub const HEADLESS_ANNOUNCE_BOOT_DELAY_MS: u64 = 15_000;
/// The spacing of every later announce.
pub const HEADLESS_ANNOUNCE_INTERVAL_MS: u64 = 6 * 60 * 60 * 1_000;

/// The wait before the next announce, given how many the node has already sent since boot.
pub const fn headless_announce_delay_ms(announces_sent: u32) -> u64 {
    if announces_sent == 0 {
        HEADLESS_ANNOUNCE_BOOT_DELAY_MS
    } else {
        HEADLESS_ANNOUNCE_INTERVAL_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_announce_waits_for_the_interfaces_then_repeats_every_six_hours() {
        assert_eq!(headless_announce_delay_ms(0), 15_000);
        assert_eq!(headless_announce_delay_ms(1), 6 * 60 * 60 * 1_000);
        assert_eq!(headless_announce_delay_ms(1_000), 6 * 60 * 60 * 1_000);
    }
}
