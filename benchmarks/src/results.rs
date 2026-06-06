//! The cross-implementation **result substrate**: one JSON object per measured figure,
//! in the schema every participating implementation emits (see CONTRIBUTING). Drivers
//! write their rows here; `render_results` is the only reader, turning them into the
//! human `RESULTS.md` table. Keeping the figures as *data* — not prose — is what lets
//! the GitHub `RESULTS.md` and the website render the same numbers with zero drift.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a figure measures. The variant decides whether it compares honestly *across*
/// implementations or only within one.
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

/// Whether figures on an axis can be lined up between implementations, or only read
/// within one — a GC and a no-alloc core racing on RSS would be a dishonest column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparability {
    CrossImpl,
    WithinImpl,
}

impl Axis {
    pub fn comparability(self) -> Comparability {
        match self {
            Axis::Conformance | Axis::Throughput | Axis::Power | Axis::Energy | Axis::BinarySize => {
                Comparability::CrossImpl
            }
            Axis::Memory | Axis::Latency => Comparability::WithinImpl,
        }
    }

    /// Canonical display order in the table (and a stable sort key for `--check`).
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

/// One measured figure, in the schema every implementation's driver emits. `value` is
/// `None` for a host that's been scaffolded but not yet measured (renders as *pending*).
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
    /// Worker threads the figure was measured with, for a scenario that sweeps
    /// parallelism (`announce-parallel`). Absent on single-threaded scenarios, so
    /// their rows serialize unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
}

/// The machine a host's figures were measured on. A `host` (rustc target triple) is the
/// substrate's grouping key, but the triple alone can't reproduce a throughput number —
/// an M1 and an M4 Max are both `aarch64-apple-darwin`. This descriptor pins the silicon
/// it actually ran on. Exactly one per host (`results/<host>/host.json`), written by
/// `describe_host` (not the figure drivers), so the spec isn't duplicated onto every row.
/// Fields are `Option` so a scaffolded-but-unmeasured host renders as *pending*.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostDescriptor {
    pub host: String,
    pub cpu_model: Option<String>,
    pub physical_cores: Option<u32>,
    pub logical_cores: Option<u32>,
    pub total_memory_bytes: Option<u64>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
}

/// The substrate root: `<crate>/results`.
pub fn results_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("results")
}

/// The descriptor path for one host: `results/<host>/host.json`.
fn host_path(host: &str) -> PathBuf {
    results_dir().join(host).join("host.json")
}

/// Write a host's machine descriptor to `results/<host>/host.json`, overwriting. Owned by
/// `describe_host` (run once per machine), beside that host's figure rows.
pub fn write_host(descriptor: &HostDescriptor) {
    let path = host_path(&descriptor.host);
    std::fs::create_dir_all(path.parent().expect("host dir")).expect("create host dir");
    let body = serde_json::to_string_pretty(descriptor).expect("serialize host descriptor");
    std::fs::write(&path, body + "\n")
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Load a host's machine descriptor, if one has been written (`None` when a host has figure
/// rows but `describe_host` hasn't run there yet).
pub fn load_host(host: &str) -> Option<HostDescriptor> {
    let text = std::fs::read_to_string(host_path(host)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Where an implementation sits in the comparison: the Python reference everything
/// is measured against, our own engine, or one of the external ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImplementationRole {
    Reference,
    Ours,
    External,
}

/// Host-independent facts about a participating implementation: its language, its
/// Ed25519 backend, and where its source lives (repo + pinned ref + license). The
/// throughput *value* is per-host (a [`ResultRow`]), but what an implementation *is*
/// is the same on every machine — so it lives once per implementation in
/// `implementations/<slug>.json`, never duplicated onto every row (the same call we
/// made for [`HostDescriptor`]). Drives the comparison table's Language/backend
/// columns and the provenance list. `maturity` is `Some("partial")` for an
/// implementation the upstream list marks not-yet-feature-complete; `None` is the
/// feature-complete default.
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
}

/// The implementation registry: `<crate>/implementations`.
fn implementations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("implementations")
}

/// Every implementation descriptor (`implementations/<slug>.json`), keyed in the
/// returned vec by file. Missing dir or unparseable files are skipped — a row whose
/// implementation has no descriptor still renders, just without language/backend.
pub fn load_implementations() -> Vec<ImplementationDescriptor> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(implementations_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(descriptor) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            out.push(descriptor);
        }
    }
    out
}

/// Write all of one implementation's rows for one `(host, scenario)` to
/// `results/<host>/<scenario>/<impl-slug>.jsonl`, overwriting. Each
/// `(host, scenario, impl)` file is owned by exactly one driver run, so a plain
/// overwrite is the whole story — no merge.
pub fn write_rows(host: &str, scenario: &str, impl_slug: &str, rows: &[ResultRow]) {
    let dir = results_dir().join(host).join(scenario);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let mut body = String::new();
    for row in rows {
        body.push_str(&serde_json::to_string(row).expect("serialize result row"));
        body.push('\n');
    }
    let path = dir.join(format!("{impl_slug}.jsonl"));
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Every committed row, across all hosts, scenarios, and implementations
/// (`results/<host>/<scenario>/<impl>.jsonl`).
pub fn load_all_rows() -> Vec<ResultRow> {
    let mut rows = Vec::new();
    for jsonl in jsonl_files(&results_dir()) {
        let text = std::fs::read_to_string(&jsonl)
            .unwrap_or_else(|e| panic!("read {}: {e}", jsonl.display()));
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let row: ResultRow = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("parse row in {}: {e}", jsonl.display()));
            rows.push(row);
        }
    }
    rows
}

/// Every `.jsonl` under `root`, recursively (host/scenario/impl nesting).
fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(jsonl_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out
}
