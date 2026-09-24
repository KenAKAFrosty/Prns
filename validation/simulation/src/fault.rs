use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransmissionOrdinal(pub(crate) u64);

impl TransmissionOrdinal {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultPlan {
    dropped_transmissions: Vec<TransmissionOrdinal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultPlanError {
    pub previous: TransmissionOrdinal,
    pub next: TransmissionOrdinal,
}

impl fmt::Display for FaultPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fault ordinals must be strictly increasing; {:?} was followed by {:?}",
            self.previous, self.next,
        )
    }
}

impl std::error::Error for FaultPlanError {}

impl FaultPlan {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            dropped_transmissions: Vec::new(),
        }
    }

    pub fn drop_transmissions(
        dropped_transmissions: Vec<TransmissionOrdinal>,
    ) -> Result<Self, FaultPlanError> {
        for pair in dropped_transmissions.windows(2) {
            if pair[0] >= pair[1] {
                return Err(FaultPlanError {
                    previous: pair[0],
                    next: pair[1],
                });
            }
        }
        Ok(Self {
            dropped_transmissions,
        })
    }

    pub(crate) fn drops(&self, ordinal: TransmissionOrdinal) -> bool {
        self.dropped_transmissions.binary_search(&ordinal).is_ok()
    }
}
