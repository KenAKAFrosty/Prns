use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::report) struct ResourceComparison {
    pub(super) target: String,
    pub(super) settings: Vec<SettingDifference>,
    pub(super) status: StatusComparison,
    pub(super) flash: EvidenceComparison<FlashComparison>,
    pub(super) artifacts: EvidenceComparison<Vec<ArtifactComparison>>,
    pub(super) ram: EvidenceComparison<Vec<RamComparison>>,
    pub(super) sections: EvidenceComparison<Vec<SectionComparison>>,
    pub(super) attribution: AttributionComparison,
    pub(super) executable: EvidenceComparison<ExecutableComparison>,
    pub(super) stack: StackEvidenceComparison,
    pub(super) async_memory: EvidenceComparison<AsyncMemoryComparison>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SettingDifference {
    Lto { before: String, after: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StatusComparison {
    pub(super) before: StatusKind,
    pub(super) after: StatusKind,
    pub(super) overflows: Vec<OverflowComparison>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusKind {
    Success,
    MemoryOverflow,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OverflowComparison {
    pub(super) linker_region: String,
    pub(super) before: OverflowState,
    pub(super) after: OverflowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverflowState {
    NoOverflow,
    NotReported,
    Overflow(u64),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EvidenceComparison<T> {
    Comparable(T),
    NotComparable {
        before: EvidenceAvailability,
        after: EvidenceAvailability,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EvidenceAvailability {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct FlashComparison {
    pub(super) image: ByteComparison,
    pub(super) headroom: ByteComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ArtifactComparison {
    pub(super) path: String,
    pub(super) bytes: ByteComparison,
    pub(super) fingerprint: ChangeState,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RamComparison {
    pub(super) backing_store: String,
    pub(super) static_sections: ByteComparison,
    pub(super) linker_padding: ByteComparison,
    pub(super) headroom: RamHeadroomComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RamHeadroomComparison {
    Known(ByteComparison),
    RuntimeDetected,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SectionComparison {
    pub(super) kind: &'static str,
    pub(super) run_bytes: ByteComparison,
    pub(super) load_bytes: ByteComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExecutableComparison {
    pub(super) entry_point: ChangeState,
    pub(super) section_bytes: ByteComparison,
    pub(super) changed_sections: Vec<String>,
    pub(super) function_boundaries: ChangeState,
    pub(super) functions_before: u64,
    pub(super) functions_after: u64,
    pub(super) changed_ranked_functions: Vec<String>,
    pub(super) decoded_bytes: ByteComparison,
    pub(super) undecoded_bytes: ByteComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StackEvidenceComparison {
    Comparable {
        before: EvidenceAvailability,
        after: EvidenceAvailability,
        value: Box<StackComparison>,
    },
    NotComparable {
        before: EvidenceAvailability,
        after: EvidenceAvailability,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StackComparison {
    pub(super) frame_source: ChangeState,
    pub(super) source_bytes: ByteComparison,
    pub(super) frames: CountComparison,
    pub(super) known_path: ByteComparison,
    pub(super) changed_largest_frames: Vec<String>,
    pub(super) limit: StackLimitComparison,
    pub(super) gaps: Vec<NamedCountComparison>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StackLimitComparison {
    Declared {
        reservation: String,
        bytes: u64,
        headroom: ByteComparison,
    },
    Undeclared,
    Changed,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AsyncMemoryComparison {
    pub(super) task_pool_total: ByteComparison,
    pub(super) task_pools: Vec<NamedSizeComparison>,
    pub(super) scenario_futures: ScenarioFutureComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ScenarioFutureComparison {
    Measured(Vec<NamedSizeComparison>),
    Unavailable { before: String, after: String },
    AvailabilityChanged { before: String, after: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct NamedSizeComparison {
    pub(super) name: String,
    pub(super) before: Option<u64>,
    pub(super) after: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct NamedCountComparison {
    pub(super) name: String,
    pub(super) count: CountComparison,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CountComparison {
    pub(super) before: u64,
    pub(super) after: u64,
    pub(super) delta: CountDelta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CountDelta {
    Decrease(u64),
    Unchanged,
    Increase(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChangeState {
    Unchanged,
    Changed,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AttributionComparison {
    Comparable {
        before: EvidenceAvailability,
        after: EvidenceAvailability,
        categories: Box<AttributionCategoriesComparison>,
    },
    NotComparable {
        before: EvidenceAvailability,
        after: EvidenceAvailability,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AttributionCategoriesComparison {
    pub(super) crates: AttributionCategoryComparison,
    pub(super) symbols: AttributionCategoryComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AttributionCategoryComparison {
    pub(super) coverage: AttributionCoverageComparison,
    pub(super) candidates: Vec<AttributionCandidateComparison>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AttributionCoverageComparison {
    pub(super) analyzed: ByteComparison,
    pub(super) attributed: ByteComparison,
    pub(super) unclassified: ByteComparison,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AttributionCandidateComparison {
    pub(super) rank: usize,
    pub(super) name: String,
    pub(super) before: AttributionCandidateBaseline,
    pub(super) after_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttributionCandidateBaseline {
    Ranked(u64),
    NotRanked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ByteComparison {
    pub(super) before: u64,
    pub(super) after: u64,
    pub(super) delta: ByteDelta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ByteDelta {
    Decrease(u64),
    Unchanged,
    Increase(u64),
}

impl ByteComparison {
    pub(super) const fn new(before: u64, after: u64) -> Self {
        let delta = if before < after {
            ByteDelta::Increase(after - before)
        } else if before == after {
            ByteDelta::Unchanged
        } else {
            ByteDelta::Decrease(before - after)
        };
        Self {
            before,
            after,
            delta,
        }
    }
}

impl CountComparison {
    pub(super) const fn new(before: u64, after: u64) -> Self {
        let delta = if before < after {
            CountDelta::Increase(after - before)
        } else if before == after {
            CountDelta::Unchanged
        } else {
            CountDelta::Decrease(before - after)
        };
        Self {
            before,
            after,
            delta,
        }
    }
}

impl fmt::Display for ByteDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decrease(bytes) => write!(formatter, "-{bytes}"),
            Self::Unchanged => formatter.write_str("0"),
            Self::Increase(bytes) => write!(formatter, "+{bytes}"),
        }
    }
}

impl fmt::Display for CountDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decrease(count) => write!(formatter, "-{count}"),
            Self::Unchanged => formatter.write_str("0"),
            Self::Increase(count) => write!(formatter, "+{count}"),
        }
    }
}

impl fmt::Display for StatusKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Success => "success",
            Self::MemoryOverflow => "memory-overflow",
        })
    }
}

impl fmt::Display for EvidenceAvailability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Unavailable => "unavailable",
        })
    }
}

impl fmt::Display for OverflowState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOverflow => formatter.write_str("none"),
            Self::NotReported => formatter.write_str("not-reported"),
            Self::Overflow(bytes) => write!(formatter, "{bytes}"),
        }
    }
}

impl fmt::Display for ChangeState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unchanged => "unchanged",
            Self::Changed => "changed",
        })
    }
}
