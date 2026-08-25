const DEGREES_E7: i32 = 10_000_000;
const MAX_LATITUDE_E7: i32 = 90 * DEGREES_E7;
const MAX_LONGITUDE_E7: i32 = 180 * DEGREES_E7;

/// Why signed degree-times-10^7 input could not form a geographic coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinateOutOfRange {
    Latitude(i32),
    Longitude(i32),
}

/// A valid WGS84 latitude represented as signed degrees times 10^7.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct LatitudeE7(i32);

impl LatitudeE7 {
    pub const MIN: Self = Self(-MAX_LATITUDE_E7);
    pub const MAX: Self = Self(MAX_LATITUDE_E7);

    /// Validate signed degree-times-10^7 input.
    pub const fn new(value: i32) -> Result<Self, CoordinateOutOfRange> {
        if value < Self::MIN.0 || value > Self::MAX.0 {
            Err(CoordinateOutOfRange::Latitude(value))
        } else {
            Ok(Self(value))
        }
    }

    /// Return signed degrees times 10^7.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// A valid WGS84 longitude represented as signed degrees times 10^7.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct LongitudeE7(i32);

impl LongitudeE7 {
    pub const MIN: Self = Self(-MAX_LONGITUDE_E7);
    pub const MAX: Self = Self(MAX_LONGITUDE_E7);

    /// Validate signed degree-times-10^7 input.
    pub const fn new(value: i32) -> Result<Self, CoordinateOutOfRange> {
        if value < Self::MIN.0 || value > Self::MAX.0 {
            Err(CoordinateOutOfRange::Longitude(value))
        } else {
            Ok(Self(value))
        }
    }

    /// Return signed degrees times 10^7.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// Height above the provider's reported reference surface, in millimetres.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct AltitudeMillimeters(i32);

impl AltitudeMillimeters {
    /// Represent a provider-reported altitude in millimetres.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// Return the altitude in millimetres.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// One source-neutral geographic position suitable for embedded, mobile, or desktop hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeographicPosition {
    latitude: LatitudeE7,
    longitude: LongitudeE7,
    altitude: Option<AltitudeMillimeters>,
}

impl GeographicPosition {
    /// Construct a position from already validated axis values.
    #[must_use]
    pub const fn new(
        latitude: LatitudeE7,
        longitude: LongitudeE7,
        altitude: Option<AltitudeMillimeters>,
    ) -> Self {
        Self {
            latitude,
            longitude,
            altitude,
        }
    }

    /// Validate raw fixed-point axes and construct a position.
    pub const fn try_from_e7(
        latitude_e7: i32,
        longitude_e7: i32,
        altitude_mm: Option<i32>,
    ) -> Result<Self, CoordinateOutOfRange> {
        let latitude = match LatitudeE7::new(latitude_e7) {
            Ok(latitude) => latitude,
            Err(error) => return Err(error),
        };
        let longitude = match LongitudeE7::new(longitude_e7) {
            Ok(longitude) => longitude,
            Err(error) => return Err(error),
        };
        Ok(Self::new(
            latitude,
            longitude,
            match altitude_mm {
                Some(altitude) => Some(AltitudeMillimeters::new(altitude)),
                None => None,
            },
        ))
    }

    /// Return the validated latitude.
    #[must_use]
    pub const fn latitude(self) -> LatitudeE7 {
        self.latitude
    }

    /// Return the validated longitude.
    #[must_use]
    pub const fn longitude(self) -> LongitudeE7 {
        self.longitude
    }

    /// Return provider-reported altitude when it was available.
    #[must_use]
    pub const fn altitude(self) -> Option<AltitudeMillimeters> {
        self.altitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geographic_position_rejects_only_out_of_range_axes() {
        assert!(GeographicPosition::try_from_e7(-900_000_000, 1_800_000_000, None).is_ok());
        assert_eq!(
            GeographicPosition::try_from_e7(900_000_001, 0, None),
            Err(CoordinateOutOfRange::Latitude(900_000_001))
        );
        assert_eq!(
            GeographicPosition::try_from_e7(0, -1_800_000_001, None),
            Err(CoordinateOutOfRange::Longitude(-1_800_000_001))
        );
    }
}
