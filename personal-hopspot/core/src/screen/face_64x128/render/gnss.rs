use core::fmt::Write as _;

use embedded_graphics::mono_font::iso_8859_1::FONT_4X6;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;

use crate::GnssSnapshot;

use super::layout::{GNSS_PANEL_TOP, WIDTH};
use super::primitives::line;

pub(super) fn draw_gnss_panel<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    snapshot: GnssSnapshot,
) {
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let mut first = HString::<16>::new();
    match snapshot {
        GnssSnapshot::Disabled => {
            let _ = first.push_str("GPS OFF");
        }
        GnssSnapshot::Starting => {
            let _ = first.push_str("GPS STARTING");
        }
        GnssSnapshot::Searching { satellites } => {
            let _ = write!(first, "GPS SEARCH S{satellites:02}");
        }
        GnssSnapshot::Fixed(fix) => {
            let _ = write!(first, "FIX S{:02}", fix.satellites());
            if let Some(altitude) = fix.position().altitude() {
                let _ = write!(first, " A{}m", altitude.get() / 1_000);
            }
        }
        GnssSnapshot::Error => {
            let _ = first.push_str("GPS ERROR");
        }
    }
    let _ = Text::with_baseline(
        first.as_str(),
        Point::new(2, GNSS_PANEL_TOP + 1),
        style,
        Baseline::Top,
    )
    .draw(display);

    if let Some(fix) = snapshot.fix() {
        let position = fix.position();
        let latitude = coordinate_text(position.latitude().get(), CoordinateAxis::Latitude);
        let longitude = coordinate_text(position.longitude().get(), CoordinateAxis::Longitude);
        let _ = Text::with_baseline(
            latitude.as_str(),
            Point::new(2, GNSS_PANEL_TOP + 8),
            style,
            Baseline::Top,
        )
        .draw(display);
        let _ = Text::with_baseline(
            longitude.as_str(),
            Point::new(2, GNSS_PANEL_TOP + 15),
            style,
            Baseline::Top,
        )
        .draw(display);
    } else if matches!(snapshot, GnssSnapshot::Searching { .. }) {
        let _ = Text::with_baseline(
            "Waiting for fix",
            Point::new(2, GNSS_PANEL_TOP + 9),
            style,
            Baseline::Top,
        )
        .draw(display);
    }
    line(
        display,
        Point::new(0, GNSS_PANEL_TOP + 22),
        Point::new(WIDTH - 1, GNSS_PANEL_TOP + 22),
    );
}

#[derive(Clone, Copy)]
enum CoordinateAxis {
    Latitude,
    Longitude,
}

fn coordinate_text(value_e7: i32, axis: CoordinateAxis) -> HString<16> {
    let mut text = HString::new();
    let direction = match (axis, value_e7.is_negative()) {
        (CoordinateAxis::Latitude, false) => 'N',
        (CoordinateAxis::Latitude, true) => 'S',
        (CoordinateAxis::Longitude, false) => 'E',
        (CoordinateAxis::Longitude, true) => 'W',
    };
    let magnitude = value_e7.unsigned_abs();
    let degrees = magnitude / 10_000_000;
    let fraction = magnitude % 10_000_000;
    let _ = match axis {
        CoordinateAxis::Latitude => write!(text, "{direction} {degrees:02}.{fraction:07}"),
        CoordinateAxis::Longitude => write!(text, "{direction} {degrees:03}.{fraction:07}"),
    };
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_text_is_fixed_width_and_hemisphere_explicit() {
        assert_eq!(
            coordinate_text(481_173_000, CoordinateAxis::Latitude).as_str(),
            "N 48.1173000"
        );
        assert_eq!(
            coordinate_text(-115_166_666, CoordinateAxis::Longitude).as_str(),
            "W 011.5166666"
        );
    }
}
