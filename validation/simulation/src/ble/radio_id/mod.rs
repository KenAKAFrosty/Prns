use super::BleSimulationError;

/// Identifies one attachment within a medium. IDs are never reused in that medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleRadioId(u64);

impl BleRadioId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

pub(super) enum RadioIdSequence {
    Available(BleRadioId),
    Exhausted,
}

impl RadioIdSequence {
    pub(super) const fn new() -> Self {
        Self::Available(BleRadioId(0))
    }

    pub(super) fn issue(&mut self) -> Result<BleRadioId, BleSimulationError> {
        let Self::Available(radio) = *self else {
            return Err(BleSimulationError::RadioIdsExhausted);
        };
        *self = match radio.0.checked_add(1) {
            Some(next) => Self::Available(BleRadioId(next)),
            None => Self::Exhausted,
        };
        Ok(radio)
    }
}

#[cfg(test)]
mod tests;
