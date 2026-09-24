use std::vec::Vec;

#[derive(Clone, Copy)]
pub(super) enum FaultPartition {
    All,
    FirstQuarter,
    SecondQuarter,
    ThirdQuarter,
    FourthQuarter,
}

impl FaultPartition {
    pub(super) fn includes(self, boundary: usize) -> bool {
        match self {
            Self::All => true,
            Self::FirstQuarter => boundary.is_multiple_of(4),
            Self::SecondQuarter => boundary % 4 == 1,
            Self::ThirdQuarter => boundary % 4 == 2,
            Self::FourthQuarter => boundary % 4 == 3,
        }
    }
}

macro_rules! partitioned_campaign {
    ($name:ident, $exercise:ident $(, $argument:expr)* $(,)?) => {
        mod $name {
            use super::*;

            #[test]
            fn first_quarter() {
                $exercise($($argument,)* FaultPartition::FirstQuarter);
            }

            #[test]
            fn second_quarter() {
                $exercise($($argument,)* FaultPartition::SecondQuarter);
            }

            #[test]
            fn third_quarter() {
                $exercise($($argument,)* FaultPartition::ThirdQuarter);
            }

            #[test]
            fn fourth_quarter() {
                $exercise($($argument,)* FaultPartition::FourthQuarter);
            }
        }
    };
}

#[test]
fn quarters_cover_every_boundary_exactly_once() {
    let quarters = [
        FaultPartition::FirstQuarter,
        FaultPartition::SecondQuarter,
        FaultPartition::ThirdQuarter,
        FaultPartition::FourthQuarter,
    ];
    for count in [0, 1, 2, 3, 4, 5, 1026, 1291, 1491, 1555] {
        let mut combined = Vec::new();
        for quarter in quarters {
            combined.extend((0..count).filter(|&boundary| quarter.includes(boundary)));
        }
        combined.sort_unstable();
        assert_eq!(combined, (0..count).collect::<Vec<_>>());
        assert!((0..count).all(|boundary| FaultPartition::All.includes(boundary)));
    }
}
