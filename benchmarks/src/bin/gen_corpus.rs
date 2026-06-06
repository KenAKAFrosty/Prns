//! The engine side of the scenario corpora. The *canonical* wire bytes are minted by
//! the RNS 1.3.1 reference (`reference/gen.py`); this bin's job is twofold:
//!
//!   `gen_corpus --check`  regenerate the announces from *our* engine and diff them
//!                         against every committed corpus — the parity oracle. Green
//!                         here and green from `gen.py --check` together prove our
//!                         announces are byte-identical to the reference's.
//!   `gen_corpus`          write each `manifest.json` (the scenario metadata our engine
//!                         owns), and bootstrap `packets.hex` when no RNS is set up.
//!
//! Bump a scenario's `version` when it changes (numbers across versions aren't
//! comparable).

use std::path::Path;

use benchmarks::{announce_fixtures, load_corpus, scenario_dir, to_hex};

/// What a scenario does with its announces — shapes the manifest's `operations` block.
enum Shape {
    /// Replay in order, then settle `ticks` cycles (the single-interface ingest path).
    Sequential { ticks: usize },
    /// Shard evenly across worker threads, single-thread vs all logical cores.
    Parallel,
}

struct Scenario {
    name: &'static str,
    count: usize,
    version: u32,
    shape: Shape,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "announce-256",
        count: 256,
        version: 1,
        shape: Shape::Sequential { ticks: 64 },
    },
    Scenario {
        name: "announce-parallel",
        count: 2560,
        version: 1,
        shape: Shape::Parallel,
    },
];

fn main() {
    let checking = std::env::args().any(|a| a == "--check");
    let mut diverged = false;
    for scenario in SCENARIOS {
        if checking {
            diverged |= !check(scenario);
        } else {
            write(scenario);
        }
    }
    if diverged {
        std::process::exit(1);
    }
}

fn check(scenario: &Scenario) -> bool {
    let ours = announce_fixtures(scenario.count);
    let committed = load_corpus(&scenario_dir(scenario.name));
    if ours == committed {
        println!("{}: engine parity IDENTICAL ({} packets)", scenario.name, ours.len());
        return true;
    }
    let first = (0..ours.len().min(committed.len()))
        .find(|&i| ours[i] != committed[i])
        .unwrap_or(ours.len().min(committed.len()));
    eprintln!("{}: engine parity DIVERGES at packet {first}", scenario.name);
    if first < committed.len() {
        eprintln!("  committed: {}…", &to_hex(&committed[first])[..64.min(committed[first].len() * 2)]);
    }
    if first < ours.len() {
        eprintln!("  engine:    {}…", &to_hex(&ours[first])[..64.min(ours[first].len() * 2)]);
    }
    false
}

fn write(scenario: &Scenario) {
    let announces = announce_fixtures(scenario.count);
    let dir = scenario_dir(scenario.name);
    std::fs::create_dir_all(&dir).expect("create scenario dir");

    if !Path::new(&dir.join("packets.hex")).exists() {
        let mut packets = String::new();
        for announce in &announces {
            packets.push_str(&to_hex(announce));
            packets.push('\n');
        }
        std::fs::write(dir.join("packets.hex"), packets).expect("write packets.hex");
        eprintln!("{}: bootstrapped packets.hex from the engine", scenario.name);
    }

    std::fs::write(dir.join("manifest.json"), manifest(scenario, announces.len()))
        .expect("write manifest.json");
    eprintln!("{}: wrote manifest to {}", scenario.name, dir.display());
}

fn manifest(scenario: &Scenario, n: usize) -> String {
    let name = scenario.name;
    let version = scenario.version;
    match scenario.shape {
        Shape::Sequential { ticks } => format!(
            r#"{{
  "name": "{name}",
  "version": {version},
  "description": "Ingest {n} distinct signed lxmf.delivery announces in order over one interface, then settle {ticks} ticks.",
  "source": "reference/gen.py (RNS 1.3.1) — canonical; engine reproduces byte-for-byte",
  "input": {{ "packets": "packets.hex", "encoding": "hex-per-line", "count": {n} }},
  "operations": {{ "ingest": "all-in-order", "settle_ticks": {ticks} }},
  "expected": {{ "route_count": {n} }}
}}
"#
        ),
        Shape::Parallel => format!(
            r#"{{
  "name": "{name}",
  "version": {version},
  "description": "Ingest {n} distinct signed lxmf.delivery announces, sharded evenly across worker threads; each shard runs the real parse → Ed25519 verify → store path on its own fresh engine. Swept single-thread vs all of the host's logical cores.",
  "source": "reference/gen.py (RNS 1.3.1) — canonical; engine reproduces byte-for-byte",
  "input": {{ "packets": "packets.hex", "encoding": "hex-per-line", "count": {n} }},
  "operations": {{ "ingest": "sharded-across-threads", "threads": "1 vs logical-cores" }},
  "expected": {{ "route_count": {n} }}
}}
"#
        ),
    }
}
