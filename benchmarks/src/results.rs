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
            Axis::Conformance | Axis::Throughput | Axis::BinarySize => Comparability::CrossImpl,
            Axis::Memory | Axis::Latency => Comparability::WithinImpl,
        }
    }

    /// Canonical display order in the table (and a stable sort key for `--check`).
    pub fn order(self) -> u8 {
        match self {
            Axis::Conformance => 0,
            Axis::Throughput => 1,
            Axis::Latency => 2,
            Axis::Memory => 3,
            Axis::BinarySize => 4,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Axis::Conformance => "Conformance",
            Axis::Throughput => "Ingest throughput",
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

/// One measured figure, in the schema every implementation's driver emits.
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
    pub value: f64,
    pub unit: String,
}

/// The substrate root: `<crate>/results`.
pub fn results_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("results")
}

/// Write all of one implementation's rows for one scenario to
/// `results/<scenario>/<impl-slug>.jsonl`, overwriting. Each `(scenario, impl)` file
/// is owned by exactly one driver, so a plain overwrite is the whole story — no merge.
pub fn write_rows(scenario: &str, impl_slug: &str, rows: &[ResultRow]) {
    let dir = results_dir().join(scenario);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let mut body = String::new();
    for row in rows {
        body.push_str(&serde_json::to_string(row).expect("serialize result row"));
        body.push('\n');
    }
    let path = dir.join(format!("{impl_slug}.jsonl"));
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Every committed row across all scenarios and implementations (`results/*/*.jsonl`).
pub fn load_all_rows() -> Vec<ResultRow> {
    let root = results_dir();
    let mut rows = Vec::new();
    let Ok(scenarios) = std::fs::read_dir(&root) else {
        return rows;
    };
    for scenario in scenarios.flatten() {
        if !scenario.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(scenario.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let row: ResultRow = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("parse row in {}: {e}", path.display()));
                rows.push(row);
            }
        }
    }
    rows
}
