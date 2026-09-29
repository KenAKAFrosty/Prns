use embedded_graphics::prelude::Point;
use personal_hopspot_core::face_64x128::{
    Frame, MappedPoint, PanelScale, PanelScaling, PanelSize, PanelTransform, PhysicalPoint,
    QuarterTurn,
};

const NATIVE_WIDTH: u32 = 122;
const NATIVE_HEIGHT: u32 = 250;
const ORIENTED_WIDTH: u32 = NATIVE_HEIGHT;
const ORIENTED_HEIGHT: u32 = NATIVE_WIDTH;
pub(super) const ROW_BYTES: usize = NATIVE_WIDTH.div_ceil(8) as usize;

pub(super) fn transform() -> PanelTransform {
    PanelTransform::centered(
        PanelSize::new(ORIENTED_WIDTH, ORIENTED_HEIGHT).expect("the MeshPocket panel is nonzero"),
        PanelScaling::PaintedSourceRectangles(PanelScale::SixtyOneToThirtyTwo),
        QuarterTurn::CounterClockwise,
    )
    .expect("the canonical face fits the MeshPocket panel")
}

pub(super) fn rasterize_row(
    frame: &Frame,
    transform: &PanelTransform,
    native_y: u32,
) -> [u8; ROW_BYTES] {
    let mut row = [0xff; ROW_BYTES];
    for native_x in 0..NATIVE_WIDTH {
        let oriented = PhysicalPoint::new(native_y, NATIVE_WIDTH - 1 - native_x);
        let Ok(MappedPoint::Source(source)) = transform.map_panel_point(oriented) else {
            continue;
        };
        if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32)) {
            row[native_x as usize / 8] &= !(0x80 >> (native_x % 8));
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;
    use embedded_graphics::prelude::{DrawTarget, Pixel};

    use super::*;

    #[test]
    fn canonical_face_is_centered_in_the_rotated_panel() {
        let viewport = transform().viewport();
        assert_eq!(viewport.origin(), PhysicalPoint::new(3, 0));
        assert_eq!(viewport.size(), PanelSize::new(244, 122).unwrap());
    }

    #[test]
    fn native_row_padding_stays_white() {
        let mut frame = Frame::new();
        frame
            .draw_iter(
                (0..128)
                    .flat_map(|y| (0..64).map(move |x| Pixel(Point::new(x, y), BinaryColor::On))),
            )
            .unwrap();
        let transform = transform();
        for native_y in 0..NATIVE_HEIGHT {
            assert_eq!(rasterize_row(&frame, &transform, native_y)[15] & 0x3f, 0x3f);
        }
    }
}
