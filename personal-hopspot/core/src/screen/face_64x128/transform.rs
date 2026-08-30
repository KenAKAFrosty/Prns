use super::{HEIGHT, WIDTH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelSize {
    width: u32,
    height: u32,
}

impl PanelSize {
    pub const fn new(width: u32, height: u32) -> Result<Self, PanelSizeError> {
        if width == 0 {
            return Err(PanelSizeError::ZeroWidth);
        }
        if height == 0 {
            return Err(PanelSizeError::ZeroHeight);
        }
        Ok(Self { width, height })
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

#[derive(Debug, Eq, PartialEq)]
pub enum PanelSizeError {
    ZeroWidth,
    ZeroHeight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalPoint {
    x: u32,
    y: u32,
}

impl LogicalPoint {
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
    TwoToOne,
    FifteenToEight,
}

impl PanelScale {
    const fn ratio(self) -> (u32, u32) {
        match self {
            Self::OneToOne => (1, 1),
            Self::ThreeToTwo => (3, 2),
            Self::TwoToOne => (2, 1),
            Self::FifteenToEight => (15, 8),
        }
    }

    fn source_coordinate(self, scaled: u32) -> u32 {
        let (numerator, denominator) = self.ratio();
        match self {
            // Retained panels paint source rectangles; the TFTs sample once per destination pixel.
            // Their qualified rasterization rules therefore require different inverses.
            Self::ThreeToTwo | Self::TwoToOne => {
                covered_source_coordinate(scaled, numerator, denominator)
            }
            Self::OneToOne | Self::FifteenToEight => {
                sampled_source_coordinate(scaled, numerator, denominator)
            }
        }
    }

    fn reversed_source_coordinate(self, scaled: u32) -> u32 {
        let (numerator, denominator) = self.ratio();
        covered_source_coordinate(scaled, numerator, denominator)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarterTurn {
    Clockwise,
    CounterClockwise,
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

#[derive(Debug, Eq, PartialEq)]
pub enum PointMapError {
    OutsidePanel,
}

#[derive(Debug, Eq, PartialEq)]
pub enum TransformError {
    ViewportDoesNotFit,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PanelTransform {
    panel: PanelSize,
    scaled_width: u32,
    scaled_height: u32,
    scale: PanelScale,
    turn: QuarterTurn,
    viewport: PanelViewport,
}

impl PanelTransform {
    pub const fn centered(
        panel: PanelSize,
        scale: PanelScale,
        turn: QuarterTurn,
    ) -> Result<Self, TransformError> {
        let (numerator, denominator) = scale.ratio();
        let scaled_width = WIDTH * numerator / denominator;
        let scaled_height = HEIGHT * numerator / denominator;
        let viewport_size = PanelSize {
            width: scaled_height,
            height: scaled_width,
        };
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
            panel,
            scaled_width,
            scaled_height,
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
            QuarterTurn::Clockwise => (v, self.scaled_height - 1 - u),
            QuarterTurn::CounterClockwise => (self.scaled_width - 1 - v, u),
        };
        Ok(MappedPoint::Source(LogicalPoint {
            x: match self.turn {
                QuarterTurn::Clockwise => self.scale.source_coordinate(scaled_x),
                QuarterTurn::CounterClockwise => self.scale.reversed_source_coordinate(scaled_x),
            },
            y: match self.turn {
                QuarterTurn::Clockwise => self.scale.reversed_source_coordinate(scaled_y),
                QuarterTurn::CounterClockwise => self.scale.source_coordinate(scaled_y),
            },
        }))
    }
}

fn sampled_source_coordinate(scaled: u32, numerator: u32, denominator: u32) -> u32 {
    ((u64::from(scaled) * u64::from(denominator)) / u64::from(numerator)) as u32
}

fn covered_source_coordinate(scaled: u32, numerator: u32, denominator: u32) -> u32 {
    let dividend = (u64::from(scaled) + 1) * u64::from(denominator) - 1;
    (dividend / u64::from(numerator)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipping_board_viewports_are_exact() {
        let t_beam = PanelTransform::centered(
            PanelSize::new(128, 64).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::Clockwise,
        )
        .unwrap();
        assert_eq!(t_beam.viewport().origin(), PhysicalPoint::new(0, 0));
        assert_eq!(t_beam.viewport().size(), PanelSize::new(128, 64).unwrap());

        let t096 = PanelTransform::centered(
            PanelSize::new(160, 80).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::Clockwise,
        )
        .unwrap();
        assert_eq!(t096.viewport().origin(), PhysicalPoint::new(16, 8));

        let t114 = PanelTransform::centered(
            PanelSize::new(240, 135).unwrap(),
            PanelScale::FifteenToEight,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        assert_eq!(t114.viewport().origin(), PhysicalPoint::new(0, 7));
        assert_eq!(t114.viewport().size(), PanelSize::new(240, 120).unwrap());

        let t_echo = PanelTransform::centered(
            PanelSize::new(200, 200).unwrap(),
            PanelScale::ThreeToTwo,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        assert_eq!(t_echo.viewport().origin(), PhysicalPoint::new(4, 52));
        assert_eq!(t_echo.viewport().size(), PanelSize::new(192, 96).unwrap());

        let e290 = PanelTransform::centered(
            PanelSize::new(296, 128).unwrap(),
            PanelScale::TwoToOne,
            QuarterTurn::Clockwise,
        )
        .unwrap();
        assert_eq!(e290.viewport().origin(), PhysicalPoint::new(20, 0));
        assert_eq!(e290.viewport().size(), PanelSize::new(256, 128).unwrap());
    }

    #[test]
    fn rotations_map_labeled_corners() {
        let clockwise = PanelTransform::centered(
            PanelSize::new(128, 64).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::Clockwise,
        )
        .unwrap();
        assert_eq!(
            clockwise.map_panel_point(PhysicalPoint::new(0, 0)),
            Ok(MappedPoint::Source(LogicalPoint { x: 0, y: 127 }))
        );
        assert_eq!(
            clockwise.map_panel_point(PhysicalPoint::new(127, 63)),
            Ok(MappedPoint::Source(LogicalPoint { x: 63, y: 0 }))
        );

        let counterclockwise = PanelTransform::centered(
            PanelSize::new(128, 64).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        assert_eq!(
            counterclockwise.map_panel_point(PhysicalPoint::new(0, 0)),
            Ok(MappedPoint::Source(LogicalPoint { x: 63, y: 0 }))
        );
        assert_eq!(
            counterclockwise.map_panel_point(PhysicalPoint::new(127, 63)),
            Ok(MappedPoint::Source(LogicalPoint { x: 0, y: 127 }))
        );
    }

    #[test]
    fn t114_mapping_matches_the_qualified_tft_sampling_at_every_pixel() {
        let transform = PanelTransform::centered(
            PanelSize::new(240, 135).unwrap(),
            PanelScale::FifteenToEight,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        let origin = transform.viewport().origin();
        for physical_y in 0..120 {
            for physical_x in 0..240 {
                let expected = LogicalPoint {
                    x: 63 - physical_y * 64 / 120,
                    y: physical_x * 128 / 240,
                };
                assert_eq!(
                    transform.map_panel_point(PhysicalPoint::new(
                        origin.x() + physical_x,
                        origin.y() + physical_y,
                    )),
                    Ok(MappedPoint::Source(expected)),
                    "physical ({physical_x}, {physical_y})",
                );
            }
        }
    }

    #[test]
    fn t_echo_mapping_matches_the_qualified_rectangle_expansion_at_every_pixel() {
        let transform = PanelTransform::centered(
            PanelSize::new(200, 200).unwrap(),
            PanelScale::ThreeToTwo,
            QuarterTurn::CounterClockwise,
        )
        .unwrap();
        let origin = transform.viewport().origin();
        for source_y in 0..128 {
            let physical_x_start = source_y * 3 / 2;
            let physical_x_end = (source_y + 1) * 3 / 2;
            for source_x in 0..64 {
                let scaled_x_start = source_x * 3 / 2;
                let scaled_x_end = (source_x + 1) * 3 / 2;
                let physical_y_start = 96 - scaled_x_end;
                let physical_y_end = physical_y_start + (scaled_x_end - scaled_x_start);
                for physical_y in physical_y_start..physical_y_end {
                    for physical_x in physical_x_start..physical_x_end {
                        assert_eq!(
                            transform.map_panel_point(PhysicalPoint::new(
                                origin.x() + physical_x,
                                origin.y() + physical_y,
                            )),
                            Ok(MappedPoint::Source(LogicalPoint {
                                x: source_x,
                                y: source_y,
                            })),
                            "source ({source_x}, {source_y}), physical ({physical_x}, {physical_y})",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn margins_outside_points_and_nonfitting_panels_are_distinct() {
        let transform = PanelTransform::centered(
            PanelSize::new(160, 80).unwrap(),
            PanelScale::OneToOne,
            QuarterTurn::Clockwise,
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
        assert_eq!(
            PanelTransform::centered(
                PanelSize::new(127, 64).unwrap(),
                PanelScale::OneToOne,
                QuarterTurn::Clockwise,
            ),
            Err(TransformError::ViewportDoesNotFit)
        );
        assert_eq!(PanelSize::new(0, 64), Err(PanelSizeError::ZeroWidth));
        assert_eq!(PanelSize::new(64, 0), Err(PanelSizeError::ZeroHeight));
    }
}
