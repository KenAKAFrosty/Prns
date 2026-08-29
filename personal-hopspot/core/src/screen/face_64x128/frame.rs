use core::convert::Infallible;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Pixel, Point, Size};

pub const WIDTH: u32 = 64;
pub const HEIGHT: u32 = 128;
pub const FRAME_BYTES: usize = (WIDTH * HEIGHT / 8) as usize;

#[derive(Clone, Eq, PartialEq)]
#[repr(transparent)]
pub struct Frame {
    bytes: [u8; FRAME_BYTES],
}

struct PackedPixel {
    byte_index: usize,
    mask: u8,
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
        let Some(pixel) = packed_pixel(point) else {
            return false;
        };
        self.bytes[pixel.byte_index] & pixel.mask != 0
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let Some(pixel) = packed_pixel(point) else {
                continue;
            };
            let byte = &mut self.bytes[pixel.byte_index];
            match color {
                BinaryColor::On => *byte |= pixel.mask,
                BinaryColor::Off => *byte &= !pixel.mask,
            }
        }
        Ok(())
    }
}

fn packed_pixel(point: Point) -> Option<PackedPixel> {
    if point.x < 0 || point.y < 0 || point.x >= WIDTH as i32 || point.y >= HEIGHT as i32 {
        return None;
    }
    let bit_index = point.y as u32 * WIDTH + point.x as u32;
    Some(PackedPixel {
        byte_index: (bit_index / 8) as usize,
        mask: 0x80 >> (bit_index % 8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_and_bit_order_are_canonical() {
        assert_eq!(core::mem::size_of::<Frame>(), 1_024);
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
        assert_eq!(frame.size(), Size::new(WIDTH, HEIGHT));
    }

    #[test]
    fn drawing_clips_outside_the_face() {
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
