use std::path::{Path, PathBuf};
use std::process::Command;

use benchmarks::{write_rows, ResultRow};

use super::arguments::SuiteArgs;

#[derive(Clone)]
struct Cell {
    scenario: &'static str,
    initiator: &'static str,
    responder: &'static str,
}

impl Cell {
    fn subject_slug(&self) -> String {
        format!("{}--{}", self.initiator, self.responder)
    }
}

const CORE: &[&str] = &["personal-rns", "rns-1.4.0-compiled"];
const DIRECT_SCENARIOS: &[&str] = &[
    "single-packet-throughput",
    "link-message-throughput",
    "request-response",
    "resource-max-segment",
    "resource-64mib-stream",
];

fn matrix() -> Vec<Cell> {
    let mut cells = Vec::new();
    for scenario in DIRECT_SCENARIOS {
        for initiator in CORE {
            for responder in CORE {
                cells.push(Cell {
                    scenario,
                    initiator,
                    responder,
                });
            }
        }
    }
    cells
}

pub(super) fn run(args: SuiteArgs) {
    let all_cells = matrix();
    if let Some(only) = &args.only_cells {
        if let Some(cell) = only.iter().find(|cell| **cell > all_cells.len()) {
            eprintln!(
                "FAIL --only-cells contains {cell}, but the matrix has {} cells",
                all_cells.len()
            );
            std::process::exit(2);
        }
    }
    let cells = all_cells
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            args.only_cells
                .as_ref()
                .is_none_or(|only| only.contains(&(index + 1)))
        })
        .collect::<Vec<_>>();
    println!(
        "release suite: {} selected of {} cells × {} sample(s)",
        cells.len(),
        all_cells.len(),
        args.samples
    );
    println!("participants: Prns and compiled RNS 1.4.0 reference");
    println!("matrix: every scenario runs all four initiator/responder pairings");
    println!("pass rule: every selected cell must run and conform; energy is optional evidence");
    for (index, cell) in &cells {
        println!(
            "{:>2}. {:<28} initiator={} responder={}",
            index + 1,
            cell.scenario,
            cell.initiator,
            cell.responder
        );
    }
    if args.dry_run {
        return;
    }
    if cfg!(debug_assertions) && !args.smoke {
        eprintln!("FAIL release suite must run from target/release/benchmark_runner");
        std::process::exit(2);
    }
    if !args.smoke && args.samples != 3 {
        eprintln!("FAIL publishing release suite requires exactly three samples");
        std::process::exit(2);
    }
    if let Err(reason) = prepare_reference() {
        eprintln!("FAIL compiled-reference preparation: {reason}");
        std::process::exit(1);
    }

    let suite_id = uuid::Uuid::new_v4().to_string();
    let staging = std::env::temp_dir().join(format!("prns-benchmark-suite-{suite_id}"));
    std::fs::create_dir_all(&staging).expect("create suite staging directory");
    let (mut passed, mut failed) = (0u32, 0u32);
    for (index, cell) in &cells {
        let run_id = format!("{suite_id}-{index}");
        let result = run_cell(cell, &args, &run_id, &staging);
        match result {
            Ok(rows) => {
                if !args.smoke {
                    let first = rows.first().expect("completed cell has rows");
                    write_rows(
                        &first.host,
                        &first.scenario,
                        &first.subject.file_slug(),
                        &rows,
                    );
                }
                passed += 1;
                println!(
                    "PASS {}/{} {} {}",
                    index + 1,
                    all_cells.len(),
                    cell.scenario,
                    cell.subject_slug()
                );
            }
            Err(reason) => {
                failed += 1;
                eprintln!(
                    "FAIL {}/{} {} {}: {reason}",
                    index + 1,
                    all_cells.len(),
                    cell.scenario,
                    cell.subject_slug()
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    println!(
        "SUMMARY selected={} matrix={} pass={passed} fail={failed}",
        cells.len(),
        all_cells.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
}

fn prepare_reference() -> Result<(), String> {
    // `build.sh` owns dependency installation and cache warming as the normal user. Release
    // measurement commonly runs under sudo for powermetrics; only verify the locked, warmed
    // environment here so a benchmark cannot leave root-owned venv/cache files behind.
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference");
    let status = Command::new(reference.join(".venv/bin/python"))
        .arg(reference.join("compiled_reference.py"))
        .arg("--verify-only")
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("preparation exited {status}"))
}

fn run_cell(
    cell: &Cell,
    args: &SuiteArgs,
    run_id: &str,
    staging: &Path,
) -> Result<Vec<ResultRow>, String> {
    for sample in 0..args.samples {
        let mut child_attempts = 0u32;
        loop {
            let mut command =
                Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
            command
                .arg("run")
                .arg(cell.scenario)
                .arg("--initiator")
                .arg(cell.initiator)
                .arg("--responder")
                .arg(cell.responder)
                .arg("--duration-ms")
                .arg(args.duration_ms.to_string())
                .arg("--sample-index")
                .arg(sample.to_string())
                .arg("--run-id")
                .arg(run_id)
                .env("BENCHMARK_RESULTS_DIR", staging);
            if args.smoke {
                command.arg("--smoke");
            }
            let output = command.output().map_err(|error| error.to_string())?;
            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = format!("{stdout}\n{stderr}");
                if child_attempts < 2 && transient_child_failure(&detail) {
                    child_attempts += 1;
                    eprintln!(
                        "RETRY {} sample={sample}: transient participant startup/result race ({child_attempts}/2)",
                        cell.subject_slug()
                    );
                    continue;
                }
                return Err(format!(
                    "child exited {}\n{}\n{}",
                    output.status, stdout, stderr
                ));
            }
            if args.smoke {
                break;
            }
            if staged_path(staging, cell).is_none() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Err(format!("successful child wrote no staged result\n{stdout}"));
            }
            break;
        }
    }
    if args.smoke {
        return Ok(Vec::new());
    }
    let path = staged_path(staging, cell)
        .ok_or_else(|| "completed cell has no staged file".to_string())?;
    let rows = load_rows(&path)?;
    let samples = rows
        .iter()
        .map(|row| row.sample_index)
        .collect::<std::collections::BTreeSet<_>>();
    if samples.len() != args.samples as usize {
        return Err(format!(
            "expected {} samples, found {}",
            args.samples,
            samples.len()
        ));
    }
    if rows
        .iter()
        .filter(|row| row.metric == "settled_clean")
        .any(|row| row.value != Some(1.0))
    {
        return Err("one or more samples were non-conformant".into());
    }
    Ok(rows)
}

fn transient_child_failure(output: &str) -> bool {
    [
        "no announce heard",
        "link did not establish",
        "no \"READY\" line within",
        "no \"RESULT\" line within",
    ]
    .iter()
    .any(|marker| output.contains(marker))
}

fn staged_path(root: &Path, cell: &Cell) -> Option<PathBuf> {
    jsonl_files(root).into_iter().find(|path| {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(cell.scenario)
            && path.file_stem().and_then(|name| name.to_str()) == Some(&cell.subject_slug())
    })
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(jsonl_files(&path));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files
}

fn load_rows(path: &Path) -> Result<Vec<ResultRow>, String> {
    std::fs::read_to_string(path)
        .map_err(|error| error.to_string())?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| format!("parse {}: {error}", path.display()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_matrix_is_five_scenarios_by_four_pairings() {
        let cells = matrix();
        assert_eq!(cells.len(), 20);
        for scenario in DIRECT_SCENARIOS {
            let subjects = cells
                .iter()
                .filter(|cell| cell.scenario == *scenario)
                .map(Cell::subject_slug)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(subjects.len(), 4, "{scenario} has a complete 2×2 matrix");
        }
    }
}
