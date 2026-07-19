use std::path::{Path, PathBuf};

use super::results_dir;
use super::schema::ResultRow;

pub fn write_rows(host: &str, scenario: &str, impl_slug: &str, rows: &[ResultRow]) {
    let dir = results_dir().join(host).join(scenario);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let mut body = String::new();
    for row in rows {
        body.push_str(&serde_json::to_string(row).expect("serialize result row"));
        body.push('\n');
    }
    let path = dir.join(format!("{impl_slug}.jsonl"));
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub fn load_all_rows() -> Vec<ResultRow> {
    let mut rows = Vec::new();
    for jsonl in jsonl_files(&results_dir()) {
        let text = std::fs::read_to_string(&jsonl)
            .unwrap_or_else(|e| panic!("read {}: {e}", jsonl.display()));
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let row: ResultRow = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("parse row in {}: {e}", jsonl.display()));
            rows.push(row);
        }
    }
    rows
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(jsonl_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out
}
