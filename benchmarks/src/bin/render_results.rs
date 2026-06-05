//! Render the benchmark tables from the result substrate (`results/<host>/<scenario>/
//! <impl>.jsonl`). Each host gets its own `RESULTS-<host>.md`; `RESULTS.md` is the index
//! that links to them (md->md links that resolve on GitHub). The website renders the same
//! files, so the tables can't drift between GitHub and the site.
//!
//! `--check` re-renders every file and diffs against what's committed — the drift gate,
//! mirroring `gen_corpus --check`. Generated, never hand-edited.
//!
//! Run: `cargo run --release --bin render_results [--check]`

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use benchmarks::{load_all_rows, load_host, scenario_dir, Axis, ResultRow};

const PERSONAL_RNS: &str = "personal-rns";

// Committed SVGs (benchmarks/assets/, mirrored to the site's public/assets/) — GitHub
// strips inline <svg> but renders <img> to a sanitized SVG file, and the same relative
// src resolves to /assets/ on the site (the /benchmarks page has no trailing slash).
const PASS_ICON: &str = r#"<img src="assets/check.svg" width="14" alt="conformant" />"#;
const FAIL_ICON: &str = r#"<img src="assets/cross.svg" width="14" alt="non-conformant" />"#;

fn main() {
    let files = render_all();
    if std::env::args().any(|a| a == "--check") {
        let stale: Vec<&PathBuf> = files
            .iter()
            .filter(|(path, body)| std::fs::read_to_string(path).unwrap_or_default() != *body)
            .map(|(path, _)| path)
            .collect();
        if stale.is_empty() {
            println!("results tables: IN SYNC with the substrate ({} files)", files.len());
            return;
        }
        eprintln!("results tables: STALE — run `cargo run --bin render_results` and commit:");
        for path in stale {
            eprintln!("  {}", path.display());
        }
        std::process::exit(1);
    }
    for (path, body) in &files {
        std::fs::write(path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }
    eprintln!("wrote {} result files to {}", files.len(), bench_dir().display());
}

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The index (`RESULTS.md`) plus one `RESULTS-<host>.md` per host.
fn render_all() -> Vec<(PathBuf, String)> {
    let mut by_host: BTreeMap<String, Vec<ResultRow>> = BTreeMap::new();
    for row in load_all_rows() {
        by_host.entry(row.host.clone()).or_default().push(row);
    }

    let dir = bench_dir();
    let mut files = Vec::new();
    let mut hosts = Vec::new();
    for (host, rows) in &by_host {
        files.push((dir.join(format!("RESULTS-{host}.md")), render_host(host, rows)));
        hosts.push((host.clone(), measured(rows), machine_label(host)));
    }
    files.push((dir.join("RESULTS.md"), render_index(&hosts)));
    files
}

/// The CPU model for the index's at-a-glance column, or `—` if no descriptor is recorded.
fn machine_label(host: &str) -> String {
    load_host(host)
        .and_then(|d| d.cpu_model)
        .unwrap_or_else(|| "—".to_string())
}

fn render_index(hosts: &[(String, bool, String)]) -> String {
    let mut out = String::from(INDEX_HEADER);
    if hosts.is_empty() {
        out.push_str("\n_No hosts recorded yet — run the drivers, then `render_results`._\n");
        return out;
    }
    out.push_str("\n| Host | Machine | Status | Results |\n|------|---------|--------|---------|\n");
    for (host, measured, machine) in hosts {
        let status = if *measured { "measured" } else { "pending" };
        let _ = writeln!(out, "| `{host}` | {machine} | {status} | [view](RESULTS-{host}.md) |");
    }
    out.push_str(INDEX_FOOTER);
    out
}

fn render_host(host: &str, rows: &[ResultRow]) -> String {
    let mut out = String::new();
    let _ = write!(out, "# Benchmark results — `{host}`\n\n[← All hosts](RESULTS.md)\n");
    render_machine(&mut out, host);

    let mut by_scenario: BTreeMap<String, Vec<&ResultRow>> = BTreeMap::new();
    for row in rows {
        by_scenario.entry(row.scenario.clone()).or_default().push(row);
    }
    for (scenario, srows) in &by_scenario {
        render_scenario(&mut out, scenario, srows);
    }
    out.push_str(HOST_FOOTNOTES);
    out
}

/// The machine the host's figures were measured on, from `results/<host>/host.json`. A
/// scaffolded-but-undescribed host (no descriptor) skips the section; a descriptor with
/// unfilled fields renders them as *pending*.
fn render_machine(out: &mut String, host: &str) {
    let Some(d) = load_host(host) else {
        return;
    };
    out.push_str("\n## Machine\n\n");
    let _ = writeln!(out, "- **CPU** — {}", spec(d.cpu_model.as_deref()));
    let _ = writeln!(out, "- **Cores** — {}", cores(d.physical_cores, d.logical_cores));
    let mem = d.total_memory_bytes.map(gib).unwrap_or_else(pending);
    let _ = writeln!(out, "- **Memory** — {mem}");
    let _ = writeln!(out, "- **OS** — {}", spec(d.os_version.as_deref()));
    let _ = writeln!(out, "- **Kernel** — {}", spec(d.kernel_version.as_deref()));
}

fn spec(value: Option<&str>) -> String {
    value.map(str::to_string).unwrap_or_else(pending)
}

fn pending() -> String {
    "_pending_".to_string()
}

fn cores(physical: Option<u32>, logical: Option<u32>) -> String {
    match (physical, logical) {
        (Some(p), Some(l)) => format!("{p} physical / {l} logical"),
        (Some(p), None) => format!("{p} physical"),
        (None, Some(l)) => format!("{l} logical"),
        (None, None) => pending(),
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn render_scenario(out: &mut String, scenario: &str, rows: &[&ResultRow]) {
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
                .find(|r| &r.implementation == name && r.axis == axis)
                .copied();
            let _ = write!(out, " {} |", cell(row, axis, manifest.expected_routes));
        }
        out.push('\n');
    }

    out.push('\n');
    for name in &impls {
        if let Some(row) = rows.iter().find(|r| &r.implementation == name) {
            let _ = writeln!(out, "- **{name}** — {}, {}, {}", row.commit, row.toolchain, row.host);
        }
    }
}

fn cell(row: Option<&ResultRow>, axis: Axis, expected_routes: u64) -> String {
    let Some(row) = row else {
        return "—".to_string();
    };
    let Some(value) = row.value else {
        return "_pending_".to_string();
    };
    match axis {
        Axis::Conformance => {
            let got = value as u64;
            let icon = if got == expected_routes { PASS_ICON } else { FAIL_ICON };
            format!("{icon} {got} / {expected_routes}")
        }
        _ => format!("{} {}", humanize(value), row.unit),
    }
}

/// True if any figure for this host has actually been measured (vs. all-`pending`).
fn measured(rows: &[ResultRow]) -> bool {
    rows.iter().any(|r| r.value.is_some())
}

/// Implementation columns: the reference (and any other external impl) first, alphabetically,
/// with our own `personal-rns` last — the field is anchored against the reference, not against
/// us, so we don't implicitly seat ourselves first.
fn impl_columns(rows: &[&ResultRow]) -> Vec<String> {
    let mut names: Vec<String> = rows.iter().map(|r| r.implementation.clone()).collect();
    names.sort();
    names.dedup();
    names.sort_by_key(|name| (name == PERSONAL_RNS, name.clone()));
    names
}

/// Axes present in the rows, in canonical display order.
fn axes_present(rows: &[&ResultRow]) -> Vec<Axis> {
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

const INDEX_HEADER: &str = "<!-- Generated by `cargo run --bin render_results` from benchmarks/results/. Do not edit by hand. -->

# Benchmark results

The suite runs on whatever machines we have; every host is its own column of the story, so
results are filed per host. Each figure is stamped with the commit, toolchain, and host that
produced it and lives as a row in `results/<host>/<scenario>/<impl>.jsonl` — the same schema
any implementation emits. These pages are rendered from those rows, and the website renders the
same files, so the two never drift.

**Comparability.** Conformance and throughput line up across implementations; memory and latency
stay within one (a GC and a no-alloc core racing on RSS would be a dishonest column). Numbers are
only comparable *within* a host — never race a laptop against a server.
";

const INDEX_FOOTER: &str = "
Pick a host above for its tables. A `pending` host has been scaffolded but not yet measured —
run the drivers there to fill it in.
";

const HOST_FOOTNOTES: &str = "
---

- _Conformance_ — distinct routes the engine resolves from the corpus, against the manifest's expected count.
- _Ingest throughput_ — best-of-N wall time to ingest the whole corpus into a fresh engine, as announces per second.

Regenerate: run each implementation's driver (`bench_result`, `reference/driver.py`) on this host to
refresh `results/`, then `render_results` to rewrite these tables.
";
