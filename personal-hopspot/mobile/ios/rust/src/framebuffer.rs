use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::Pixel;

pub const PANEL_WIDTH: usize = 64;
pub const PANEL_HEIGHT: usize = 128;
pub const PIXEL_COUNT: usize = PANEL_WIDTH * PANEL_HEIGHT;
pub const RGBA_BYTES: usize = PIXEL_COUNT * 4;

pub const LIT_RGBA: [u8; 4] = [0x4a, 0x9e, 0xff, 0xff];
pub const DARK_RGBA: [u8; 4] = [0x00, 0x06, 0x1a, 0xff];

pub struct FrameBuffer {
    lit: [bool; PIXEL_COUNT],
}

impl FrameBuffer {
    pub const fn new() -> Self {
        Self {
            lit: [false; PIXEL_COUNT],
        }
    }

    pub fn clear(&mut self) {
        self.lit = [false; PIXEL_COUNT];
    }

    pub fn expand_rgba(&self, out: &mut [u8]) {
        for (lit, chunk) in self.lit.iter().zip(out.chunks_exact_mut(4)) {
            chunk.copy_from_slice(if *lit { &LIT_RGBA } else { &DARK_RGBA });
        }
    }
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(PANEL_WIDTH as u32, PANEL_HEIGHT as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if (0..PANEL_WIDTH as i32).contains(&point.x)
                && (0..PANEL_HEIGHT as i32).contains(&point.y)
            {
                let index = point.y as usize * PANEL_WIDTH + point.x as usize;
                self.lit[index] = color.is_on();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    #[test]
    fn a_drawn_rectangle_lands_in_the_expanded_buffer() {
        let mut frame = FrameBuffer::new();
        Rectangle::new(Point::new(0, 0), Size::new(2, 2))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut frame)
            .unwrap();

        let mut out = vec![0u8; RGBA_BYTES];
        frame.expand_rgba(&mut out);

        let top_left = &out[0..4];
        assert_eq!(top_left, &LIT_RGBA);
        let below_the_square = &out[(2 * PANEL_WIDTH) * 4..(2 * PANEL_WIDTH) * 4 + 4];
        assert_eq!(below_the_square, &DARK_RGBA);
    }

    #[test]
    fn out_of_bounds_pixels_are_dropped_not_panicked() {
        let mut frame = FrameBuffer::new();
        frame
            .draw_iter([
                Pixel(Point::new(-1, -1), BinaryColor::On),
                Pixel(Point::new(PANEL_WIDTH as i32, 0), BinaryColor::On),
                Pixel(Point::new(0, PANEL_HEIGHT as i32), BinaryColor::On),
            ])
            .unwrap();

        let mut out = vec![0u8; RGBA_BYTES];
        frame.expand_rgba(&mut out);
        assert!(out.chunks_exact(4).all(|px| px == DARK_RGBA));
    }
}
