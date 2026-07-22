use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub const IMPLEMENTATIONS: [&str; 2] = ["personal-rns", "rns-1.4.0-compiled"];
const STANDARD_ENCRYPTED_LINK_MDU: usize = 383;
const STOCK_REQUEST_ENVELOPE_BUDGET: usize = 64;
pub const DEFAULT_SIZE_SEED: u64 = 0x5EED_CAFE_F00D_0001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioId {
    SinglePacketThroughput,
    LinkMessageThroughput,
    RequestResponse,
    ResourceMaxSegment,
    #[serde(rename = "resource-64mib-stream")]
    Resource64mibStream,
}

impl ScenarioId {
    pub const ALL: [Self; 5] = [
        Self::SinglePacketThroughput,
        Self::LinkMessageThroughput,
        Self::RequestResponse,
        Self::ResourceMaxSegment,
        Self::Resource64mibStream,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SinglePacketThroughput => "single-packet-throughput",
            Self::LinkMessageThroughput => "link-message-throughput",
            Self::RequestResponse => "request-response",
            Self::ResourceMaxSegment => "resource-max-segment",
            Self::Resource64mibStream => "resource-64mib-stream",
        }
    }
}

impl fmt::Display for ScenarioId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ScenarioId {
    type Err = CatalogError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.as_str() == value)
            .ok_or_else(|| CatalogError::Invalid(format!("unknown benchmark scenario {value:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceRule {
    ExactSingle,
    ExactLink,
    ExactRequest,
    ExactResource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioManifest {
    pub name: ScenarioId,
    pub version: u32,
    pub order: u32,
    pub title: String,
    pub category: String,
    pub summary: String,
    pub primary_metric: String,
    pub headline: bool,
    #[serde(default)]
    pub notes: Vec<String>,
    pub description: String,
    pub roles: Vec<String>,
    pub profile: WorkloadProfile,
    pub conformance_rule: ConformanceRule,
    pub conformance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadProfile {
    pub mechanism: String,
    #[serde(default)]
    pub payload_len: usize,
    #[serde(default)]
    pub payload_min: usize,
    #[serde(default)]
    pub payload_max: usize,
    #[serde(default)]
    pub request_min: usize,
    #[serde(default)]
    pub request_max: usize,
    #[serde(default)]
    pub response_min: usize,
    #[serde(default)]
    pub response_max: usize,
    pub window: usize,
    #[serde(default)]
    pub request_links: usize,
    #[serde(default)]
    pub link_mtu: usize,
    pub duration_ms: u64,
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
    #[serde(default = "default_announce_every_ms")]
    pub announce_every_ms: u64,
    #[serde(default = "default_initiator_count")]
    pub initiator_count: usize,
    #[serde(default = "default_size_seed")]
    pub size_seed: u64,
    #[serde(default = "default_compression")]
    pub compression: String,
    #[serde(default = "default_payload_shape")]
    pub payload_shape: String,
}

const fn default_announce_every_ms() -> u64 {
    500
}

const fn default_initiator_count() -> usize {
    1
}

const fn default_drain_timeout_ms() -> u64 {
    30_000
}

const fn default_size_seed() -> u64 {
    DEFAULT_SIZE_SEED
}

fn default_compression() -> String {
    "off".into()
}

fn default_payload_shape() -> String {
    "dense".into()
}

#[derive(Debug)]
pub enum CatalogError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    Invalid(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(formatter, "read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(formatter, "parse {}: {source}", path.display()),
            Self::Invalid(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for CatalogError {}

pub fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

pub fn load_manifest(id: ScenarioId) -> Result<ScenarioManifest, CatalogError> {
    let path = scenarios_dir().join(id.as_str()).join("manifest.json");
    let body = std::fs::read_to_string(&path).map_err(|source| CatalogError::Read {
        path: path.clone(),
        source,
    })?;
    let manifest = serde_json::from_str(&body).map_err(|source| CatalogError::Parse {
        path: path.clone(),
        source,
    })?;
    validate_manifest(&manifest, &path)?;
    Ok(manifest)
}

pub fn load_catalog() -> Result<Vec<ScenarioManifest>, CatalogError> {
    let mut manifests = ScenarioId::ALL
        .into_iter()
        .map(load_manifest)
        .collect::<Result<Vec<_>, _>>()?;
    manifests.sort_by_key(|manifest| manifest.order);
    let orders = manifests
        .iter()
        .map(|manifest| manifest.order)
        .collect::<Vec<_>>();
    if orders != vec![1, 2, 3, 4, 5] {
        return Err(CatalogError::Invalid(format!(
            "scenario order must be exactly 1..=5, found {orders:?}"
        )));
    }
    let directories = std::fs::read_dir(scenarios_dir())
        .map_err(|source| CatalogError::Read {
            path: scenarios_dir(),
            source,
        })?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .count();
    if directories != ScenarioId::ALL.len() {
        return Err(CatalogError::Invalid(format!(
            "expected exactly five scenario directories, found {directories}"
        )));
    }
    Ok(manifests)
}

fn validate_manifest(manifest: &ScenarioManifest, path: &Path) -> Result<(), CatalogError> {
    let directory = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if directory != Some(manifest.name.as_str()) {
        return Err(CatalogError::Invalid(format!(
            "{} names {}, but its directory is {:?}",
            path.display(),
            manifest.name,
            directory
        )));
    }
    if manifest.version == 0 || manifest.order == 0 || manifest.profile.window == 0 {
        return Err(CatalogError::Invalid(format!(
            "{} has a zero version, order, or window",
            path.display()
        )));
    }
    if manifest.profile.duration_ms == 0 || manifest.profile.size_seed == 0 {
        return Err(CatalogError::Invalid(format!(
            "{} has a zero duration or deterministic seed",
            path.display()
        )));
    }
    let expected_mechanism = match manifest.name {
        ScenarioId::SinglePacketThroughput => "single",
        ScenarioId::LinkMessageThroughput => "link",
        ScenarioId::RequestResponse => "request",
        ScenarioId::ResourceMaxSegment | ScenarioId::Resource64mibStream => "resource",
    };
    if manifest.profile.mechanism != expected_mechanism {
        return Err(CatalogError::Invalid(format!(
            "{} must use mechanism {expected_mechanism}",
            manifest.name
        )));
    }
    let expected_conformance = match manifest.name {
        ScenarioId::SinglePacketThroughput => ConformanceRule::ExactSingle,
        ScenarioId::LinkMessageThroughput => ConformanceRule::ExactLink,
        ScenarioId::RequestResponse => ConformanceRule::ExactRequest,
        ScenarioId::ResourceMaxSegment | ScenarioId::Resource64mibStream => {
            ConformanceRule::ExactResource
        }
    };
    if manifest.conformance_rule != expected_conformance {
        return Err(CatalogError::Invalid(format!(
            "{} must use conformance rule {expected_conformance:?}",
            manifest.name
        )));
    }
    if manifest.name == ScenarioId::RequestResponse
        && manifest.profile.request_link_count() < manifest.profile.window
    {
        return Err(CatalogError::Invalid(format!(
            "{} needs at least one request link per in-flight operation",
            manifest.name
        )));
    }
    if manifest.name == ScenarioId::RequestResponse && manifest.profile.link_mtu != 500 {
        return Err(CatalogError::Invalid(format!(
            "{} must fix the RNS link MTU at 500 bytes so small requests stay packets and 1–4 KiB responses are resources",
            manifest.name
        )));
    }
    if manifest.name == ScenarioId::RequestResponse
        && (manifest.profile.request_max + STOCK_REQUEST_ENVELOPE_BUDGET
            > STANDARD_ENCRYPTED_LINK_MDU
            || manifest.profile.response_min <= STANDARD_ENCRYPTED_LINK_MDU)
    {
        return Err(CatalogError::Invalid(format!(
            "{} must keep its request envelope below the 383-byte encrypted MDU and every response above it",
            manifest.name
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SizeSequence {
    state: u64,
    min: usize,
    max: usize,
}

impl SizeSequence {
    pub fn new(seed: u64, min: usize, max: usize, fixed: usize) -> Self {
        let (min, max) = if max > 0 { (min, max) } else { (fixed, fixed) };
        Self {
            state: seed,
            min,
            max,
        }
    }

    pub fn next_len(&mut self) -> usize {
        self.next_in(self.min, self.max)
    }

    pub fn next_in(&mut self, min: usize, max: usize) -> usize {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let span = (max - min + 1) as u64;
        min + (self.state % span) as usize
    }
}

pub fn deterministic_payload(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

impl WorkloadProfile {
    pub fn request_link_count(&self) -> usize {
        if self.request_links == 0 {
            self.window
        } else {
            self.request_links
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_complete_and_ordered() {
        let catalog = load_catalog().expect("valid benchmark catalog");
        assert_eq!(
            catalog
                .iter()
                .map(|manifest| manifest.name)
                .collect::<Vec<_>>(),
            ScenarioId::ALL
        );
    }

    #[test]
    fn deterministic_workload_vector_is_stable() {
        let golden: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(scenarios_dir().join("workload-vectors.json"))
                .expect("shared workload golden"),
        )
        .expect("valid workload golden");
        let mut sizes = SizeSequence::new(DEFAULT_SIZE_SEED, 16, 300, 16);
        let expected_sizes = golden["sizes"]
            .as_array()
            .expect("size vector")
            .iter()
            .map(|value| value.as_u64().expect("size") as usize)
            .collect::<Vec<_>>();
        assert_eq!(
            (0..8).map(|_| sizes.next_len()).collect::<Vec<_>>(),
            expected_sizes
        );
        assert_eq!(
            deterministic_payload(16)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            golden["payload_hex"].as_str().expect("payload vector")
        );
    }
}
