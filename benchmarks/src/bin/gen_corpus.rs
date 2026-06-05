//! Regenerate a scenario corpus on disk: the language-neutral input (wire packets +
//! manifest) every measurement backend — and every other implementation — replays.
//! Run after changing a scenario, and bump the manifest `version` (numbers across
//! versions aren't comparable).
//!
//! Run: `cargo run --release --bin gen_corpus`

use benchmarks::{announce_fixtures, scenario_dir, to_hex};

const NAME: &str = "announce-256";
const COUNT: usize = 256;
const SETTLE_TICKS: usize = 64;
const VERSION: u32 = 1;

fn main() {
    let announces = announce_fixtures(COUNT);
    let dir = scenario_dir(NAME);
    std::fs::create_dir_all(&dir).expect("create scenario dir");

    let mut packets = String::new();
    for announce in &announces {
        packets.push_str(&to_hex(announce));
        packets.push('\n');
    }
    std::fs::write(dir.join("packets.hex"), packets).expect("write packets.hex");

    let manifest = format!(
        r#"{{
  "name": "{NAME}",
  "version": {VERSION},
  "description": "Ingest {n} distinct signed lxmf.delivery announces in order over one interface, then settle {SETTLE_TICKS} ticks.",
  "input": {{ "packets": "packets.hex", "encoding": "hex-per-line", "count": {n} }},
  "operations": {{ "ingest": "all-in-order", "settle_ticks": {SETTLE_TICKS} }},
  "expected": {{ "route_count": {n} }}
}}
"#,
        n = announces.len(),
    );
    std::fs::write(dir.join("manifest.json"), manifest).expect("write manifest.json");

    eprintln!(
        "wrote {} packets + manifest to {}",
        announces.len(),
        dir.display()
    );
}
