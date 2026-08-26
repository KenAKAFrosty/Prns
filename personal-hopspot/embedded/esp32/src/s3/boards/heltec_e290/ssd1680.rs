//! Host-testable E290 presentation policy and SSD1680 controller-byte packing.

use embedded_graphics::geometry::Point;
use personal_hopspot_core as screen;

pub(crate) const PANEL_WIDTH: u32 = 296;
pub(crate) const PANEL_HEIGHT: u32 = 128;
pub(crate) const BYTES_PER_COLUMN: usize = PANEL_HEIGHT as usize / 8;
pub(crate) const FRAME_BYTES: usize = PANEL_WIDTH as usize * BYTES_PER_COLUMN;

pub(crate) const TELEMETRY_REFRESH_MINIMUM_MS: u64 = 30_000;

pub(crate) type E290Presentation =
    screen::presentation::ExactPresentationState<screen::face_64x128::Frame>;

const _: () = assert!(PANEL_HEIGHT.is_multiple_of(8));
const _: () = assert!(FRAME_BYTES == 4_736);

pub(crate) fn presentation_state() -> E290Presentation {
    let telemetry = screen::presentation::NonZeroDuration::new(TELEMETRY_REFRESH_MINIMUM_MS)
        .expect("the E290 telemetry interval is nonzero");
    let policy = screen::presentation::PresentationPolicy::RetainedFullWaveformOnly(
        screen::presentation::RetainedFullWaveformOnlyPolicy::new(
            // The display owner serializes complete SSD1680 waveforms. There is no separate
            // evidence-backed post-operation dwell, so changed user-input frames may begin as soon
            // as the preceding operation completes; routine telemetry remains rate-limited below.
            screen::presentation::PresentationSpacing::OperationCompletionOnly,
            telemetry,
            screen::presentation::RetryBackoff::NextRenderOpportunity,
        ),
    );
    screen::presentation::ExactPresentationState::new(
        screen::face_64x128::Frame::new(),
        screen::face_64x128::Frame::new(),
        policy,
    )
}

/// Coordinate-only projection into the SSD1680's native stream order. The panel consumes sixteen
/// vertical bytes for each landscape x coordinate; set bits are white and cleared bits are black.
pub(crate) struct ControllerPacking {
    transform: screen::PanelTransform,
}

impl ControllerPacking {
    pub(crate) fn front_facing() -> Self {
        let transform = screen::PanelTransform::centered_clockwise_quarter_turn(
            screen::LogicalSize::new(
                screen::face_64x128::LOGICAL_WIDTH,
                screen::face_64x128::LOGICAL_HEIGHT,
            ),
            screen::PanelSize::new(PANEL_WIDTH, PANEL_HEIGHT),
            screen::PanelScale::TwoToOne,
        )
        .expect("the E290 face viewport fits the physical panel");
        Self { transform }
    }

    pub(crate) fn fill(
        &self,
        frame: &screen::face_64x128::Frame,
        offset: usize,
        output: &mut [u8],
    ) {
        assert!(offset.saturating_add(output.len()) <= FRAME_BYTES);
        for (relative, byte) in output.iter_mut().enumerate() {
            *byte = self.byte(frame, offset + relative);
        }
    }

    fn byte(&self, frame: &screen::face_64x128::Frame, index: usize) -> u8 {
        let controller_x = index / BYTES_PER_COLUMN;
        // Powered V0.3.1 E290 fixtures establish that increasing SSD1680 gate addresses run from
        // the viewer's right edge toward the left. Reflect that controller order before applying
        // the front-facing clockwise panel transform; otherwise the complete face is mirrored.
        let panel_x = PANEL_WIDTH as usize - 1 - controller_x;
        let first_y = (index % BYTES_PER_COLUMN) * 8;
        let mut byte = u8::MAX;
        for bit in 0..8 {
            let mapped = self
                .transform
                .map_panel_point(screen::PhysicalPoint::new(
                    panel_x as u32,
                    (first_y + bit) as u32,
                ))
                .expect("the controller stream contains only in-panel points");
            let black = matches!(
                mapped,
                screen::MappedPoint::Source(source)
                    if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32))
            );
            if black {
                byte &= !(1 << (7 - bit));
            }
        }
        byte
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{draw_target::DrawTarget, pixelcolor::BinaryColor, Pixel};

    use super::*;

    fn packed(frame: &screen::face_64x128::Frame) -> [u8; FRAME_BYTES] {
        let mut bytes = [0u8; FRAME_BYTES];
        ControllerPacking::front_facing().fill(frame, 0, &mut bytes);
        bytes
    }

    #[test]
    fn white_frame_and_twenty_pixel_side_margins_are_exact() {
        let frame = screen::face_64x128::Frame::new();
        assert!(packed(&frame).iter().all(|byte| *byte == u8::MAX));

        let mut black = screen::face_64x128::Frame::new();
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
    fn asymmetric_logical_corners_match_powered_front_facing_controller_order() {
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
            let mut frame = screen::face_64x128::Frame::new();
            frame.draw_iter([Pixel(logical, BinaryColor::On)]).unwrap();
            let bytes = packed(&frame);
            assert_eq!(bytes[byte_index], expected, "logical corner {logical:?}");
            assert_eq!(
                bytes.iter().filter(|byte| **byte != u8::MAX).count(),
                2,
                "two scaled controller bytes carry each 2x2 logical corner"
            );
        }
    }

    #[test]
    fn retained_policy_never_selects_partial_and_separates_input_from_telemetry() {
        use screen::presentation::{
            ExactPresentationDecision, MonotonicMillis, PresentationUrgency, RefreshKind,
        };

        let mut state = presentation_state();
        state
            .working_mut()
            .unwrap()
            .draw_iter([Pixel(Point::new(1, 1), BinaryColor::On)])
            .unwrap();
        let ExactPresentationDecision::Present(first) = state
            .plan(MonotonicMillis::new(0), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("the unknown initial frame must be presented")
        };
        assert_eq!(first.kind(), RefreshKind::RetainedFullWaveform);
        state
            .attempt_succeeded(first, MonotonicMillis::new(100))
            .unwrap();

        state
            .working_mut()
            .unwrap()
            .draw_iter([Pixel(Point::new(1, 1), BinaryColor::On)])
            .unwrap();
        assert!(matches!(
            state
                .plan(MonotonicMillis::new(100), PresentationUrgency::Telemetry)
                .unwrap(),
            ExactPresentationDecision::Unchanged
        ));

        state
            .working_mut()
            .unwrap()
            .draw_iter([Pixel(Point::new(2, 2), BinaryColor::On)])
            .unwrap();
        let ExactPresentationDecision::Present(input) = state
            .plan(MonotonicMillis::new(100), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("input may refresh as soon as the prior waveform completes")
        };
        assert_eq!(input.kind(), RefreshKind::RetainedFullWaveform);
        state
            .attempt_succeeded(input, MonotonicMillis::new(200))
            .unwrap();

        state
            .working_mut()
            .unwrap()
            .draw_iter([Pixel(Point::new(3, 3), BinaryColor::On)])
            .unwrap();
        assert!(matches!(
            state
                .plan(MonotonicMillis::new(10_200), PresentationUrgency::Telemetry)
                .unwrap(),
            ExactPresentationDecision::DeferredUntil(deadline)
                if deadline == MonotonicMillis::new(30_200)
        ));
    }
}
