use core::convert::Infallible;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Pixel, Point, Size};

pub const LOGICAL_WIDTH: u32 = 64;
pub const LOGICAL_HEIGHT: u32 = 128;
pub const FRAME_BYTES: usize = (LOGICAL_WIDTH * LOGICAL_HEIGHT / 8) as usize;

/// Fixed canonical storage for the current monochrome Personal Hopspot face.
#[derive(Clone, Eq, PartialEq)]
pub struct Frame {
    bytes: [u8; FRAME_BYTES],
}

impl Frame {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bytes: [0; FRAME_BYTES],
        }
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8; FRAME_BYTES] {
        &self.bytes
    }

    #[must_use]
    pub fn pixel_is_on(&self, point: Point) -> bool {
        if point.x < 0
            || point.y < 0
            || point.x >= LOGICAL_WIDTH as i32
            || point.y >= LOGICAL_HEIGHT as i32
        {
            return false;
        }
        let bit_index = point.y as u32 * LOGICAL_WIDTH + point.x as u32;
        self.bytes[(bit_index / 8) as usize] & (0x80 >> (bit_index % 8)) != 0
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(LOGICAL_WIDTH, LOGICAL_HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(Point { x, y }, color) in pixels {
            if x < 0 || y < 0 || x >= LOGICAL_WIDTH as i32 || y >= LOGICAL_HEIGHT as i32 {
                continue;
            }
            let bit_index = y as u32 * LOGICAL_WIDTH + x as u32;
            let byte = &mut self.bytes[(bit_index / 8) as usize];
            let mask = 0x80 >> (bit_index % 8);
            match color {
                BinaryColor::On => *byte |= mask,
                BinaryColor::Off => *byte &= !mask,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_storage_and_bit_order_are_canonical() {
        let mut frame = Frame::new();
        frame
            .draw_iter([
                Pixel(Point::new(0, 0), BinaryColor::On),
                Pixel(Point::new(7, 0), BinaryColor::On),
                Pixel(Point::new(8, 0), BinaryColor::On),
                Pixel(Point::new(63, 127), BinaryColor::On),
            ])
            .unwrap();

        assert_eq!(frame.bytes()[0], 0x81);
        assert_eq!(frame.bytes()[1], 0x80);
        assert_eq!(frame.bytes()[FRAME_BYTES - 1], 0x01);
        assert_eq!(frame.size(), Size::new(64, 128));
    }

    #[test]
    fn drawing_clips_outside_the_logical_face() {
        let mut frame = Frame::new();
        frame
            .draw_iter([
                Pixel(Point::new(-1, 0), BinaryColor::On),
                Pixel(Point::new(64, 0), BinaryColor::On),
                Pixel(Point::new(0, 128), BinaryColor::On),
            ])
            .unwrap();
        assert_eq!(frame.bytes(), &[0; FRAME_BYTES]);
    }
}
