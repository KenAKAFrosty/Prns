use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub Uuid);

impl DeviceId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubmitterId(pub Uuid);

impl SubmitterId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    Conformance,
    Throughput,
    Power,
    Energy,
    Memory,
    BinarySize,
    Latency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparability {
    CrossImpl,
    WithinImpl,
}

impl Axis {
    pub fn comparability(self) -> Comparability {
        match self {
            Axis::Conformance
            | Axis::Throughput
            | Axis::Power
            | Axis::Energy
            | Axis::BinarySize => Comparability::CrossImpl,
            Axis::Memory | Axis::Latency => Comparability::WithinImpl,
        }
    }

    pub fn order(self) -> u8 {
        match self {
            Axis::Conformance => 0,
            Axis::Throughput => 1,
            Axis::Power => 2,
            Axis::Energy => 3,
            Axis::Latency => 4,
            Axis::Memory => 5,
            Axis::BinarySize => 6,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Axis::Conformance => "Conformance",
            Axis::Throughput => "Ingest throughput",
            Axis::Power => "CPU power",
            Axis::Energy => "Energy",
            Axis::Memory => "Memory",
            Axis::BinarySize => "Binary size",
            Axis::Latency => "Latency",
        }
    }
}

impl Comparability {
    pub fn label(self) -> &'static str {
        match self {
            Comparability::CrossImpl => "cross-impl",
            Comparability::WithinImpl => "within-impl",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    pub scenario: String,
    pub scenario_version: u32,
    pub implementation: String,
    pub commit: String,
    pub toolchain: String,
    pub host: String,
    pub axis: Axis,
    pub metric: String,
    pub value: Option<f64>,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitter_id: Option<SubmitterId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostDescriptor {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
    pub cpu_model: Option<String>,
    pub physical_cores: Option<u32>,
    pub logical_cores: Option<u32>,
    pub total_memory_bytes: Option<u64>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_governor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_max_mhz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency_cores: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImplementationRole {
    Reference,
    Ours,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationDescriptor {
    pub implementation: String,
    pub slug: String,
    pub language: String,
    pub crypto_backend: String,
    pub role: ImplementationRole,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub pinned_ref: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub maturity: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}
