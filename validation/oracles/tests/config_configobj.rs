use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prns_config::configobj::{self, Section, Value};

mod support;

fn canon(section: &Section) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in &section.scalars {
        let json = match value {
            Value::Scalar(text) => serde_json::Value::String(text.clone()),
            Value::List(items) => serde_json::Value::Array(
                items
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        };
        map.insert(key.clone(), json);
    }
    for (name, sub) in &section.sections {
        map.insert(name.clone(), canon(sub));
    }
    serde_json::Value::Object(map)
}

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("python/configobj_oracle.py")
}

fn run_oracle(python: &std::ffi::OsStr, corpus: &[String]) -> Vec<serde_json::Value> {
    let input = serde_json::to_string(corpus).expect("corpus serializes");
    let mut child = Command::new(python)
        .arg(oracle_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oracle python");
    child
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(input.as_bytes())
        .expect("write corpus to oracle");
    let output = child.wait_with_output().expect("oracle runs");
    assert!(
        output.status.success(),
        "oracle exited with failure:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle emits json array")
}

fn compare(python: &std::ffi::OsStr, corpus: &[String], seed: Option<u64>) {
    let oracle = run_oracle(python, corpus);
    assert_eq!(
        oracle.len(),
        corpus.len(),
        "oracle returned one result per config"
    );
    for (index, (text, verdict)) in corpus.iter().zip(&oracle).enumerate() {
        let provenance = seed.map_or_else(
            || format!("case {index}"),
            |seed| format!("seed {seed:#018x}, case {index}"),
        );
        let ours = configobj::parse(text);
        let verdict = verdict.as_object().expect("oracle result is an object");
        match (ours, verdict.get("ok"), verdict.get("error")) {
            (Ok(section), Some(tree), _) => {
                let mine = canon(&section);
                assert_eq!(
                    &mine, tree,
                    "{provenance} parsed differently from ConfigObj:\n--- input ---\n{text}\n--- ours ---\n{mine:#}\n--- oracle ---\n{tree:#}"
                );
            }
            (Err(error), _, Some(oracle_error)) => {
                let _ = (error, oracle_error);
            }
            (Ok(section), None, Some(oracle_error)) => panic!(
                "{provenance}: we accepted but ConfigObj rejected ({oracle_error}):\n{text}\nours={:#}",
                canon(&section)
            ),
            (Err(error), Some(tree), None) => panic!(
                "{provenance}: ConfigObj accepted but we rejected ({error}):\n{text}\noracle={tree:#}"
            ),
            (_, None, None) => {
                panic!("{provenance}: oracle result had neither ok nor error:\n{text}")
            }
        }
    }
}

const CURATED: &[&str] = &[
    "[reticulum]\nshare_instance = Yes\n[interfaces]\n  [[Hub]]\n    type = TCPClientInterface\n    target_host = h.example.com\n    target_port = 4965\n    devices = eth0, wlan0\n",
    "[x]\nk = \"a # b\"\n",
    "[x]\nk = value  # trailing comment\n",
    "# whole line comment\n[x]\nk = v\n",
    "[x]\nlist = a, b, c\n",
    "[x]\none = a,\n",
    "[x]\nempty =\n",
    "[x]\nspaces = a b c\n",
    "[x]\nsingle = 'quoted'\n",
    "[x]\ndouble = \"quoted\"\n",
    "[x]\nqcomma = \"a, b\", c\n",
    "[x]\nqhash = \"a # b\", c\n",
    "[a]\n[[b]]\n[[[c]]]\nk = deep\n",
    "[interfaces]\n  [[Radio]]\n    type = RNodeMultiInterface\n    [[[Fast]]]\n      vport = 0\n      frequency = 867200000\n    [[[Slow]]]\n      vport = 1\n",
    "[x]\nzero = 0\nyes = Yes\nno = off\n",
    "[x]\npath = /dev/ttyUSB0\nhostport = host:4965\n",
    "[a]\nk1 = v1\n[b]\nk2 = v2\n[c]\nk3 = v3\n",
];

const ADVERSARIAL: &[&str] = &[
    "[[orphan]]\nk = v\n",
    "[parent]\n[[[skipped]]]\nk = v\n",
    "[broken]]\nk = v\n",
    "[[broken]\nk = v\n",
    "[x]\nk = 'unterminated\n",
    "[x]\nmissing equals\n",
    "[x]\nk = first\nk = second\n",
    "[x]\nk = first\n[x]\nk = second\n",
    "[x]\n[[child]]\nk = first\n[[child]]\nk = second\n",
    "[x]\nplain = alpha,beta\n",
    "[x]\nleading = ,alpha\n",
    "[x]\ndoubled = alpha,,beta\n",
    "[x]\nquoted = 'alpha, beta', \"gamma # delta\" # tail\n",
    "[x]\ncomment = value#attached\n",
    "[x]\nempty =\nblank =   \nquoted_empty = \"\"\n",
    "[unicode]\nключ = значение\nemoji = mesh-🛰️\n",
    "[control]\nvalue = before\u{0}after\n",
    "\t[x]\r\n\tkey\t=\tvalue\t\r\n",
    "[ x ]\n spaced key = spaced value \n",
    "root = value\n[x]\nk = v\n",
    "[x]\nk = \\\"escaped\\\"\n",
    "[x]\nk = \"backslash\\\\tail\"\n",
    "[x]\nk = one, 'two,three', four # five\n",
];

struct Generator {
    state: u64,
    counter: u64,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Generator {
            state: seed,
            counter: 0,
        }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state >> 33
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn ident(&mut self, prefix: char) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    fn word(&mut self) -> String {
        let length = 1 + self.below(6);
        (0..length)
            .map(|_| {
                let pool = b"abcdefghijklmnopqrstuvwxyz0123456789";
                pool[self.below(pool.len() as u64) as usize] as char
            })
            .collect()
    }

    fn value(&mut self) -> String {
        match self.below(9) {
            0 => self.word(),
            1 => format!("{}, {}", self.word(), self.word()),
            2 => format!("{},", self.word()),
            3 => format!("{} {} {}", self.word(), self.word(), self.word()),
            4 => format!("'{}'", self.word()),
            5 => format!("\"{}\"", self.word()),
            6 => format!("{}  # {}", self.word(), self.word()),
            7 => format!("\"{}, {}\", {}", self.word(), self.word(), self.word()),
            _ => format!("\"{} # {}\"", self.word(), self.word()),
        }
    }

    fn body(&mut self, bracket_depth: usize, out: &mut String) {
        let indent = "  ".repeat(bracket_depth);
        for _ in 0..self.below(4) {
            let key = self.ident('k');
            let value = self.value();
            out.push_str(&format!("{indent}{key} = {value}\n"));
        }
        if bracket_depth < 3 {
            for _ in 0..self.below(3) {
                let name = self.ident('s');
                let brackets = bracket_depth + 1;
                out.push_str(&format!(
                    "{indent}{}{name}{}\n",
                    "[".repeat(brackets),
                    "]".repeat(brackets)
                ));
                self.body(brackets, out);
            }
        }
    }

    fn config(&mut self) -> String {
        let mut out = String::new();
        for _ in 0..1 + self.below(4) {
            let name = self.ident('s');
            out.push_str(&format!("[{name}]\n"));
            self.body(1, &mut out);
        }
        out
    }
}

fn generated_corpus(count: usize, seed: u64) -> Vec<String> {
    let mut generator = Generator::new(seed);
    (0..count).map(|_| generator.config()).collect()
}

#[test]
fn matches_configobj_on_curated_dialect_corners() {
    let python = support::required_python("SMOKE_PYTHON");
    let corpus: Vec<String> = CURATED.iter().map(|text| text.to_string()).collect();
    compare(&python, &corpus, None);
}

#[test]
fn matches_configobj_on_generated_configs() {
    let python = support::required_python("SMOKE_PYTHON");
    const SEED: u64 = 0x5eed_1337;
    compare(&python, &generated_corpus(250, SEED), Some(SEED));
}

#[test]
fn matches_configobj_on_adversarial_boundaries() {
    let python = support::required_python("SMOKE_PYTHON");
    let corpus = ADVERSARIAL
        .iter()
        .map(|text| text.to_string())
        .collect::<Vec<_>>();
    compare(&python, &corpus, None);
}
