use core::fmt;

/// UTC seconds supplied by the platform clock contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seconds(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockInterval {
    lower: Timestamp,
    upper: Timestamp,
}

impl ClockInterval {
    pub const fn new(lower: Timestamp, upper: Timestamp) -> Result<Self, TimeError> {
        if lower.0 > upper.0 {
            return Err(TimeError::InvalidClockInterval);
        }
        Ok(Self { lower, upper })
    }

    pub const fn lower(self) -> Timestamp {
        self.lower
    }

    pub const fn upper(self) -> Timestamp {
        self.upper
    }
}

/// A trusted clock interval is a platform assertion, never a peer's claimed time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupClockReading"))]
pub enum ClockReading {
    Unknown,
    Trusted(ClockInterval),
}

/// A fixed delivery window. Retrying cannot create or extend this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationValidity {
    created_at: Timestamp,
    deadline: Timestamp,
}

impl PublicationValidity {
    pub const fn new(
        created_at: Timestamp,
        deadline: Timestamp,
        maximum_window: Seconds,
    ) -> Result<Self, TimeError> {
        if maximum_window.0 == 0 {
            return Err(TimeError::ZeroMaximumWindow);
        }
        if deadline.0 <= created_at.0 {
            return Err(TimeError::InvalidPublicationWindow);
        }
        if deadline.0 - created_at.0 > maximum_window.0 {
            return Err(TimeError::WindowTooLong);
        }
        Ok(Self {
            created_at,
            deadline,
        })
    }

    pub const fn created_at(self) -> Timestamp {
        self.created_at
    }

    pub const fn deadline(self) -> Timestamp {
        self.deadline
    }

    pub const fn admission(self, clock: ClockReading, allowed_future_skew: Seconds) -> Admission {
        let ClockReading::Trusted(interval) = clock else {
            return Admission::TimeUncertain;
        };
        if interval.lower.0 >= self.deadline.0 {
            return Admission::Expired;
        }
        if interval.upper.0 >= self.deadline.0
            || self.created_at.0 > interval.upper.0.saturating_add(allowed_future_skew.0)
        {
            return Admission::TimeUncertain;
        }
        Admission::Accept
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Accept,
    Expired,
    TimeUncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeError {
    InvalidClockInterval,
    ZeroMaximumWindow,
    InvalidPublicationWindow,
    WindowTooLong,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid publication time: {self:?}")
    }
}

impl core::error::Error for TimeError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn clock(lower: u64, upper: u64) -> ClockReading {
        ClockReading::Trusted(ClockInterval::new(Timestamp(lower), Timestamp(upper)).unwrap())
    }

    #[test]
    fn overlapping_or_unknown_time_is_neither_admission_nor_expiry() {
        let validity =
            PublicationValidity::new(Timestamp(100), Timestamp(200), Seconds(100)).unwrap();
        for (reading, expected) in [
            (clock(100, 100), Admission::Accept),
            (clock(199, 199), Admission::Accept),
            (clock(199, 200), Admission::TimeUncertain),
            (clock(100, 201), Admission::TimeUncertain),
            (clock(200, 200), Admission::Expired),
            (clock(201, 210), Admission::Expired),
            (ClockReading::Unknown, Admission::TimeUncertain),
        ] {
            assert_eq!(validity.admission(reading, Seconds(0)), expected);
        }
        assert_eq!(validity.deadline(), Timestamp(200));
    }

    #[test]
    fn configured_window_and_future_skew_are_explicit_and_checked() {
        assert_eq!(
            PublicationValidity::new(Timestamp(100), Timestamp(200), Seconds(0)),
            Err(TimeError::ZeroMaximumWindow)
        );
        for deadline in [Timestamp(99), Timestamp(100)] {
            assert_eq!(
                PublicationValidity::new(Timestamp(100), deadline, Seconds(100)),
                Err(TimeError::InvalidPublicationWindow)
            );
        }
        assert_eq!(
            PublicationValidity::new(Timestamp(100), Timestamp(201), Seconds(100)),
            Err(TimeError::WindowTooLong)
        );
        assert_eq!(
            ClockInterval::new(Timestamp(2), Timestamp(1)),
            Err(TimeError::InvalidClockInterval)
        );
        let validity =
            PublicationValidity::new(Timestamp(100), Timestamp(200), Seconds(100)).unwrap();
        assert_eq!(
            validity.admission(clock(89, 90), Seconds(9)),
            Admission::TimeUncertain
        );
        assert_eq!(
            validity.admission(clock(89, 90), Seconds(10)),
            Admission::Accept
        );
    }

    #[test]
    fn timestamps_near_u64_limit_do_not_wrap() {
        let validity =
            PublicationValidity::new(Timestamp(u64::MAX - 1), Timestamp(u64::MAX), Seconds(1))
                .unwrap();
        assert_eq!(
            validity.admission(clock(u64::MAX - 2, u64::MAX - 2), Seconds(4)),
            Admission::Accept
        );
        assert_eq!(
            validity.admission(clock(u64::MAX - 1, u64::MAX), Seconds(4)),
            Admission::TimeUncertain
        );
        assert_eq!(
            validity.admission(clock(u64::MAX, u64::MAX), Seconds(4)),
            Admission::Expired
        );
    }
}
