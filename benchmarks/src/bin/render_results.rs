//! Render the benchmark tables from the result substrate (`results/<host>/<scenario>/
//! <pairing>.jsonl`) joined with the implementation registry (`implementations/<slug>.json`).
//! Each host gets its own `RESULTS-<host>.md`; `RESULTS.md` is the index. The website renders
//! the same files, so the tables can't drift between GitHub and the site. The page is a
//! cross-implementation interop matrix: every initiator→responder pairing that ran a scenario
//! on this host, with conformance, delivered throughput, goodput, settlement latency, and the
//! energy per delivered message (bracketed on the live run, so efficiency is the realistic
//! firehose's own figure, not a synthetic one).
//!
//! `--check` re-renders every file and diffs against what's committed: the drift gate.
//! Generated, never hand-edited. Run: `cargo run --release --bin render_results [--check]`

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
    for row in load_all_rows()
        .into_iter()
        .filter(|row| Manifest::exists(&row.scenario))
    {
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

/// One initiator→responder pairing's figures for an interop scenario, aggregated from its rows.
struct Pairing {
    initiator: String,
    responder: String,
    scenario_version: Option<u32>,
    sent: Option<f64>,
    delivered: Option<f64>,
    timed_out: Option<f64>,
    raced: Option<f64>,
    delivered_per_sec: Option<f64>,
    goodput_bytes_per_sec: Option<f64>,
    rtt_p50: Option<f64>,
    rtt_p99: Option<f64>,
    mj_per_delivered: Option<f64>,
    /// The combined energy apportioned to each role by its CPU-time share (initiator = sender,
    /// responder = receiver/prover). Both `None` until an energy run files them.
    init_mj_per_delivered: Option<f64>,
    resp_mj_per_delivered: Option<f64>,
    init_rss_bytes: Option<f64>,
    resp_rss_bytes: Option<f64>,
}

/// How trustworthy a pairing's headline figures are: clean rows rank first, rows with no
/// conformance data yet sit in the middle, and rows that dropped or timed out messages sink
/// to the bottom — a cheap-but-broken run must never top a table.
fn conformance_rank(p: &Pairing) -> u8 {
    match (p.sent, p.delivered) {
        (Some(sent), Some(delivered)) => {
            let timed_out = p.timed_out.unwrap_or(0.0);
            let raced = p.raced.unwrap_or(0.0);
            if sent == delivered + timed_out + raced {
                0
            } else {
                2
            }
        }
        _ => 1,
    }
}

/// Split a pairing label ("Prns → RNS 1.3.5") into its initiator and responder.
fn split_pairing(label: &str) -> (String, String) {
    match label.split_once(" \u{2192} ") {
        Some((i, r)) => (i.trim().to_string(), r.trim().to_string()),
        None => (label.to_string(), label.to_string()),
    }
}

/// Aggregate the rows of one interop scenario into a pairing per initiator→responder.
fn pairings(rows: &[&ResultRow]) -> Vec<Pairing> {
    #[derive(Default)]
    struct Acc {
        version: Option<u32>,
        sent: Option<f64>,
        delivered: Option<f64>,
        timed_out: Option<f64>,
        raced: Option<f64>,
        delivered_per_sec: Option<f64>,
        goodput_bytes_per_sec: Option<f64>,
        rtt_p50: Option<f64>,
        rtt_p99: Option<f64>,
        mj_per_delivered: Option<f64>,
        init_mj_per_delivered: Option<f64>,
        resp_mj_per_delivered: Option<f64>,
        init_rss_bytes: Option<f64>,
        resp_rss_bytes: Option<f64>,
    }
    let mut by_pairing: BTreeMap<String, Acc> = BTreeMap::new();
    for row in rows {
        let acc = by_pairing.entry(row.implementation.clone()).or_default();
        acc.version = Some(
            acc.version
                .map_or(row.scenario_version, |v| v.max(row.scenario_version)),
        );
        match (row.axis, row.metric.as_str()) {
            (Axis::Conformance, "sent") => acc.sent = row.value,
            (Axis::Conformance, "delivered") => acc.delivered = row.value,
            (Axis::Conformance, "timed_out") => acc.timed_out = row.value,
            (Axis::Conformance, "raced") => acc.raced = row.value,
            (Axis::Throughput, "delivered_per_sec") => acc.delivered_per_sec = row.value,
            (Axis::Throughput, "goodput_bytes_per_sec") => acc.goodput_bytes_per_sec = row.value,
            (Axis::Latency, "rtt_p50_ms") => acc.rtt_p50 = row.value,
            (Axis::Latency, "rtt_p99_ms") => acc.rtt_p99 = row.value,
            (Axis::Energy, "net_millijoules_per_delivered") => acc.mj_per_delivered = row.value,
            (Axis::Energy, "initiator_net_millijoules_per_delivered") => {
                acc.init_mj_per_delivered = row.value
            }
            (Axis::Energy, "responder_net_millijoules_per_delivered") => {
                acc.resp_mj_per_delivered = row.value
            }
            (Axis::Memory, "initiator_peak_rss_bytes") => acc.init_rss_bytes = row.value,
            (Axis::Memory, "responder_peak_rss_bytes") => acc.resp_rss_bytes = row.value,
            _ => {}
        }
    }
    by_pairing
        .into_iter()
        .map(|(label, acc)| {
            let (initiator, responder) = split_pairing(&label);
            Pairing {
                initiator,
                responder,
                scenario_version: acc.version,
                sent: acc.sent,
                delivered: acc.delivered,
                timed_out: acc.timed_out,
                raced: acc.raced,
                delivered_per_sec: acc.delivered_per_sec,
                goodput_bytes_per_sec: acc.goodput_bytes_per_sec,
                rtt_p50: acc.rtt_p50,
                rtt_p99: acc.rtt_p99,
                mj_per_delivered: acc.mj_per_delivered,
                init_mj_per_delivered: acc.init_mj_per_delivered,
                resp_mj_per_delivered: acc.resp_mj_per_delivered,
                init_rss_bytes: acc.init_rss_bytes,
                resp_rss_bytes: acc.resp_rss_bytes,
            }
        })
        .collect()
}

fn render_scenario(
    out: &mut String,
    scenario: &str,
    rows: &[&ResultRow],
    impls: &[ImplementationDescriptor],
) {
    let manifest = Manifest::load(scenario);
    let measured_version = rows
        .iter()
        .map(|r| r.scenario_version)
        .max()
        .unwrap_or(manifest.version as u32);
    render_interop(
        out,
        scenario,
        pairings(rows),
        &manifest,
        measured_version,
        impls,
    );
}

/// The interop matrix: every initiator→responder pairing with its conformance, delivered
/// throughput, goodput, settlement latency, peak RSS, and energy per delivered message.
/// Conformant pairings rank first, ordered by energy per message ascending: a cheap-but-broken
/// run must never top the table, and the static GitHub table can't be re-sorted. Pairings
/// without an energy figure sort last within their conformance class, tie-broken by throughput.
/// The section version is the one recorded ON the rows, never the manifest's current version.
fn render_interop(
    out: &mut String,
    scenario: &str,
    mut pairings: Vec<Pairing>,
    manifest: &Manifest,
    measured_version: u32,
    impls: &[ImplementationDescriptor],
) {
    pairings.sort_by(|a, b| {
        conformance_rank(a)
            .cmp(&conformance_rank(b))
            .then_with(|| {
                let ea = a
                    .mj_per_delivered
                    .filter(|v| *v > 0.0)
                    .unwrap_or(f64::INFINITY);
                let eb = b
                    .mj_per_delivered
                    .filter(|v| *v > 0.0)
                    .unwrap_or(f64::INFINITY);
                ea.partial_cmp(&eb).unwrap_or(Ordering::Equal)
            })
            .then_with(|| {
                let ta = a.delivered_per_sec.unwrap_or(0.0);
                let tb = b.delivered_per_sec.unwrap_or(0.0);
                tb.partial_cmp(&ta).unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.initiator.cmp(&b.initiator))
            .then_with(|| a.responder.cmp(&b.responder))
    });

    let _ = write!(out, "\n## {scenario} (v{measured_version})\n\n");
    if manifest.version as u32 != measured_version {
        let _ = writeln!(
            out,
            "_The manifest has since moved to v{}; every figure below was measured under \
             v{measured_version}._\n",
            manifest.version
        );
    }
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "{}\n", manifest.description);
    }
    out.push_str(
        "Each row is one live pairing — the initiator drives a windowed firehose at the \
         responder over loopback, and every figure is the protocol's own: delivery proven by \
         receipt, latency from the proofs, energy bracketed around the run. Conformant \
         pairings rank first, ordered by energy per delivered message — a cheap-but-broken \
         run never tops the table; energy needs `sudo` for the power counters and renders \
         pending without it. Numbers compare within a host, never across.\n\n",
    );

    out.push_str(
        "| Initiator \u{2192} Responder | Conformance | Throughput | Goodput | RTT p50 / p99 | Peak RSS init / resp | Energy / msg |\n",
    );
    out.push_str(
        "|------------------------|-------------|-----------:|--------:|--------------:|---------------------:|-------------:|\n",
    );
    for p in &pairings {
        let version_marker = match p.scenario_version {
            Some(v) if v != measured_version => format!(" _(measured at v{v})_"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "| {} \u{2192} {}{version_marker} | {} | {} | {} | {} | {} | {} |",
            label_with_role(&p.initiator, impls),
            label_with_role(&p.responder, impls),
            interop_conformance_cell(p),
            throughput_cell(p.delivered_per_sec),
            goodput_cell(p.goodput_bytes_per_sec),
            rtt_cell(p.rtt_p50, p.rtt_p99),
            rss_cell(p.init_rss_bytes, p.resp_rss_bytes),
            energy_cell(
                p.mj_per_delivered,
                p.init_mj_per_delivered,
                p.resp_mj_per_delivered
            ),
        );
    }

    for caveat in &manifest.caveats {
        let _ = writeln!(out, "\n> _{caveat}_");
    }

    render_legend(out, &pairings, impls);
}

/// An implementation's display name, tagged `(ref)` when it is the parity reference.
fn label_with_role(name: &str, impls: &[ImplementationDescriptor]) -> String {
    let is_reference = impls
        .iter()
        .find(|d| d.implementation == name)
        .is_some_and(|d| d.role == ImplementationRole::Reference);
    if is_reference {
        format!("{name} _(ref)_")
    } else {
        name.to_string()
    }
}

/// `delivered / sent` with a pass/fail icon. Rows pass when every sent message is accounted
/// for by delivery, timeout, or a scenario-declared race bucket; timed-out and raced counts are
/// still called out so the reader sees where the accounting landed.
fn interop_conformance_cell(p: &Pairing) -> String {
    match (p.sent, p.delivered) {
        (Some(sent), Some(delivered)) => {
            let timed_out = p.timed_out.unwrap_or(0.0);
            let raced = p.raced.unwrap_or(0.0);
            let icon = if sent == delivered + timed_out + raced {
                PASS_ICON
            } else {
                FAIL_ICON
            };
            let mut cell = format!("{icon} {} / {}", commas(delivered), commas(sent));
            if timed_out > 0.0 {
                let _ = write!(cell, " \u{00b7} {} timed out", commas(timed_out));
            }
            if raced > 0.0 {
                let _ = write!(cell, " \u{00b7} {} raced", commas(raced));
            }
            cell
        }
        _ => pending(),
    }
}

fn throughput_cell(value: Option<f64>) -> String {
    value
        .map(|t| format!("{} msg/s", humanize(t)))
        .unwrap_or_else(pending)
}

fn goodput_cell(value: Option<f64>) -> String {
    value
        .map(|b| {
            if b >= 1e6 {
                format!("{:.1} MB/s", b / 1e6)
            } else if b >= 1e3 {
                format!("{:.0} kB/s", b / 1e3)
            } else {
                format!("{b:.0} B/s")
            }
        })
        .unwrap_or_else(pending)
}

fn rtt_cell(p50: Option<f64>, p99: Option<f64>) -> String {
    match (p50, p99) {
        (Some(a), Some(b)) => format!("{a:.0} / {b:.0} ms"),
        _ => pending(),
    }
}

/// A delivered message can never cost zero joules — a non-positive figure means the run's
/// package energy fell below the idle baseline (baseline drift), so it renders as pending
/// rather than as an impossibly perfect number.
/// The combined energy per delivered message, with the CPU-apportioned sender/receiver split
/// appended when an energy run filed it (`i` = initiator, `r` = responder).
fn energy_cell(combined: Option<f64>, init: Option<f64>, resp: Option<f64>) -> String {
    match combined {
        Some(mj) if mj > 0.0 => match (init, resp) {
            (Some(i), Some(r)) if i > 0.0 || r > 0.0 => {
                format!("{mj:.2} mJ \u{00b7} i {i:.2} / r {r:.2}")
            }
            _ => format!("{mj:.2} mJ"),
        },
        _ => pending(),
    }
}

fn rss_cell(init: Option<f64>, resp: Option<f64>) -> String {
    let mib = |bytes: f64| bytes / (1024.0 * 1024.0);
    match (init, resp) {
        (Some(i), Some(r)) => format!("{:.1} / {:.1} MiB", mib(i), mib(r)),
        _ => pending(),
    }
}

/// Group an integer count with thousands separators — conformance shows exact `delivered /
/// sent`, where `128,193 / 128,193` reads as proof and `128.2k` would hide a near miss.
fn commas(v: f64) -> String {
    let n = v.round() as i64;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Each implementation that appears in the matrix: its language, Ed25519 backend, and where it
/// came from — enough to reproduce or audit any pairing.
fn render_legend(out: &mut String, pairings: &[Pairing], impls: &[ImplementationDescriptor]) {
    let mut names: Vec<String> = pairings
        .iter()
        .flat_map(|p| [p.initiator.clone(), p.responder.clone()])
        .collect();
    names.sort();
    names.dedup();

    out.push_str("\n**Implementations.**\n");
    for name in &names {
        let descriptor = impls.iter().find(|d| &d.implementation == name);
        let language = descriptor.map_or("—", |d| d.language.as_str());
        let backend = descriptor.map_or("—", |d| d.crypto_backend.as_str());
        let mut line = format!("\n- **{name}** — {language}, {backend}");
        if let Some(repo) = descriptor.and_then(|d| d.repo.as_deref()) {
            let _ = write!(line, " \u{00b7} [{repo}]({repo})");
            if let Some(pin) = descriptor.and_then(|d| d.pinned_ref.as_deref()) {
                let _ = write!(line, " @ `{pin}`");
            }
        }
        if let Some(license) = descriptor.and_then(|d| d.license.as_deref()) {
            let _ = write!(line, " \u{00b7} {license}");
        }
        if let Some(notes) = descriptor.and_then(|d| d.notes.as_deref()) {
            let _ = write!(line, "\n  - _{notes}_");
        }
        out.push_str(&line);
    }
    out.push('\n');
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
    caveats: Vec<String>,
}

impl Manifest {
    fn exists(scenario: &str) -> bool {
        scenario_dir(scenario).join("manifest.json").is_file()
    }

    fn load(scenario: &str) -> Self {
        let path = scenario_dir(scenario).join("manifest.json");
        let json: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(serde_json::Value::Null);
        let caveats = json["caveats"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Manifest {
            version: json["version"].as_u64().unwrap_or(0),
            description: json["description"].as_str().unwrap_or("").to_string(),
            caveats,
        }
    }
}

const INDEX_HEADER: &str = "<!-- Generated by `cargo run --bin render_results` from benchmarks/results/. Do not edit by hand. -->

# Benchmark results

The suite runs on whatever machines we have; every host is its own column of the story, so
results are filed per host. Each figure is stamped with the commit, toolchain, and host that
produced it and lives as a row in `results/<host>/<scenario>/<pairing>.jsonl` — the same schema
any implementation emits. These pages are rendered from those rows, and the website renders the
same files, so the two never drift.

**Comparability.** Conformance, throughput, and latency line up across pairings *within* a host;
energy is the package-domain figure that host's silicon actually reports. Numbers are only
comparable within a host — never race a laptop against a server.
";

const INDEX_FOOTER: &str = "
Pick a host above for its tables. A `pending` host has been scaffolded but not yet measured —
run the drivers there to fill it in.
";

const HOST_FOOTNOTES: &str = "
---

- _Conformance_ — every sent message accounted for, shown as `delivered / sent`. Extra suffixes call out messages that timed out or landed in a scenario-declared `raced` bucket, such as the RNS 1.3.5 request-response send-before-register loopback race.
- _Throughput_ — delivered messages per second, initiator-bound.
- _Goodput_ — delivered application payload per second (framing excluded).
- _RTT_ — settlement latency from the protocol's own proofs, p50 / p99.
- _Peak RSS_ — peak resident set size (the physical RAM a process holds), initiator / responder, reaped from outside so a contestant can't under-report it.
- _Energy / msg_ — (package energy − idle baseline) ÷ delivered: the joules per delivered message. The power counters are package-domain, so this is the *combined* cost of both roles on the SoC; only the diagonal (a self-pair) is a single impl. The `i … / r …` split apportions it to initiator vs responder by their CPU-time share — the honest cross-platform proxy (Linux RAPL has no per-process counter), exact only insofar as power tracks CPU time. Needs `sudo` for the power counters; renders pending without.

Regenerate: `sudo env \"PATH=$PATH\" ./run.sh` (root, for the power counters), then `cargo run --bin render_results`.
";
