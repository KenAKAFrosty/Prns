use core::convert::Infallible;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;

const PANEL_SIZE: i32 = 200;
const SCREEN_WIDTH: i32 = 64;
const SCREEN_HEIGHT: i32 = 128;
const SCALE_NUM: i32 = 3;
const SCALE_DEN: i32 = 2;
const SCALED_SHORT: i32 = SCREEN_WIDTH * SCALE_NUM / SCALE_DEN;
const SCALED_LONG: i32 = SCREEN_HEIGHT * SCALE_NUM / SCALE_DEN;
const SCALED_ORIGIN_X: i32 = (PANEL_SIZE - SCALED_LONG) / 2;
const SCALED_ORIGIN_Y: i32 = (PANEL_SIZE - SCALED_SHORT) / 2;

pub(crate) struct EinkScreen<'a> {
    pub(crate) panel: &'a mut Display1in54,
}

impl OriginDimensions for EinkScreen<'_> {
    fn size(&self) -> Size {
        Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)
    }
}

impl DrawTarget for EinkScreen<'_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let panel_color = match color {
                BinaryColor::On => EpdColor::Black,
                BinaryColor::Off => EpdColor::White,
            };
            let sx0 = point.x * SCALE_NUM / SCALE_DEN;
            let sx1 = (point.x + 1) * SCALE_NUM / SCALE_DEN;
            let sy0 = point.y * SCALE_NUM / SCALE_DEN;
            let sy1 = (point.y + 1) * SCALE_NUM / SCALE_DEN;
            let top_left = Point::new(
                SCALED_ORIGIN_X + sy0,
                SCALED_ORIGIN_Y + (SCALED_SHORT - sx1),
            );
            let size = Size::new((sy1 - sy0) as u32, (sx1 - sx0) as u32);
            let _ = self
                .panel
                .fill_solid(&Rectangle::new(top_left, size), panel_color);
        }
        Ok(())
    }
}

pub(crate) fn frame_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
