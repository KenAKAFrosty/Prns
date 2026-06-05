//! Render `benchmarks/RESULTS.md` from the result substrate (`results/*/*.jsonl`).
//! `--check` re-renders and diffs against the committed file — the drift gate, mirroring
//! `gen_corpus --check`. RESULTS.md is generated, never hand-edited, and the website
//! includes the *same* file, so the table can't drift between GitHub and the site.
//!
//! Run: `cargo run --release --bin render_results [--check]`

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use benchmarks::{load_all_rows, scenario_dir, Axis, ResultRow};

const PERSONAL_RNS: &str = "personal-rns";

fn main() {
    let md = render();
    let path = results_md_path();
    if std::env::args().any(|a| a == "--check") {
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed == md {
            println!("results table: IN SYNC with the substrate");
            return;
        }
        eprintln!("results table: STALE — run `cargo run --bin render_results` and commit RESULTS.md");
        std::process::exit(1);
    }
    std::fs::write(&path, md).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    eprintln!("wrote {}", path.display());
}

fn results_md_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("RESULTS.md")
}

fn render() -> String {
    let mut by_scenario: BTreeMap<String, Vec<ResultRow>> = BTreeMap::new();
    for row in load_all_rows() {
        by_scenario.entry(row.scenario.clone()).or_default().push(row);
    }

    let mut out = String::new();
    out.push_str(HEADER);
    if by_scenario.is_empty() {
        out.push_str("\n_No results recorded yet — run the drivers, then `render_results`._\n");
        return out;
    }
    for (scenario, rows) in &by_scenario {
        render_scenario(&mut out, scenario, rows);
    }
    out.push_str(FOOTNOTES);
    out
}

fn render_scenario(out: &mut String, scenario: &str, rows: &[ResultRow]) {
    let manifest = Manifest::load(scenario);
    let impls = impl_columns(rows);
    let axes = axes_present(rows);

    let _ = write!(out, "\n## {scenario} (v{})\n\n", manifest.version);
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "{}\n", manifest.description);
    }

    let _ = write!(out, "| Axis | Scope |");
    for name in &impls {
        let _ = write!(out, " {name} |");
    }
    out.push_str("\n|------|-------|");
    for _ in &impls {
        out.push_str("------|");
    }
    out.push('\n');

    for axis in axes {
        let _ = write!(out, "| {} | {} |", axis.title(), axis.comparability().label());
        for name in &impls {
            let row = rows
                .iter()
                .find(|r| &r.implementation == name && r.axis == axis);
            let _ = write!(out, " {} |", cell(row, axis, manifest.expected_routes));
        }
        out.push('\n');
    }

    out.push('\n');
    for name in &impls {
        if let Some(row) = rows.iter().find(|r| &r.implementation == name) {
            let _ = writeln!(
                out,
                "- **{name}** — {}, {}, {}",
                row.commit, row.toolchain, row.host
            );
        }
    }
}

fn cell(row: Option<&ResultRow>, axis: Axis, expected_routes: u64) -> String {
    let Some(row) = row else {
        return "—".to_string();
    };
    match axis {
        Axis::Conformance => {
            let got = row.value as u64;
            let mark = if got == expected_routes { "✅" } else { "❌" };
            format!("{mark} {got} / {expected_routes}")
        }
        _ => format!("{} {}", humanize(row.value), row.unit),
    }
}

/// Implementation columns, personal-rns first, then the rest alphabetically.
fn impl_columns(rows: &[ResultRow]) -> Vec<String> {
    let mut names: Vec<String> = rows.iter().map(|r| r.implementation.clone()).collect();
    names.sort();
    names.dedup();
    names.sort_by_key(|name| (name != PERSONAL_RNS, name.clone()));
    names
}

/// Axes present in the rows, in canonical display order.
fn axes_present(rows: &[ResultRow]) -> Vec<Axis> {
    let mut axes: Vec<Axis> = rows.iter().map(|r| r.axis).collect();
    axes.sort_by_key(|a| a.order());
    axes.dedup();
    axes
}

fn humanize(v: f64) -> String {
    if v >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

struct Manifest {
    version: u64,
    description: String,
    expected_routes: u64,
}

impl Manifest {
    fn load(scenario: &str) -> Self {
        let path = scenario_dir(scenario).join("manifest.json");
        let json: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(serde_json::Value::Null);
        Manifest {
            version: json["version"].as_u64().unwrap_or(0),
            description: json["description"].as_str().unwrap_or("").to_string(),
            expected_routes: json["expected"]["route_count"].as_u64().unwrap_or(0),
        }
    }
}

const HEADER: &str = "<!-- Generated by `cargo run --bin render_results` from benchmarks/results/. Do not edit by hand. -->

# Benchmark results

Each figure is stamped with the commit, toolchain, and host that produced it and lives as a
row in `results/<scenario>/<impl>.jsonl` — the same schema any implementation emits. This file
is rendered from those rows, and the website renders this same file, so the two never drift.

**Comparability.** Conformance and throughput line up across implementations; memory and latency
stay within one (a GC and a no-alloc core racing on RSS would be a dishonest column). Only
`cross-impl` rows are a fair head-to-head.
";

const FOOTNOTES: &str = "
---

- _Conformance_ — distinct routes the engine resolves from the corpus, against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to ingest the whole corpus into a fresh engine, as announces per second.

Regenerate: run each implementation's driver (`bench_result`, `reference/driver.py`) to refresh
`results/`, then `render_results` to rewrite this table.
";
