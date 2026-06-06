//! The engine side of the scenario corpus. The *canonical* wire bytes are minted by
//! the RNS 1.3.1 reference (`reference/gen.py`); this bin's job is twofold:
//!
//!   `gen_corpus --check`  regenerate the announces from *our* engine and diff them
//!                         against the committed corpus — the parity oracle. Green
//!                         here and green from `gen.py --check` together prove our
//!                         announces are byte-identical to the reference's.
//!   `gen_corpus`          write `manifest.json` (the scenario metadata our engine
//!                         owns), and bootstrap `packets.hex` when no RNS is set up.
//!
//! Bump the manifest `version` when a scenario changes (numbers across versions
//! aren't comparable).

use std::path::Path;

use benchmarks::{announce_fixtures, load_corpus, scenario_dir, to_hex};

const NAME: &str = "announce-256";
const COUNT: usize = 256;
const SETTLE_TICKS: usize = 64;
const VERSION: u32 = 1;

fn main() {
    if std::env::args().any(|a| a == "--check") {
        check();
    } else {
        write();
    }
}

fn check() {
    let ours = announce_fixtures(COUNT);
    let committed = load_corpus(&scenario_dir(NAME));
    if ours == committed {
        println!("engine parity: IDENTICAL ({} packets)", ours.len());
        return;
    }
    let first = (0..ours.len().min(committed.len()))
        .find(|&i| ours[i] != committed[i])
        .unwrap_or(ours.len().min(committed.len()));
    eprintln!("engine parity: DIVERGES at packet {first}");
    eprintln!("  committed: {}…", &to_hex(&committed[first])[..64.min(committed[first].len() * 2)]);
    eprintln!("  engine:    {}…", &to_hex(&ours[first])[..64.min(ours[first].len() * 2)]);
    std::process::exit(1);
}

fn write() {
    let announces = announce_fixtures(COUNT);
    let dir = scenario_dir(NAME);
    std::fs::create_dir_all(&dir).expect("create scenario dir");

    if !Path::new(&dir.join("packets.hex")).exists() {
        let mut packets = String::new();
        for announce in &announces {
            packets.push_str(&to_hex(announce));
            packets.push('\n');
        }
        std::fs::write(dir.join("packets.hex"), packets).expect("write packets.hex");
        eprintln!("bootstrapped packets.hex from the engine (canonical source is reference/gen.py)");
    }

    let manifest = format!(
        r#"{{
  "name": "{NAME}",
  "version": {VERSION},
  "description": "Ingest {n} distinct signed lxmf.delivery announces in order over one interface, then settle {SETTLE_TICKS} ticks.",
  "source": "reference/gen.py (RNS 1.3.1) — canonical; engine reproduces byte-for-byte",
  "input": {{ "packets": "packets.hex", "encoding": "hex-per-line", "count": {n} }},
  "operations": {{ "ingest": "all-in-order", "settle_ticks": {SETTLE_TICKS} }},
  "expected": {{ "route_count": {n} }}
}}
"#,
        n = announces.len(),
    );
    std::fs::write(dir.join("manifest.json"), manifest).expect("write manifest.json");

    eprintln!("wrote manifest to {}", dir.display());
}
