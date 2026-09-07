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

impl fmt::Display for ByteDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decrease(bytes) => write!(formatter, "-{bytes}"),
            Self::Unchanged => formatter.write_str("0"),
            Self::Increase(bytes) => write!(formatter, "+{bytes}"),
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
