#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanvasDimensions {
    width: u32,
    height: u32,
}

impl CanvasDimensions {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        assert!(width > 0);
        assert!(height > 0);
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarterTurn {
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalPoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RotatedCanvasMapping {
    logical: CanvasDimensions,
    physical: CanvasDimensions,
    turn: QuarterTurn,
}

impl RotatedCanvasMapping {
    #[must_use]
    pub const fn new(
        logical: CanvasDimensions,
        physical: CanvasDimensions,
        turn: QuarterTurn,
    ) -> Self {
        Self {
            logical,
            physical,
            turn,
        }
    }

    #[must_use]
    pub const fn logical_point(self, physical_x: u32, physical_y: u32) -> LogicalPoint {
        assert!(physical_x < self.physical.width);
        assert!(physical_y < self.physical.height);
        match self.turn {
            QuarterTurn::Clockwise => LogicalPoint {
                x: physical_y * self.logical.width / self.physical.height,
                y: self.logical.height - 1 - physical_x * self.logical.height / self.physical.width,
            },
            QuarterTurn::CounterClockwise => LogicalPoint {
                x: self.logical.width - 1 - physical_y * self.logical.width / self.physical.height,
                y: physical_x * self.logical.height / self.physical.width,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGICAL: CanvasDimensions = CanvasDimensions::new(64, 128);

    #[test]
    fn clockwise_mapping_uses_every_logical_edge() {
        let mapping = RotatedCanvasMapping::new(
            LOGICAL,
            CanvasDimensions::new(128, 64),
            QuarterTurn::Clockwise,
        );
        assert_eq!(mapping.logical_point(0, 0), LogicalPoint { x: 0, y: 127 });
        assert_eq!(mapping.logical_point(127, 0), LogicalPoint { x: 0, y: 0 });
        assert_eq!(mapping.logical_point(0, 63), LogicalPoint { x: 63, y: 127 });
        assert_eq!(mapping.logical_point(127, 63), LogicalPoint { x: 63, y: 0 });
    }

    #[test]
    fn scaled_counter_clockwise_mapping_uses_full_physical_area_and_logical_edges() {
        let mapping = RotatedCanvasMapping::new(
            LOGICAL,
            CanvasDimensions::new(240, 120),
            QuarterTurn::CounterClockwise,
        );
        assert_eq!(mapping.logical_point(0, 0), LogicalPoint { x: 63, y: 0 });
        assert_eq!(
            mapping.logical_point(239, 0),
            LogicalPoint { x: 63, y: 127 }
        );
        assert_eq!(mapping.logical_point(0, 119), LogicalPoint { x: 0, y: 0 });
        assert_eq!(
            mapping.logical_point(239, 119),
            LogicalPoint { x: 0, y: 127 }
        );
        assert_eq!(
            mapping.logical_point(120, 60),
            LogicalPoint { x: 31, y: 64 }
        );
    }
}
