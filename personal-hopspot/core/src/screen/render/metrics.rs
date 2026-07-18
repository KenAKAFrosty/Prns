use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::FONT_5X8;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;

use super::layout::*;
use super::primitives::fill;

/// 3 significant figures, rolling unit B -> K -> M -> G (1000-based), max 3 numeric chars: `1.0K` up to `10K` up to `100K`, then `1.0M`, and so on. Integer-only (no float), max 4 chars including the unit.
pub(super) fn fmt_bytes(n: u64) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}B");
        return s;
    }
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('G', 1_000_000_000)
    };
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000;
    if int_part < 10 {
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

pub(in crate::screen) fn fmt_count(n: u32) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}");
        return s;
    }

    let n = n as u64;
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('B', 1_000_000_000)
    };
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000;
    if int_part < 10 {
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

pub(in crate::screen) fn fmt_rate_bytes_per_sec(n: u32) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}B");
        return s;
    }

    let n = n as u64;
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('G', 1_000_000_000)
    };
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000;
    if int_part < 10 {
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

pub(in crate::screen) fn fmt_activity_age(age_secs: Option<u32>) -> HString<8> {
    let mut s = HString::new();
    match age_secs {
        None => {
            let _ = write!(s, "-");
        }
        Some(0) => {
            let _ = write!(s, "now");
        }
        Some(seconds) if seconds < 60 => {
            let _ = write!(s, "{seconds}s");
        }
        Some(seconds) if seconds < 3600 => {
            let _ = write!(s, "{}m", seconds / 60);
        }
        Some(seconds) => {
            let hours = (seconds / 3600).min(99);
            let _ = write!(s, "{hours}h");
        }
    }
    s
}

#[cfg(test)]
pub(in crate::screen) fn compact_numeric_width(text: &str) -> i32 {
    text.chars()
        .map(|ch| {
            if ch == '.' {
                COMPACT_DECIMAL_WIDTH
            } else if ch == '/' {
                COMPACT_SLASH_WIDTH
            } else {
                NUMBER_GLYPH_WIDTH
            }
        })
        .sum()
}

pub(in crate::screen) fn draw_compact_number<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    text: &str,
    point: Point,
    color: BinaryColor,
) {
    let style = MonoTextStyle::new(&FONT_5X8, color);
    let mut x = point.x;
    for ch in text.chars() {
        if ch == '.' {
            let _ = Rectangle::new(Point::new(x, point.y + COMPACT_DECIMAL_Y), Size::new(1, 1))
                .into_styled(fill(color))
                .draw(display);
            x += COMPACT_DECIMAL_WIDTH;
            continue;
        }

        if ch == '/' {
            for (dx, dy) in [(2, 0), (1, 1), (0, 2)] {
                let _ = Rectangle::new(
                    Point::new(x + dx, point.y + COMPACT_SLASH_Y + dy),
                    Size::new(1, 1),
                )
                .into_styled(fill(color))
                .draw(display);
            }
            x += COMPACT_SLASH_WIDTH;
            continue;
        }

        let mut glyph: HString<2> = HString::new();
        let _ = glyph.push(ch);
        let _ =
            Text::with_baseline(&glyph, Point::new(x, point.y), style, Baseline::Top).draw(display);
        x += NUMBER_GLYPH_WIDTH;
    }
}
