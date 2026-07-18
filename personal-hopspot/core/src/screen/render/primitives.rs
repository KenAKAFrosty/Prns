use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};

pub(super) fn fill(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_fill(color)
}

pub(super) fn stroke(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_stroke(color, 1)
}

pub(super) fn line<D: DrawTarget<Color = BinaryColor>>(display: &mut D, a: Point, b: Point) {
    line_colored(display, a, b, BinaryColor::On);
}

pub(super) fn line_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    a: Point,
    b: Point,
    color: BinaryColor,
) {
    let _ = Line::new(a, b).into_styled(stroke(color)).draw(display);
}

pub(super) fn draw_pattern_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    rows: &[&str],
    color: BinaryColor,
) {
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, pixel) in row.as_bytes().iter().enumerate() {
            if *pixel == b'#' {
                let _ = Rectangle::new(
                    Point::new(x + col_index as i32, y + row_index as i32),
                    Size::new(1, 1),
                )
                .into_styled(fill(color))
                .draw(display);
            }
        }
    }
}
