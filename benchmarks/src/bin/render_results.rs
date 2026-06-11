//! Render the benchmark tables from the result substrate (`results/<host>/<scenario>/
//! <impl>.jsonl`) joined with the implementation registry (`implementations/<slug>.json`).
//! Each host gets its own `RESULTS-<host>.md`; `RESULTS.md` is the index that links to
//! them. The website renders the same files, so the tables can't drift between GitHub and
//! the site.
//!
//! The page is a **cross-implementation comparison**: every implementation that filed a
//! figure for this host, with its language, Ed25519 backend, conformance, sustained
//! throughput, CPU power, and energy per announce, sorted by energy. The announce path is
//! ~97% Ed25519 verify, so the ranking is a crypto-backend story.
//!
//! `--check` re-renders every file and diffs against what's committed — the drift gate,
//! mirroring `gen_corpus --check`. Generated, never hand-edited.
//!
//! Run: `cargo run --release --bin render_results [--check]`

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use benchmarks::{
    load_all_rows, load_host, load_implementations, scenario_dir, Axis, ImplementationDescriptor,
    ImplementationRole, ResultRow,
};

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
            println!(
                "results tables: IN SYNC with the substrate ({} files)",
                files.len()
            );
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
    eprintln!(
        "wrote {} result files to {}",
        files.len(),
        bench_dir().display()
    );
}

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The index (`RESULTS.md`) plus one `RESULTS-<host>.md` per host.
fn render_all() -> Vec<(PathBuf, String)> {
    let impls = load_implementations();
    let mut by_host: BTreeMap<String, Vec<ResultRow>> = BTreeMap::new();
    for row in load_all_rows() {
        by_host.entry(row.host.clone()).or_default().push(row);
    }

    let dir = bench_dir();
    let mut files = Vec::new();
    let mut hosts = Vec::new();
    for (host, rows) in &by_host {
        files.push((
            dir.join(format!("RESULTS-{host}.md")),
            render_host(host, rows, &impls),
        ));
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
    out.push_str(
        "\n| Host | Machine | Status | Results |\n|------|---------|--------|---------|\n",
    );
    for (host, measured, machine) in hosts {
        let status = if *measured { "measured" } else { "pending" };
        let _ = writeln!(
            out,
            "| `{host}` | {machine} | {status} | [view](RESULTS-{host}.md) |"
        );
    }
    out.push_str(INDEX_FOOTER);
    out
}

fn render_host(host: &str, rows: &[ResultRow], impls: &[ImplementationDescriptor]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "# Benchmark results — `{host}`\n\n[← All hosts](RESULTS.md)\n"
    );
    render_machine(&mut out, host);

    let mut by_scenario: BTreeMap<String, Vec<&ResultRow>> = BTreeMap::new();
    for row in rows {
        by_scenario
            .entry(row.scenario.clone())
            .or_default()
            .push(row);
    }
    for (scenario, srows) in &by_scenario {
        render_scenario(&mut out, scenario, srows, impls);
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
    let _ = writeln!(
        out,
        "- **Cores** — {}",
        cores(d.physical_cores, d.logical_cores)
    );
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

/// One implementation's figures for the energy scenario, joined with its descriptor.
/// `conformance_metric` distinguishes a full route-store pass (`routes_resolved`) from a
/// verify-only port (`announces_verified`).
struct Comparison<'a> {
    name: String,
    descriptor: Option<&'a ImplementationDescriptor>,
    conformance: Option<f64>,
    conformance_metric: Option<String>,
    throughput: Option<f64>,
    power_watts: Option<f64>,
    energy_uj: Option<f64>,
    toolchain: String,
}

/// The cross-implementation comparison for a scenario — the energy table.
fn render_scenario(
    out: &mut String,
    scenario: &str,
    rows: &[&ResultRow],
    impls: &[ImplementationDescriptor],
) {
    let manifest = Manifest::load(scenario);
    let entries = comparisons(rows, impls);
    render_energy(out, scenario, entries, &manifest);
}

/// The energy comparison: sustained all-cores ingest, with the Ed25519 backend (the
/// controlled variable) and conformance alongside throughput, CPU power, and the headline —
/// energy per announce, the price a battery/solar node pays. Sorted by energy ascending.
fn render_energy(
    out: &mut String,
    scenario: &str,
    mut entries: Vec<Comparison<'_>>,
    manifest: &Manifest,
) {
    entries.sort_by(|a, b| {
        let ka = a.energy_uj.unwrap_or(f64::INFINITY);
        let kb = b.energy_uj.unwrap_or(f64::INFINITY);
        ka.partial_cmp(&kb)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    let _ = write!(out, "\n## {scenario} (v{})\n\n", manifest.version);
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "{}\n", manifest.description);
    }
    out.push_str(
        "Energy per announce = (active CPU power − idle baseline) ÷ throughput — it normalizes \
         throughput and is fair across every runtime regardless of GC/JIT/interpreter, because \
         it's the actual joules a user pays. The Ed25519 backend is the controlled variable; \
         conformance confirms every implementation processed the same work. Measured on macOS \
         via `powermetrics` (root), so it reproduces with `sudo`, not the one-command drivers.\n\n",
    );

    out.push_str(
        "| Implementation | Language | Ed25519 backend | Conformance | Throughput | CPU power | Energy / announce |\n",
    );
    out.push_str(
        "|----------------|----------|-----------------|-------------|-----------:|---------:|------------------:|\n",
    );

    let mut any_partial = false;
    let mut any_verify_only = false;
    for entry in &entries {
        let language = entry.descriptor.map_or("—", |d| d.language.as_str());
        let backend = entry.descriptor.map_or("—", |d| d.crypto_backend.as_str());
        let is_reference = entry
            .descriptor
            .is_some_and(|d| d.role == ImplementationRole::Reference);
        let partial = entry.descriptor.and_then(|d| d.maturity.as_deref()) == Some("partial");
        let verify_only = entry.conformance_metric.as_deref() == Some("announces_verified");
        any_partial |= partial;
        any_verify_only |= verify_only;

        let mut label = entry.name.clone();
        if is_reference {
            label.push_str(" _(reference)_");
        }
        if partial {
            label.push_str(" †");
        }
        if verify_only {
            label.push_str(" ‡");
        }
        let conformance = conformance_cell(entry.conformance, manifest.expected_routes);
        let throughput = throughput_cell(entry.throughput);
        let power = entry
            .power_watts
            .map(|w| format!("{w:.1} W"))
            .unwrap_or_else(pending);
        let energy = entry
            .energy_uj
            .map(|e| format!("{e:.0} µJ"))
            .unwrap_or_else(pending);
        let _ = writeln!(
            out,
            "| {label} | {language} | {backend} | {conformance} | {throughput} | {power} | {energy} |"
        );
    }

    if any_partial {
        out.push_str(
            "\n† Marked partial / not-yet-feature-complete on the upstream maturity list — \
             included as a data point, not part of the feature-complete tier.\n",
        );
    }
    if any_verify_only {
        out.push_str(
            "\n‡ Measured verify-only (parse + Ed25519 verify, no route store) — its store isn't \
             thread-safe; this axis is ~97% verify, so it isolates the dominant work.\n",
        );
    }
    out.push_str(
        "\nThroughput here is the sustained average under continuous all-cores load (the energy \
         denominator). Python runs all-core threads but is GIL-bound, so its all-cores ≈ one core.\n",
    );

    render_provenance(out, &entries);
}

fn throughput_cell(value: Option<f64>) -> String {
    value
        .map(|t| format!("{} announce/s", humanize(t)))
        .unwrap_or_else(pending)
}

/// Collect each implementation's conformance + throughput figures and join with its
/// descriptor (unsorted — each table sorts by its own throughput column).
fn comparisons<'a>(
    rows: &[&ResultRow],
    impls: &'a [ImplementationDescriptor],
) -> Vec<Comparison<'a>> {
    #[derive(Default)]
    struct Acc {
        conformance: Option<f64>,
        conformance_metric: Option<String>,
        throughput: Option<f64>,
        power_watts: Option<f64>,
        energy_uj: Option<f64>,
        toolchain: String,
    }
    let mut figures: BTreeMap<String, Acc> = BTreeMap::new();
    for row in rows {
        let acc = figures.entry(row.implementation.clone()).or_default();
        acc.toolchain = row.toolchain.clone();
        match row.axis {
            Axis::Conformance => {
                acc.conformance = row.value;
                acc.conformance_metric = Some(row.metric.clone());
            }
            Axis::Throughput => acc.throughput = row.value,
            Axis::Power => acc.power_watts = row.value,
            Axis::Energy => acc.energy_uj = row.value,
            _ => {}
        }
    }

    figures
        .into_iter()
        .map(|(name, acc)| Comparison {
            descriptor: impls.iter().find(|d| d.implementation == name),
            name,
            conformance: acc.conformance,
            conformance_metric: acc.conformance_metric,
            throughput: acc.throughput,
            power_watts: acc.power_watts,
            energy_uj: acc.energy_uj,
            toolchain: acc.toolchain,
        })
        .collect()
}

/// Where each figure came from: repo, pinned ref, license, and the toolchain that produced
/// the row — enough to reproduce or audit any column.
fn render_provenance(out: &mut String, entries: &[Comparison<'_>]) {
    out.push_str("\n**Provenance.**\n\n");
    for entry in entries {
        let mut line = format!("- **{}** — ", entry.name);
        match entry.descriptor.and_then(|d| d.repo.as_deref()) {
            Some(repo) => {
                let _ = write!(line, "[{repo}]({repo})");
                if let Some(pin) = entry.descriptor.and_then(|d| d.pinned_ref.as_deref()) {
                    let _ = write!(line, " @ `{pin}`");
                }
            }
            None => line.push('—'),
        }
        if let Some(license) = entry.descriptor.and_then(|d| d.license.as_deref()) {
            let _ = write!(line, " · {license}");
        }
        let _ = writeln!(line, " · {}", entry.toolchain);
        out.push_str(&line);
    }
}

fn conformance_cell(value: Option<f64>, expected: u64) -> String {
    match value {
        None => pending(),
        Some(v) => {
            let got = v as u64;
            let icon = if got == expected {
                PASS_ICON
            } else {
                FAIL_ICON
            };
            format!("{icon} {got} / {expected}")
        }
    }
}

/// True if any figure for this host has actually been measured (vs. all-`pending`).
fn measured(rows: &[ResultRow]) -> bool {
    rows.iter().any(|r| r.value.is_some())
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

- _Conformance_ — distinct routes the engine resolves from the corpus (or announces verified, for a verify-only port), against the manifest's expected count.
- _Throughput_ — sustained announces per second under continuous all-cores load (the energy denominator).
- _CPU power_ — average active CPU power over that sustained run.
- _Energy / announce_ — (active power − idle baseline) ÷ throughput; the cross-comparable price paid, sorted ascending.

Regenerate: `energy/build.sh` then `sudo energy/measure.sh` (root, for the power counters) to
refresh `results/`, then `cargo run --bin render_results` to rewrite these tables.
";
