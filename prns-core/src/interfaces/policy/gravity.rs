use crate::interfaces::BitrateBps;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct InterfaceGravity(i64);

impl InterfaceGravity {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Converts bitrate into the same numeric preference, saturating only values beyond the
    /// signed gravity range.
    #[must_use]
    pub const fn from_bitrate(bitrate: BitrateBps) -> Self {
        let bps = bitrate.get();
        if bps > i64::MAX as u64 {
            Self(i64::MAX)
        } else {
            Self(bps as i64)
        }
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// The medium-owned gravity policy used when configuration supplies no explicit override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceGravityDefault {
    FromBitrate,
    Fixed(InterfaceGravity),
}

impl InterfaceGravityDefault {
    #[must_use]
    pub const fn resolve(self, bitrate: BitrateBps) -> InterfaceGravity {
        match self {
            Self::FromBitrate => InterfaceGravity::from_bitrate(bitrate),
            Self::Fixed(gravity) => gravity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_is_the_automatic_gravity() {
        let bitrate = BitrateBps::guess(5_000_000);
        assert_eq!(
            InterfaceGravityDefault::FromBitrate.resolve(bitrate),
            InterfaceGravity::new(5_000_000)
        );
    }

    #[test]
    fn automatic_gravity_saturates_unrepresentable_bitrates() {
        let bitrate = BitrateBps::guess(i64::MAX as u64 + 1);
        assert_eq!(
            InterfaceGravity::from_bitrate(bitrate),
            InterfaceGravity::new(i64::MAX)
        );
    }

    #[test]
    fn a_fixed_default_remains_independent_of_bitrate() {
        assert_eq!(
            InterfaceGravityDefault::Fixed(InterfaceGravity::new(-7))
                .resolve(BitrateBps::guess(1_000_000_000)),
            InterfaceGravity::new(-7)
        );
    }
}
