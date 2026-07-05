#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitrateBps(u32);

impl BitrateBps {
    pub const MINIMUM: u32 = 5;

    #[must_use]
    pub const fn new(bps: u32) -> Option<Self> {
        if bps >= Self::MINIMUM {
            Some(Self(bps))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn guess(bps: u32) -> Self {
        assert!(
            bps >= Self::MINIMUM,
            "an interface bitrate guess must meet the RNS minimum bitrate",
        );
        Self(bps)
    }

    #[must_use]
    pub const fn clamped(bps: u32) -> Self {
        Self(if bps >= Self::MINIMUM {
            bps
        } else {
            Self::MINIMUM
        })
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_below_the_floor_and_accepts_from_it_up() {
        assert_eq!(BitrateBps::new(4), None);
        assert_eq!(BitrateBps::new(5).map(BitrateBps::get), Some(5));
        assert_eq!(BitrateBps::new(0), None);
        assert_eq!(
            BitrateBps::new(1_000_000).map(BitrateBps::get),
            Some(1_000_000)
        );
    }

    #[test]
    fn guess_holds_a_compile_time_default() {
        const GIGABIT: BitrateBps = BitrateBps::guess(1_000_000_000);
        assert_eq!(GIGABIT.get(), 1_000_000_000);
    }

    #[test]
    #[should_panic(expected = "RNS minimum bitrate")]
    fn guess_below_the_floor_panics() {
        let _ = BitrateBps::guess(4);
    }

    #[test]
    fn clamped_floors_a_trusted_rate_without_rejecting_it() {
        assert_eq!(BitrateBps::clamped(3).get(), BitrateBps::MINIMUM);
        assert_eq!(BitrateBps::clamped(293).get(), 293);
    }
}
