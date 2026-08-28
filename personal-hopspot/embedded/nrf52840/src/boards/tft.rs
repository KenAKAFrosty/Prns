use embedded_graphics::prelude::Point;
use personal_hopspot_core::face_64x128::{Frame, MappedPoint, PanelTransform, PhysicalPoint};

pub(super) fn rgb565_pixel(
    frame: &Frame,
    transform: &PanelTransform,
    point: PhysicalPoint,
) -> [u8; 2] {
    match transform.map_panel_point(point) {
        Ok(MappedPoint::Source(source))
            if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32)) =>
        {
            [0xff, 0xff]
        }
        Ok(MappedPoint::Source(_) | MappedPoint::Margin) | Err(_) => [0x00, 0x00],
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::prelude::{DrawTarget, Pixel};
    use personal_hopspot_core::face_64x128::{PanelScale, PanelSize, QuarterTurn};

    use super::*;

    fn patterned_frame() -> Frame {
        let mut frame = Frame::new();
        frame
            .draw_iter((0..128).flat_map(|y| {
                (0..64).map(move |x| {
                    let color = if (x * 3 + y * 5) % 11 < 5 {
                        BinaryColor::On
                    } else {
                        BinaryColor::Off
                    };
                    Pixel(Point::new(x, y), color)
                })
            }))
            .unwrap();
        frame
    }

    #[test]
    fn t096_rgb565_matches_the_qualified_rotation_at_every_pixel() {
        let transform = PanelTransform::centered(
            PanelSize::new(160, 80).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::Clockwise,
        )
        .unwrap();
        let frame = patterned_frame();

        for y in 0..64 {
            for x in 0..128 {
                let source = Point::new(y, 127 - x);
                let expected = if frame.pixel_is_on(source) {
                    [0xff, 0xff]
                } else {
                    [0x00, 0x00]
                };
                assert_eq!(
                    rgb565_pixel(
                        &frame,
                        &transform,
                        PhysicalPoint::new(16 + x as u32, 8 + y as u32)
                    ),
                    expected
                );
            }
        }
        assert_eq!(
            rgb565_pixel(&frame, &transform, PhysicalPoint::new(0, 0)),
            [0x00, 0x00]
        );
    }

    #[test]
    fn t114_rgb565_matches_the_qualified_scaling_at_every_pixel() {
        let transform = PanelTransform::centered(
            PanelSize::new(240, 135).unwrap(),
            PanelScale::FifteenToEight,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        let frame = patterned_frame();

        for y in 0..120 {
            for x in 0..240 {
                let source = Point::new(63 - y * 64 / 120, x * 128 / 240);
                let expected = if frame.pixel_is_on(source) {
                    [0xff, 0xff]
                } else {
                    [0x00, 0x00]
                };
                assert_eq!(
                    rgb565_pixel(
                        &frame,
                        &transform,
                        PhysicalPoint::new(x as u32, 7 + y as u32)
                    ),
                    expected
                );
            }
        }
        assert_eq!(
            rgb565_pixel(&frame, &transform, PhysicalPoint::new(0, 0)),
            [0x00, 0x00]
        );
    }
}
