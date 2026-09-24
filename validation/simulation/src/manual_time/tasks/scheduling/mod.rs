use crate::SimulationSeed;

/// Version of the portable seed/ordinal ranking algorithm, independent of fault generation.
pub const SEEDED_TASK_SCHEDULING_ALGORITHM_VERSION: u8 = 1;

/// A fixed cyclic ordering of live actor IDs, not a random choice on each poll.
/// Continuously ready actors in a fixed finite population each receive one turn per cycle.
/// Alternative seeds explore alternative orders, not all possible interleavings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ManualTaskScheduling {
    /// Admission order; the default retained for existing scenarios.
    #[default]
    Cyclic,
    /// Order by a stable seed-derived permutation of admission ordinals.
    Seeded { seed: SimulationSeed },
}

impl ManualTaskScheduling {
    pub(super) fn rank(self, ordinal: u64) -> u64 {
        match self {
            Self::Cyclic => ordinal,
            Self::Seeded { seed } => {
                // V1: domain-separated XOR input, then the SplitMix64 finalizer.
                // All operations are explicit u64 wrapping arithmetic. This is a
                // permutation (odd multipliers and reversible XOR shifts), not a
                // cryptographic primitive or mutable PRNG shared with faults.
                let mut value = ordinal ^ seed.get() ^ 0x7461_736b_6f72_6465;
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                value ^ (value >> 31)
            }
        }
    }
}

#[cfg(test)]
mod tests;
