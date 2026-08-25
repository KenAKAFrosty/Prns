//! Checked geometry from a logical face to a centered, quarter-turned panel viewport.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalSize {
    width: u32,
    height: u32,
}

impl LogicalSize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelSize {
    width: u32,
    height: u32,
}

impl PanelSize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalPoint {
    x: u32,
    y: u32,
}

impl LogicalPoint {
    #[must_use]
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalPoint {
    x: u32,
    y: u32,
}

impl PhysicalPoint {
    #[must_use]
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelScale {
    OneToOne,
    ThreeToTwo,
    FifteenToEight,
    TwoToOne,
}

impl PanelScale {
    const fn ratio(self) -> (u32, u32) {
        match self {
            Self::OneToOne => (1, 1),
            Self::ThreeToTwo => (3, 2),
            Self::FifteenToEight => (15, 8),
            Self::TwoToOne => (2, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuarterTurn {
    Clockwise,
    Counterclockwise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelViewport {
    origin: PhysicalPoint,
    size: PanelSize,
}

impl PanelViewport {
    #[must_use]
    pub const fn origin(self) -> PhysicalPoint {
        self.origin
    }

    #[must_use]
    pub const fn size(self) -> PanelSize {
        self.size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappedPoint {
    Source(LogicalPoint),
    Margin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointMapError {
    OutsidePanel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformError {
    ZeroLogicalDimension,
    ZeroPanelDimension,
    ArithmeticOverflow,
    ViewportDoesNotFit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelTransform {
    logical: LogicalSize,
    panel: PanelSize,
    scaled: LogicalSize,
    scale: PanelScale,
    turn: QuarterTurn,
    viewport: PanelViewport,
}

impl PanelTransform {
    /// Build a centered transform that rotates the scaled face clockwise as viewed from the front.
    pub fn centered_clockwise_quarter_turn(
        logical: LogicalSize,
        panel: PanelSize,
        scale: PanelScale,
    ) -> Result<Self, TransformError> {
        Self::centered(logical, panel, scale, QuarterTurn::Clockwise)
    }

    /// Build a centered transform that rotates the scaled face counterclockwise as viewed from the front.
    pub fn centered_counterclockwise_quarter_turn(
        logical: LogicalSize,
        panel: PanelSize,
        scale: PanelScale,
    ) -> Result<Self, TransformError> {
        Self::centered(logical, panel, scale, QuarterTurn::Counterclockwise)
    }

    fn centered(
        logical: LogicalSize,
        panel: PanelSize,
        scale: PanelScale,
        turn: QuarterTurn,
    ) -> Result<Self, TransformError> {
        if logical.width == 0 || logical.height == 0 {
            return Err(TransformError::ZeroLogicalDimension);
        }
        if panel.width == 0 || panel.height == 0 {
            return Err(TransformError::ZeroPanelDimension);
        }
        let (numerator, denominator) = scale.ratio();
        let scaled = LogicalSize::new(
            scale_dimension(logical.width, numerator, denominator)?,
            scale_dimension(logical.height, numerator, denominator)?,
        );
        let viewport_size = PanelSize::new(scaled.height, scaled.width);
        if viewport_size.width > panel.width || viewport_size.height > panel.height {
            return Err(TransformError::ViewportDoesNotFit);
        }
        let viewport = PanelViewport {
            origin: PhysicalPoint::new(
                (panel.width - viewport_size.width) / 2,
                (panel.height - viewport_size.height) / 2,
            ),
            size: viewport_size,
        };
        Ok(Self {
            logical,
            panel,
            scaled,
            scale,
            turn,
            viewport,
        })
    }

    #[must_use]
    pub const fn viewport(&self) -> PanelViewport {
        self.viewport
    }

    pub fn map_panel_point(&self, point: PhysicalPoint) -> Result<MappedPoint, PointMapError> {
        if point.x >= self.panel.width || point.y >= self.panel.height {
            return Err(PointMapError::OutsidePanel);
        }
        let origin = self.viewport.origin;
        let size = self.viewport.size;
        let Some(u) = point.x.checked_sub(origin.x) else {
            return Ok(MappedPoint::Margin);
        };
        let Some(v) = point.y.checked_sub(origin.y) else {
            return Ok(MappedPoint::Margin);
        };
        if u >= size.width || v >= size.height {
            return Ok(MappedPoint::Margin);
        }

        let (scaled_x, scaled_y) = match self.turn {
            QuarterTurn::Clockwise => (v, self.scaled.height - 1 - u),
            QuarterTurn::Counterclockwise => (self.scaled.width - 1 - v, u),
        };
        let (numerator, denominator) = self.scale.ratio();
        let logical_x = inverse_scaled_coordinate(scaled_x, numerator, denominator);
        let logical_y = inverse_scaled_coordinate(scaled_y, numerator, denominator);
        debug_assert!(logical_x < self.logical.width && logical_y < self.logical.height);
        Ok(MappedPoint::Source(LogicalPoint::new(logical_x, logical_y)))
    }
}

fn scale_dimension(
    dimension: u32,
    numerator: u32,
    denominator: u32,
) -> Result<u32, TransformError> {
    let scaled = u64::from(dimension)
        .checked_mul(u64::from(numerator))
        .ok_or(TransformError::ArithmeticOverflow)?
        / u64::from(denominator);
    u32::try_from(scaled).map_err(|_| TransformError::ArithmeticOverflow)
}

fn inverse_scaled_coordinate(scaled: u32, numerator: u32, denominator: u32) -> u32 {
    let dividend = (u64::from(scaled) + 1) * u64::from(denominator) - 1;
    (dividend / u64::from(numerator)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACE: LogicalSize = LogicalSize::new(64, 128);

    #[test]
    fn known_board_viewports_are_exact() {
        let t_beam = PanelTransform::centered_clockwise_quarter_turn(
            FACE,
            PanelSize::new(128, 64),
            PanelScale::OneToOne,
        )
        .unwrap();
        assert_eq!(
            t_beam.viewport(),
            PanelViewport {
                origin: PhysicalPoint::new(0, 0),
                size: PanelSize::new(128, 64),
            }
        );

        let t096 = PanelTransform::centered_clockwise_quarter_turn(
            FACE,
            PanelSize::new(160, 80),
            PanelScale::OneToOne,
        )
        .unwrap();
        assert_eq!(t096.viewport().origin(), PhysicalPoint::new(16, 8));

        let t114 = PanelTransform::centered_counterclockwise_quarter_turn(
            FACE,
            PanelSize::new(240, 135),
            PanelScale::FifteenToEight,
        )
        .unwrap();
        assert_eq!(
            t114.viewport(),
            PanelViewport {
                origin: PhysicalPoint::new(0, 7),
                size: PanelSize::new(240, 120),
            }
        );

        let t_echo = PanelTransform::centered_counterclockwise_quarter_turn(
            FACE,
            PanelSize::new(200, 200),
            PanelScale::ThreeToTwo,
        )
        .unwrap();
        assert_eq!(
            t_echo.viewport(),
            PanelViewport {
                origin: PhysicalPoint::new(4, 52),
                size: PanelSize::new(192, 96),
            }
        );

        let e290 = PanelTransform::centered_clockwise_quarter_turn(
            FACE,
            PanelSize::new(296, 128),
            PanelScale::TwoToOne,
        )
        .unwrap();
        assert_eq!(
            e290.viewport(),
            PanelViewport {
                origin: PhysicalPoint::new(20, 0),
                size: PanelSize::new(256, 128),
            }
        );
    }

    #[test]
    fn labeled_corners_map_for_both_turns() {
        let clockwise = PanelTransform::centered_clockwise_quarter_turn(
            LogicalSize::new(3, 2),
            PanelSize::new(2, 3),
            PanelScale::OneToOne,
        )
        .unwrap();
        assert_eq!(
            clockwise.map_panel_point(PhysicalPoint::new(0, 0)),
            Ok(MappedPoint::Source(LogicalPoint::new(0, 1)))
        );
        assert_eq!(
            clockwise.map_panel_point(PhysicalPoint::new(1, 2)),
            Ok(MappedPoint::Source(LogicalPoint::new(2, 0)))
        );

        let counterclockwise = PanelTransform::centered_counterclockwise_quarter_turn(
            LogicalSize::new(3, 2),
            PanelSize::new(2, 3),
            PanelScale::OneToOne,
        )
        .unwrap();
        assert_eq!(
            counterclockwise.map_panel_point(PhysicalPoint::new(0, 0)),
            Ok(MappedPoint::Source(LogicalPoint::new(2, 0)))
        );
        assert_eq!(
            counterclockwise.map_panel_point(PhysicalPoint::new(1, 2)),
            Ok(MappedPoint::Source(LogicalPoint::new(0, 1)))
        );
    }

    #[test]
    fn margins_and_invalid_panel_points_are_distinct() {
        let transform = PanelTransform::centered_clockwise_quarter_turn(
            FACE,
            PanelSize::new(160, 80),
            PanelScale::OneToOne,
        )
        .unwrap();
        assert_eq!(
            transform.map_panel_point(PhysicalPoint::new(0, 0)),
            Ok(MappedPoint::Margin)
        );
        assert_eq!(
            transform.map_panel_point(PhysicalPoint::new(160, 0)),
            Err(PointMapError::OutsidePanel)
        );
    }

    #[test]
    fn construction_rejects_each_invalid_geometry_class() {
        assert_eq!(
            PanelTransform::centered_clockwise_quarter_turn(
                LogicalSize::new(0, 1),
                PanelSize::new(1, 1),
                PanelScale::OneToOne,
            ),
            Err(TransformError::ZeroLogicalDimension)
        );
        assert_eq!(
            PanelTransform::centered_clockwise_quarter_turn(
                LogicalSize::new(1, 1),
                PanelSize::new(0, 1),
                PanelScale::OneToOne,
            ),
            Err(TransformError::ZeroPanelDimension)
        );
        assert_eq!(
            PanelTransform::centered_clockwise_quarter_turn(
                LogicalSize::new(u32::MAX, 1),
                PanelSize::new(u32::MAX, u32::MAX),
                PanelScale::TwoToOne,
            ),
            Err(TransformError::ArithmeticOverflow)
        );
        assert_eq!(
            PanelTransform::centered_clockwise_quarter_turn(
                FACE,
                PanelSize::new(127, 64),
                PanelScale::OneToOne,
            ),
            Err(TransformError::ViewportDoesNotFit)
        );
    }

    #[test]
    fn geometry_uses_the_explicit_non_product_logical_size() {
        let transform = PanelTransform::centered_clockwise_quarter_turn(
            LogicalSize::new(4, 6),
            PanelSize::new(12, 8),
            PanelScale::TwoToOne,
        )
        .unwrap();
        for y in 0..8 {
            for x in 0..12 {
                let mapped = transform.map_panel_point(PhysicalPoint::new(x, y)).unwrap();
                let MappedPoint::Source(point) = mapped else {
                    panic!("the viewport fills this panel");
                };
                assert!(point.x() < 4 && point.y() < 6);
            }
        }
    }

    #[test]
    fn forward_rectangles_and_inverse_mapping_agree_for_every_scale_and_panel_point() {
        let logical = LogicalSize::new(4, 6);
        for scale in [
            PanelScale::OneToOne,
            PanelScale::ThreeToTwo,
            PanelScale::TwoToOne,
        ] {
            let (numerator, denominator) = scale.ratio();
            let scaled_width = logical.width() * numerator / denominator;
            let scaled_height = logical.height() * numerator / denominator;
            let panel = PanelSize::new(scaled_height + 3, scaled_width + 5);
            for clockwise in [true, false] {
                let transform = if clockwise {
                    PanelTransform::centered_clockwise_quarter_turn(logical, panel, scale)
                } else {
                    PanelTransform::centered_counterclockwise_quarter_turn(logical, panel, scale)
                }
                .unwrap();
                let viewport = transform.viewport();
                let origin = viewport.origin();
                let size = viewport.size();

                for panel_y in 0..panel.height() {
                    for panel_x in 0..panel.width() {
                        let mapped = transform
                            .map_panel_point(PhysicalPoint::new(panel_x, panel_y))
                            .unwrap();
                        let inside = panel_x >= origin.x()
                            && panel_x < origin.x() + size.width()
                            && panel_y >= origin.y()
                            && panel_y < origin.y() + size.height();
                        assert_eq!(matches!(mapped, MappedPoint::Source(_)), inside);
                    }
                }

                for logical_y in 0..logical.height() {
                    for logical_x in 0..logical.width() {
                        let sx0 = logical_x * numerator / denominator;
                        let sx1 = (logical_x + 1) * numerator / denominator;
                        let sy0 = logical_y * numerator / denominator;
                        let sy1 = (logical_y + 1) * numerator / denominator;
                        let (u0, u1, v0, v1) = if clockwise {
                            (scaled_height - sy1, scaled_height - sy0, sx0, sx1)
                        } else {
                            (sy0, sy1, scaled_width - sx1, scaled_width - sx0)
                        };
                        for v in v0..v1 {
                            for u in u0..u1 {
                                assert_eq!(
                                    transform.map_panel_point(PhysicalPoint::new(
                                        origin.x() + u,
                                        origin.y() + v,
                                    )),
                                    Ok(MappedPoint::Source(LogicalPoint::new(
                                        logical_x, logical_y,
                                    )))
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
