use embedded_graphics::prelude::Point;
use personal_hopspot_core::face_64x128::{
    Frame, MappedPoint, PanelScale, PanelSize, PanelTransform, PhysicalPoint, QuarterTurn,
};

const PANEL_WIDTH: u32 = 200;
const PANEL_HEIGHT: u32 = 200;
pub(super) const ROW_BYTES: usize = PANEL_WIDTH as usize / 8;

pub(super) fn transform() -> PanelTransform {
    PanelTransform::centered(
        PanelSize::new(PANEL_WIDTH, PANEL_HEIGHT).expect("the T-Echo panel is nonzero"),
        PanelScale::ThreeToTwo,
        QuarterTurn::CounterClockwise,
    )
    .expect("the canonical face fits the T-Echo panel")
}

pub(super) fn rasterize_row(frame: &Frame, transform: &PanelTransform, y: u32) -> [u8; ROW_BYTES] {
    let mut row = [0xff; ROW_BYTES];
    for x in 0..PANEL_WIDTH {
        let Ok(MappedPoint::Source(source)) = transform.map_panel_point(PhysicalPoint::new(x, y))
        else {
            continue;
        };
        if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32)) {
            row[x as usize / 8] &= !(0x80 >> (x % 8));
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Pixel, Size};
    use embedded_graphics::primitives::Rectangle;
    use epd_waveshare::color::Color as EpdColor;
    use epd_waveshare::epd1in54_v2::Display1in54;

    use super::*;

    struct QualifiedRaster<'a> {
        panel: &'a mut Display1in54,
    }

    impl OriginDimensions for QualifiedRaster<'_> {
        fn size(&self) -> Size {
            Size::new(64, 128)
        }
    }

    impl DrawTarget for QualifiedRaster<'_> {
        type Color = BinaryColor;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                let sx0 = point.x * 3 / 2;
                let sx1 = (point.x + 1) * 3 / 2;
                let sy0 = point.y * 3 / 2;
                let sy1 = (point.y + 1) * 3 / 2;
                let top_left = Point::new(4 + sy0, 52 + (96 - sx1));
                let size = Size::new((sy1 - sy0) as u32, (sx1 - sx0) as u32);
                let panel_color = match color {
                    BinaryColor::On => EpdColor::Black,
                    BinaryColor::Off => EpdColor::White,
                };
                let _ = self
                    .panel
                    .fill_solid(&Rectangle::new(top_left, size), panel_color);
            }
            Ok(())
        }
    }

    fn pattern() -> impl Iterator<Item = Pixel<BinaryColor>> {
        (0..128).flat_map(|y| {
            (0..64).map(move |x| {
                let on = (x + y * 3) % 11 < 4 || x == 0 || x == 63 || y == 0 || y == 127;
                Pixel(
                    Point::new(x, y),
                    if on {
                        BinaryColor::On
                    } else {
                        BinaryColor::Off
                    },
                )
            })
        })
    }

    #[test]
    fn shipping_raster_points_are_inside_the_panel() {
        let transform = transform();
        for y in 0..PANEL_HEIGHT {
            for x in 0..PANEL_WIDTH {
                assert!(transform.map_panel_point(PhysicalPoint::new(x, y)).is_ok());
            }
        }
    }

    #[test]
    fn canonical_transform_matches_the_qualified_t_echo_raster() {
        let mut frame = Frame::new();
        frame.draw_iter(pattern()).unwrap();

        let transform = transform();
        let mut canonical = [0_u8; ROW_BYTES * PANEL_HEIGHT as usize];
        for y in 0..PANEL_HEIGHT {
            let start = y as usize * ROW_BYTES;
            canonical[start..start + ROW_BYTES]
                .copy_from_slice(&rasterize_row(&frame, &transform, y));
        }

        let mut qualified = Display1in54::default();
        qualified.clear(EpdColor::White).unwrap();
        QualifiedRaster {
            panel: &mut qualified,
        }
        .draw_iter(pattern())
        .unwrap();

        assert_eq!(canonical, qualified.buffer());
    }
}
