use embedded_graphics::geometry::Point;
use personal_hopspot_core::face_64x128::{
    Frame, MappedPoint, PanelScale, PanelSize, PanelTransform, PhysicalPoint, QuarterTurn,
};

pub(crate) const PANEL_WIDTH: u32 = 296;
pub(crate) const PANEL_HEIGHT: u32 = 128;
pub(crate) const BYTES_PER_COLUMN: usize = PANEL_HEIGHT as usize / 8;
pub(crate) const FRAME_BYTES: usize = PANEL_WIDTH as usize * BYTES_PER_COLUMN;

const PANEL_SIZE: PanelSize = match PanelSize::new(PANEL_WIDTH, PANEL_HEIGHT) {
    Ok(size) => size,
    Err(_) => panic!("the E290 panel dimensions are nonzero"),
};
const FRONT_FACING_TRANSFORM: PanelTransform =
    match PanelTransform::centered(PANEL_SIZE, PanelScale::TwoToOne, QuarterTurn::Clockwise) {
        Ok(transform) => transform,
        Err(_) => panic!("the E290 face fits the physical panel"),
    };

const _: () = assert!(PANEL_HEIGHT.is_multiple_of(8));
const _: () = assert!(FRAME_BYTES == 4_736);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackingError {
    OutputOutsideFrame,
    ControllerPointOutsidePanel,
}

pub(crate) struct ControllerPacking;

impl ControllerPacking {
    pub(crate) const fn front_facing() -> Self {
        Self
    }

    pub(crate) fn fill(
        &self,
        frame: &Frame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), PackingError> {
        let Some(end) = offset.checked_add(output.len()) else {
            return Err(PackingError::OutputOutsideFrame);
        };
        if end > FRAME_BYTES {
            return Err(PackingError::OutputOutsideFrame);
        }
        for (relative, byte) in output.iter_mut().enumerate() {
            *byte = self.byte(frame, offset + relative)?;
        }
        Ok(())
    }

    fn byte(&self, frame: &Frame, index: usize) -> Result<u8, PackingError> {
        let controller_x = index / BYTES_PER_COLUMN;
        // Powered E290 fixtures establish that increasing controller gates run viewer-right to
        // viewer-left, so controller X is reflected before applying the front-facing transform.
        let panel_x = PANEL_WIDTH as usize - 1 - controller_x;
        let first_y = (index % BYTES_PER_COLUMN) * 8;
        let mut byte = u8::MAX;
        for bit in 0..8 {
            let mapped = FRONT_FACING_TRANSFORM
                .map_panel_point(PhysicalPoint::new(panel_x as u32, (first_y + bit) as u32))
                .map_err(|_| PackingError::ControllerPointOutsidePanel)?;
            let black = matches!(
                mapped,
                MappedPoint::Source(source)
                    if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32))
            );
            if black {
                byte &= !(1 << (7 - bit));
            }
        }
        Ok(byte)
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{draw_target::DrawTarget, pixelcolor::BinaryColor, Pixel};

    use super::*;

    fn packed(frame: &Frame) -> [u8; FRAME_BYTES] {
        let mut bytes = [0u8; FRAME_BYTES];
        ControllerPacking::front_facing()
            .fill(frame, 0, &mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn white_frame_and_twenty_pixel_side_margins_are_exact() {
        let frame = Frame::new();
        assert!(packed(&frame).iter().all(|byte| *byte == u8::MAX));

        let mut black = Frame::new();
        black.clear(BinaryColor::On).unwrap();
        let bytes = packed(&black);
        for x in 0..PANEL_WIDTH as usize {
            let column = &bytes[x * BYTES_PER_COLUMN..(x + 1) * BYTES_PER_COLUMN];
            if (20..276).contains(&x) {
                assert!(column.iter().all(|byte| *byte == 0));
            } else {
                assert!(column.iter().all(|byte| *byte == u8::MAX));
            }
        }
    }

    #[test]
    fn asymmetric_corners_match_the_qualified_front_facing_controller_order() {
        let cases = [
            (Point::new(0, 127), 275 * BYTES_PER_COLUMN, 0x3f),
            (
                Point::new(63, 127),
                275 * BYTES_PER_COLUMN + BYTES_PER_COLUMN - 1,
                0xfc,
            ),
            (Point::new(0, 0), 20 * BYTES_PER_COLUMN, 0x3f),
            (
                Point::new(63, 0),
                20 * BYTES_PER_COLUMN + BYTES_PER_COLUMN - 1,
                0xfc,
            ),
        ];
        for (logical, byte_index, expected) in cases {
            let mut frame = Frame::new();
            frame.draw_iter([Pixel(logical, BinaryColor::On)]).unwrap();
            let bytes = packed(&frame);
            assert_eq!(bytes[byte_index], expected, "logical corner {logical:?}");
            assert_eq!(
                bytes.iter().filter(|byte| **byte != u8::MAX).count(),
                2,
                "two controller bytes carry each 2x2 logical corner"
            );
        }
    }

    #[test]
    fn writes_must_remain_inside_the_controller_frame() {
        let frame = Frame::new();
        let mut byte = [0];
        assert_eq!(
            ControllerPacking::front_facing().fill(&frame, FRAME_BYTES, &mut byte),
            Err(PackingError::OutputOutsideFrame)
        );
    }
}
