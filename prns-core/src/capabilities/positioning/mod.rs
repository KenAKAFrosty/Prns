//! Source-neutral geographic positions and optional positioning providers.

mod geographic;
pub mod gnss;

pub use geographic::{
    AltitudeMillimeters, CoordinateOutOfRange, GeographicPosition, LatitudeE7, LongitudeE7,
};
