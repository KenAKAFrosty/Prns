use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::results::Subject;
use serde::{Deserialize, Serialize};

pub const REFERENCE_VERSION: &str = "1.4.2";
pub const REFERENCE_IMPLEMENTATION: &str = "rns-1.4.2-compiled";
pub const IMPLEMENTATIONS: [&str; 2] = ["personal-rns", REFERENCE_IMPLEMENTATION];
pub const KNOWN_IMPLEMENTATIONS: [&str; 3] = [
    "personal-rns",
    "rns-1.4.0-compiled",
    REFERENCE_IMPLEMENTATION,
];
const STANDARD_ENCRYPTED_LINK_MDU: usize = 383;
const STOCK_REQUEST_ENVELOPE_BUDGET: usize = 64;
const LARGE_RESOURCE_SEGMENTS: usize = 64;
const LARGE_RESOURCE_PAYLOAD_BYTES: usize =
    personal_rns::routing::links::resources::MAX_EFFICIENT_SIZE * LARGE_RESOURCE_SEGMENTS;
pub const DEFAULT_SIZE_SEED: u64 = 0x5EED_CAFE_F00D_0001;

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum ScenarioId {
        SinglePacketThroughput,
        LinkMessageThroughput,
        RequestResponse,
        ResourceMaxSegment,
        ResourceMaxSegmentUnleashed,
        #[serde(rename = "resource-64mib-stream")]
        Resource64mibStream,
        #[serde(rename = "resource-64mib-stream-unleashed")]
        Resource64mibStreamUnleashed,
        RawTransportThroughput,
        TransportResourceThroughput,
        TransportResourceThroughputUnleashed,
    }
}

impl ScenarioId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SinglePacketThroughput => "single-packet-throughput",
            Self::LinkMessageThroughput => "link-message-throughput",
            Self::RequestResponse => "request-response",
            Self::ResourceMaxSegment => "resource-max-segment",
            Self::ResourceMaxSegmentUnleashed => "resource-max-segment-unleashed",
            Self::Resource64mibStream => "resource-64mib-stream",
            Self::Resource64mibStreamUnleashed => "resource-64mib-stream-unleashed",
            Self::RawTransportThroughput => "raw-transport-throughput",
            Self::TransportResourceThroughput => "transport-resource-throughput",
            Self::TransportResourceThroughputUnleashed => "transport-resource-throughput-unleashed",
        }
    }

    pub const fn is_transport(self) -> bool {
        matches!(
            self,
            Self::RawTransportThroughput
                | Self::TransportResourceThroughput
                | Self::TransportResourceThroughputUnleashed
        )
    }

    pub const fn is_transport_resource(self) -> bool {
        matches!(
            self,
            Self::TransportResourceThroughput | Self::TransportResourceThroughputUnleashed
        )
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
    ExactTransport,
    ExactTransportResource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioTopology {
    #[default]
    Direct,
    Relay,
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
    #[serde(default)]
    pub cell_notes: Vec<ScenarioCellNote>,
    pub description: String,
    pub roles: Vec<String>,
    #[serde(default)]
    pub topology: ScenarioTopology,
    pub profile: WorkloadProfile,
    pub conformance_rule: ConformanceRule,
    pub conformance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioCellNote {
    pub subject: Subject,
    pub text: String,
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
    #[serde(default)]
    pub transport_link_mtu: usize,
    #[serde(default)]
    pub tcp_bitrate_bps: Option<u64>,
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
    if orders != (1..=ScenarioId::ALL.len() as u32).collect::<Vec<_>>() {
        return Err(CatalogError::Invalid(format!(
            "scenario order must be contiguous from one, found {orders:?}"
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
            "expected exactly {} scenario directories, found {directories}",
            ScenarioId::ALL.len()
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
        ScenarioId::ResourceMaxSegment
        | ScenarioId::ResourceMaxSegmentUnleashed
        | ScenarioId::Resource64mibStream
        | ScenarioId::Resource64mibStreamUnleashed => "resource",
        ScenarioId::RawTransportThroughput => "transport",
        ScenarioId::TransportResourceThroughput
        | ScenarioId::TransportResourceThroughputUnleashed => "transport-resource",
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
        ScenarioId::ResourceMaxSegment
        | ScenarioId::ResourceMaxSegmentUnleashed
        | ScenarioId::Resource64mibStream
        | ScenarioId::Resource64mibStreamUnleashed => ConformanceRule::ExactResource,
        ScenarioId::RawTransportThroughput => ConformanceRule::ExactTransport,
        ScenarioId::TransportResourceThroughput
        | ScenarioId::TransportResourceThroughputUnleashed => {
            ConformanceRule::ExactTransportResource
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
    if matches!(
        manifest.name,
        ScenarioId::Resource64mibStream | ScenarioId::Resource64mibStreamUnleashed
    ) && manifest.profile.payload_len != LARGE_RESOURCE_PAYLOAD_BYTES
    {
        return Err(CatalogError::Invalid(format!(
            "{} must carry exactly {LARGE_RESOURCE_SEGMENTS} maximum-efficient resource segments",
            manifest.name
        )));
    }
    let expected_topology = if manifest.name.is_transport() {
        ScenarioTopology::Relay
    } else {
        ScenarioTopology::Direct
    };
    if manifest.topology != expected_topology {
        return Err(CatalogError::Invalid(format!(
            "{} must use topology {expected_topology:?}",
            manifest.name
        )));
    }
    if manifest.name.is_transport() && manifest.roles != ["wire-driver", "relay"] {
        return Err(CatalogError::Invalid(format!(
            "{} must declare wire-driver and relay roles",
            manifest.name
        )));
    }
    let mut annotated_subjects = BTreeSet::new();
    for note in &manifest.cell_notes {
        if note.text.trim().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "{} has an empty cell note",
                manifest.name
            )));
        }
        if !annotated_subjects.insert(note.subject.file_slug()) {
            return Err(CatalogError::Invalid(format!(
                "{} annotates subject {} more than once",
                manifest.name,
                note.subject.file_slug()
            )));
        }
        let valid_subject = match (&note.subject, manifest.topology) {
            (
                Subject::Direct {
                    initiator,
                    responder,
                    relay: None,
                },
                ScenarioTopology::Direct,
            ) => {
                KNOWN_IMPLEMENTATIONS.contains(&initiator.as_str())
                    && KNOWN_IMPLEMENTATIONS.contains(&responder.as_str())
            }
            (
                Subject::Direct {
                    initiator,
                    responder,
                    relay: Some(relay),
                },
                ScenarioTopology::Relay,
            ) => {
                initiator == "benchmark-wire-driver"
                    && responder == "benchmark-wire-driver"
                    && KNOWN_IMPLEMENTATIONS.contains(&relay.as_str())
            }
            _ => false,
        };
        if !valid_subject {
            return Err(CatalogError::Invalid(format!(
                "{} has a cell note for subject {} outside its {:?} topology",
                manifest.name,
                note.subject.file_slug(),
                manifest.topology
            )));
        }
    }
    if manifest.name == ScenarioId::RawTransportThroughput
        && (manifest.profile.window != 256
            || manifest.profile.payload_min != 60
            || manifest.profile.payload_max != 420
            || manifest.profile.duration_ms != 30_000
            || manifest.profile.drain_timeout_ms != 30_000
            || manifest.profile.size_seed != DEFAULT_SIZE_SEED
            || manifest.profile.link_mtu != 0)
    {
        return Err(CatalogError::Invalid(format!(
            "{} fixes a 30-second issue/drain profile, 256-frame directional windows, the shared seed, and 60–420-byte payloads without a fixed MTU",
            manifest.name
        )));
    }
    if manifest.name.is_transport_resource()
        && (manifest.profile.window != 16
            || manifest.profile.payload_len != 0
            || manifest.profile.payload_min != 0
            || manifest.profile.payload_max != 0
            || manifest.profile.duration_ms != 30_000
            || manifest.profile.drain_timeout_ms != 30_000
            || manifest.profile.size_seed != DEFAULT_SIZE_SEED
            || manifest.profile.link_mtu != 0
            || manifest.profile.transport_link_mtu != 524_288
            || manifest.profile.payload_shape != "effective-mtu")
    {
        return Err(CatalogError::Invalid(format!(
            "{} fixes a 30-second issue/drain profile, 16-frame directional windows, the shared seed, and a maximum requested transported-link MTU",
            manifest.name
        )));
    }
    if !manifest.name.is_transport_resource() && manifest.profile.transport_link_mtu != 0 {
        return Err(CatalogError::Invalid(format!(
            "{} does not own a transported-link MTU request",
            manifest.name
        )));
    }
    match manifest.name {
        ScenarioId::RawTransportThroughput
        | ScenarioId::ResourceMaxSegment
        | ScenarioId::Resource64mibStream
        | ScenarioId::TransportResourceThroughput => {
            if manifest.profile.tcp_bitrate_bps.is_some() {
                return Err(CatalogError::Invalid(format!(
                    "{} must preserve each implementation's default TCP bitrate policy",
                    manifest.name
                )));
            }
        }
        ScenarioId::ResourceMaxSegmentUnleashed
        | ScenarioId::Resource64mibStreamUnleashed
        | ScenarioId::TransportResourceThroughputUnleashed => {
            if manifest.profile.tcp_bitrate_bps != Some(1_000_000_000) {
                return Err(CatalogError::Invalid(format!(
                    "{} must explicitly configure a 1 Gbps TCP bitrate policy",
                    manifest.name
                )));
            }
        }
        ScenarioId::SinglePacketThroughput
        | ScenarioId::LinkMessageThroughput
        | ScenarioId::RequestResponse => {
            if manifest.profile.tcp_bitrate_bps.is_some() {
                return Err(CatalogError::Invalid(format!(
                    "{} does not own a TCP bitrate override",
                    manifest.name
                )));
            }
        }
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
    fn cell_notes_are_typed_unique_subject_annotations() {
        let mut manifest = load_manifest(ScenarioId::RequestResponse).expect("annotated manifest");
        assert_eq!(manifest.cell_notes.len(), 2);
        assert_eq!(
            manifest
                .cell_notes
                .iter()
                .map(|note| note.subject.clone())
                .collect::<Vec<_>>(),
            ["rns-1.4.0-compiled", REFERENCE_IMPLEMENTATION]
                .into_iter()
                .map(|responder| Subject::Direct {
                    initiator: "personal-rns".into(),
                    responder: responder.into(),
                    relay: None,
                })
                .collect::<Vec<_>>()
        );

        manifest.cell_notes.push(manifest.cell_notes[0].clone());
        let path = scenarios_dir()
            .join(manifest.name.as_str())
            .join("manifest.json");
        let error = validate_manifest(&manifest, &path).expect_err("duplicate cell note");
        assert!(
            error.to_string().contains("more than once"),
            "unexpected validation error: {error}"
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
        let block_len = golden["resource_repeat_block_len"]
            .as_u64()
            .expect("resource block length") as usize;
        let stream_len = golden["resource_repeat_len"]
            .as_u64()
            .expect("resource stream length") as usize;
        let block = deterministic_payload(block_len);
        let resource_stream = block
            .iter()
            .copied()
            .cycle()
            .take(stream_len)
            .collect::<Vec<_>>();
        assert_eq!(
            resource_stream
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            golden["resource_repeat_hex"]
                .as_str()
                .expect("resource stream vector")
        );
    }

    #[test]
    fn large_resource_stream_is_exactly_sixty_four_full_segments() {
        for scenario in [
            ScenarioId::Resource64mibStream,
            ScenarioId::Resource64mibStreamUnleashed,
        ] {
            let manifest = load_manifest(scenario).expect("valid large-resource manifest");
            assert_eq!(manifest.profile.payload_len, LARGE_RESOURCE_PAYLOAD_BYTES);
        }
    }
}
