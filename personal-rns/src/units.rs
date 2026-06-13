#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct InstantMillis(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct ByteCount(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct BitsPerSecond(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct HopCount(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct DurationMillis(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct LinkCount(pub usize);

impl InstantMillis {
    pub const fn saturating_add(self, elapsed: DurationMillis) -> InstantMillis {
        InstantMillis(self.0.saturating_add(elapsed.0))
    }

    pub const fn duration_since(self, earlier: InstantMillis) -> DurationMillis {
        DurationMillis(self.0.saturating_sub(earlier.0))
    }
}

impl DurationMillis {
    pub const fn saturating_add(self, rhs: DurationMillis) -> DurationMillis {
        DurationMillis(self.0.saturating_add(rhs.0))
    }
}

impl ByteCount {
    pub const fn saturating_add(self, rhs: ByteCount) -> ByteCount {
        ByteCount(self.0.saturating_add(rhs.0))
    }
}

impl core::iter::Sum for ByteCount {
    fn sum<I: Iterator<Item = ByteCount>>(iter: I) -> ByteCount {
        iter.fold(ByteCount(0), ByteCount::saturating_add)
    }
}
