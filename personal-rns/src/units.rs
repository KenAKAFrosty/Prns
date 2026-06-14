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

/// A link's measured round-trip time, in milliseconds. Always a real
/// measurement: a link only carries an RTT once it is active, and the unmeasured
/// state is its pre-active phase, which has no RTT field at all — so there is no
/// "unknown" sentinel to guard against and a zero is a genuine sub-millisecond
/// round trip. Where a measurement is genuinely pending (a resource that has not
/// yet timed a round trip), the absence is an honest `Option<Rtt>`, never a zero.
/// Deliberately not `Default`: an `Rtt` comes from a measurement or the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Rtt(pub u64);

impl Rtt {
    pub const fn from_millis(millis: u64) -> Rtt {
        Rtt(millis)
    }

    /// The round trip from `sent` to `arrived` — the way the link and resource
    /// layers time an ack against the moment its packet went out.
    pub const fn measured_between(sent: InstantMillis, arrived: InstantMillis) -> Rtt {
        Rtt(arrived.0.saturating_sub(sent.0))
    }

    pub const fn millis(self) -> u64 {
        self.0
    }
}

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
