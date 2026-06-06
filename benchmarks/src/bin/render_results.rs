//! Render the benchmark tables from the result substrate (`results/<host>/<scenario>/
//! <impl>.jsonl`) joined with the implementation registry (`implementations/<slug>.json`).
//! Each host gets its own `RESULTS-<host>.md`; `RESULTS.md` is the index that links to
//! them. The website renders the same files, so the tables can't drift between GitHub and
//! the site.
//!
//! Per scenario, the page is a **cross-implementation comparison**: every implementation
//! that filed a figure for this host, sorted by ingest throughput, with its language,
//! Ed25519 backend, conformance, and speed relative to the Python reference. announce-256
//! is ~97% Ed25519 verify, so the ranking is a crypto-backend story.
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
    let impls = load_implementations();
    let mut by_host: BTreeMap<String, Vec<ResultRow>> = BTreeMap::new();
    for row in load_all_rows() {
        by_host.entry(row.host.clone()).or_default().push(row);
    }

    let dir = bench_dir();
    let mut files = Vec::new();
    let mut hosts = Vec::new();
    for (host, rows) in &by_host {
        files.push((dir.join(format!("RESULTS-{host}.md")), render_host(host, rows, &impls)));
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

fn render_host(host: &str, rows: &[ResultRow], impls: &[ImplementationDescriptor]) -> String {
    let mut out = String::new();
    let _ = write!(out, "# Benchmark results — `{host}`\n\n[← All hosts](RESULTS.md)\n");
    render_machine(&mut out, host);

    let mut by_scenario: BTreeMap<String, Vec<&ResultRow>> = BTreeMap::new();
    for row in rows {
        by_scenario.entry(row.scenario.clone()).or_default().push(row);
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

/// One implementation's figures for a scenario, joined with its descriptor. A standard
/// scenario fills `throughput_single`; the parallel scenario fills `throughput_by_threads`,
/// one entry per swept thread count. `conformance_metric` distinguishes a full route-store
/// pass (`routes_resolved`) from a verify-only port (`announces_verified`).
struct Comparison<'a> {
    name: String,
    descriptor: Option<&'a ImplementationDescriptor>,
    conformance: Option<f64>,
    conformance_metric: Option<String>,
    throughput_single: Option<f64>,
    throughput_by_threads: BTreeMap<u32, f64>,
    toolchain: String,
}

/// The cross-implementation comparison for one scenario: the standard `×ref` ingest table,
/// or — when the scenario sweeps parallelism — the single-thread-vs-all-cores table.
fn render_scenario(
    out: &mut String,
    scenario: &str,
    rows: &[&ResultRow],
    impls: &[ImplementationDescriptor],
) {
    let manifest = Manifest::load(scenario);
    let entries = comparisons(rows, impls);
    if entries.iter().any(|e| !e.throughput_by_threads.is_empty()) {
        render_parallel(out, scenario, entries, &manifest);
    } else {
        render_standard(out, scenario, entries, &manifest, impls);
    }
}

/// The single-interface ingest comparison: every implementation sorted by ingest throughput,
/// with the Python reference's throughput as the `×ref` anchor.
fn render_standard(
    out: &mut String,
    scenario: &str,
    mut entries: Vec<Comparison<'_>>,
    manifest: &Manifest,
    impls: &[ImplementationDescriptor],
) {
    entries.sort_by(|a, b| {
        throughput_desc(a.throughput_single, &a.name, b.throughput_single, &b.name)
    });
    let reference_throughput = impls
        .iter()
        .find(|d| d.role == ImplementationRole::Reference)
        .and_then(|d| entries.iter().find(|e| e.name == d.implementation))
        .and_then(|e| e.throughput_single);

    let _ = write!(out, "\n## {scenario} (v{})\n\n", manifest.version);
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "{}\n", manifest.description);
    }
    out.push_str(
        "Same wire bytes through each implementation's real parse → Ed25519 verify → store \
         path, best-of-50 min wall time. This axis is ~97% Ed25519 verify, so the ranking is a \
         crypto-backend story; figures are comparable only within this host.\n\n",
    );

    out.push_str(
        "| Implementation | Language | Ed25519 backend | Conformance | Ingest throughput | ×ref |\n",
    );
    out.push_str(
        "|----------------|----------|-----------------|-------------|-------------------|------|\n",
    );
    let mut any_partial = false;
    for entry in &entries {
        let language = entry.descriptor.map_or("—", |d| d.language.as_str());
        let backend = entry.descriptor.map_or("—", |d| d.crypto_backend.as_str());
        let is_reference = entry.descriptor.is_some_and(|d| d.role == ImplementationRole::Reference);
        let partial = entry.descriptor.and_then(|d| d.maturity.as_deref()) == Some("partial");
        any_partial |= partial;

        let mut label = entry.name.clone();
        if is_reference {
            label.push_str(" _(reference)_");
        }
        if partial {
            label.push_str(" †");
        }
        let conformance = conformance_cell(entry.conformance, manifest.expected_routes);
        let throughput = entry
            .throughput_single
            .map(|t| format!("{} announce/s", humanize(t)))
            .unwrap_or_else(pending);
        let relative = match (entry.throughput_single, reference_throughput) {
            (Some(t), Some(r)) if r > 0.0 => format!("{:.1}×", t / r),
            _ => "—".to_string(),
        };
        let _ = writeln!(
            out,
            "| {label} | {language} | {backend} | {conformance} | {throughput} | {relative} |"
        );
    }

    if any_partial {
        out.push_str(
            "\n† Marked partial / not-yet-feature-complete on the upstream maturity list — \
             included as a data point, not part of the feature-complete tier.\n",
        );
    }

    render_provenance(out, &entries);
}

/// The parallelism comparison: every implementation's ingest throughput single-threaded vs
/// sharded across all of the host's logical cores, sorted by the all-cores figure.
fn render_parallel(
    out: &mut String,
    scenario: &str,
    mut entries: Vec<Comparison<'_>>,
    manifest: &Manifest,
) {
    let thread_counts: Vec<u32> = entries
        .iter()
        .flat_map(|e| e.throughput_by_threads.keys().copied())
        .collect();
    let lo = thread_counts.iter().copied().min().unwrap_or(1);
    let hi = thread_counts.iter().copied().max().unwrap_or(lo);

    entries.sort_by(|a, b| {
        let ka = a.throughput_by_threads.get(&hi).copied();
        let kb = b.throughput_by_threads.get(&hi).copied();
        throughput_desc(ka, &a.name, kb, &b.name)
    });

    let _ = write!(out, "\n## {scenario} (v{})\n\n", manifest.version);
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "{}\n", manifest.description);
    }
    out.push_str(
        "Best-of-30 min wall time; the two columns are single-threaded and the same corpus \
         sharded across all of this host's logical cores. The announce path is ~97% independent \
         per-announce Ed25519 verify, so it parallelizes cleanly — but a runtime with a global \
         interpreter lock (CPython) can't use the extra cores from threads, so its all-cores \
         figure barely moves, while compiled/JIT runtimes scale with the core count. Figures are \
         comparable only within this host.\n\n",
    );

    let _ = writeln!(
        out,
        "| Implementation | Language | Conformance | {} | {} |",
        thread_header(lo),
        thread_header(hi)
    );
    out.push_str("|----------------|----------|-------------|--------:|--------:|\n");

    let mut any_partial = false;
    let mut any_verify_only = false;
    for entry in &entries {
        let language = entry.descriptor.map_or("—", |d| d.language.as_str());
        let is_reference = entry.descriptor.is_some_and(|d| d.role == ImplementationRole::Reference);
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
        let lo_cell = throughput_cell(entry.throughput_by_threads.get(&lo).copied());
        let hi_cell = throughput_cell(entry.throughput_by_threads.get(&hi).copied());
        let _ = writeln!(out, "| {label} | {language} | {conformance} | {lo_cell} | {hi_cell} |");
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
             thread-safe, so the parallel figure isolates the verify work that dominates this axis.\n",
        );
    }

    render_provenance(out, &entries);
}

/// Throughput-descending order (unmeasured last), ties broken by name — the table sort.
fn throughput_desc(a: Option<f64>, a_name: &str, b: Option<f64>, b_name: &str) -> Ordering {
    let ka = a.unwrap_or(f64::NEG_INFINITY);
    let kb = b.unwrap_or(f64::NEG_INFINITY);
    kb.partial_cmp(&ka)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a_name.cmp(b_name))
}

fn thread_header(t: u32) -> String {
    if t == 1 {
        "1 thread".to_string()
    } else {
        format!("{t} threads")
    }
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
        throughput_single: Option<f64>,
        throughput_by_threads: BTreeMap<u32, f64>,
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
            Axis::Throughput => match row.threads {
                None => acc.throughput_single = row.value,
                Some(t) => {
                    if let Some(v) = row.value {
                        acc.throughput_by_threads.insert(t, v);
                    }
                }
            },
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
            throughput_single: acc.throughput_single,
            throughput_by_threads: acc.throughput_by_threads,
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
            let icon = if got == expected { PASS_ICON } else { FAIL_ICON };
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
- _Ingest throughput_ — best-of-N wall time to parse + verify + store the whole corpus into a fresh engine, as announces per second.
- _×ref_ — throughput relative to the Python reference (`RNS`) on this host.
- _1 thread / N threads_ — for the parallel scenario, ingest throughput single-threaded and sharded across all of this host's logical cores.

Regenerate: run each implementation's driver on this host (`bench_result`, `bench_parallel`,
`reference/driver.py`, `reference/driver_parallel.py`, and the `external/<impl>/run.sh` + `run-mt.sh`
one-command drivers) to refresh `results/`, then `render_results` to rewrite these tables.
";
