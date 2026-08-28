use core::num::{NonZeroU32, NonZeroU64};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicMillis(u64);

impl MonotonicMillis {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    pub(super) const fn saturating_add(self, duration: DisplayDuration) -> Self {
        Self(self.0.saturating_add(duration.as_millis()))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ZeroDuration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayDuration(NonZeroU64);

impl DisplayDuration {
    pub const fn from_millis(milliseconds: u64) -> Result<Self, ZeroDuration> {
        match NonZeroU64::new(milliseconds) {
            Some(value) => Ok(Self(value)),
            None => Err(ZeroDuration),
        }
    }

    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ZeroPartialRefreshLimit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialRefreshLimit(NonZeroU32);

impl PartialRefreshLimit {
    pub const fn new(limit: u32) -> Result<Self, ZeroPartialRefreshLimit> {
        match NonZeroU32::new(limit) {
            Some(value) => Ok(Self(value)),
            None => Err(ZeroPartialRefreshLimit),
        }
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}
