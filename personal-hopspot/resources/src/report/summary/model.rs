use std::fmt;

use serde::{Deserialize, Serialize};

pub(super) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MatrixSummary {
    pub(super) schema_version: u32,
    pub(super) report_schema_version: u32,
    pub(super) baseline_schema_version: u32,
    pub(super) targets: Vec<TargetSummary>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetSummary {
    pub(super) id: String,
    pub(super) display_name: String,
    pub(super) memory_profile: String,
    pub(super) rust_target: String,
    pub(super) linker_flavor: String,
    pub(super) build_fingerprint: String,
    pub(super) toolchain: ToolchainSummary,
    pub(super) flash_image: ByteMetric,
    pub(super) flash_headroom: ByteMetric,
    pub(super) static_ram: ByteMetric,
    pub(super) known_ram_used: ByteMetric,
    pub(super) known_ram_headroom: ByteMetric,
    pub(super) runtime_detected_ram: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolchainSummary {
    pub(super) baseline_fingerprint: String,
    pub(super) current_fingerprint: String,
    pub(super) relation: ToolchainRelation,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ToolchainRelation {
    Exact,
    Different,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ByteMetric {
    pub(super) baseline_bytes: u64,
    pub(super) current_bytes: u64,
    pub(super) delta: ByteDelta,
}

impl ByteMetric {
    pub(super) const fn new(baseline_bytes: u64, current_bytes: u64) -> Self {
        let delta = if baseline_bytes < current_bytes {
            ByteDelta::Increase(current_bytes - baseline_bytes)
        } else if baseline_bytes == current_bytes {
            ByteDelta::Unchanged
        } else {
            ByteDelta::Decrease(baseline_bytes - current_bytes)
        };
        Self {
            baseline_bytes,
            current_bytes,
            delta,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "direction", content = "bytes", rename_all = "kebab-case")]
pub(super) enum ByteDelta {
    Decrease(u64),
    Unchanged,
    Increase(u64),
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

impl fmt::Display for ToolchainRelation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Exact => "exact",
            Self::Different => "different",
        })
    }
}
