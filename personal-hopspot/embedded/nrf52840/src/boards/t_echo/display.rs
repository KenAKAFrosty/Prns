use embedded_graphics::geometry::Point;
use embedded_graphics::prelude::{DrawTarget, Pixel};
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;
use personal_hopspot_core::{
    face_64x128, LogicalSize, MappedPoint, PanelScale, PanelSize, PanelTransform, PhysicalPoint,
};

pub(crate) fn transform() -> PanelTransform {
    PanelTransform::centered_counterclockwise_quarter_turn(
        LogicalSize::new(face_64x128::LOGICAL_WIDTH, face_64x128::LOGICAL_HEIGHT),
        PanelSize::new(200, 200),
        PanelScale::ThreeToTwo,
    )
    .expect("the T-Echo face viewport fits its panel")
}

pub(crate) fn write_face(
    frame: &face_64x128::Frame,
    transform: &PanelTransform,
    panel: &mut Display1in54,
) {
    let viewport = transform.viewport();
    let origin = viewport.origin();
    let size = viewport.size();
    let pixels = (origin.y()..origin.y() + size.height()).flat_map(|y| {
        (origin.x()..origin.x() + size.width()).map(move |x| {
            let mapped = transform
                .map_panel_point(PhysicalPoint::new(x, y))
                .expect("the viewport is inside the T-Echo panel");
            let color = match mapped {
                MappedPoint::Source(point)
                    if frame.pixel_is_on(Point::new(point.x() as i32, point.y() as i32)) =>
                {
                    EpdColor::Black
                }
                MappedPoint::Source(_) | MappedPoint::Margin => EpdColor::White,
            };
            Pixel(Point::new(x as i32, y as i32), color)
        })
    });
    let _ = panel.draw_iter(pixels);
}
