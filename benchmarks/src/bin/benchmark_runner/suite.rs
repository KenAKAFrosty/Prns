use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use benchmarks::{load_catalog, ResultRow, ScenarioId, ScenarioTopology, Subject, IMPLEMENTATIONS};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::arguments::SuiteArgs;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    scenario: ScenarioId,
    scenario_version: u32,
    initiator: &'static str,
    responder: &'static str,
    relay: Option<&'static str>,
}

impl Cell {
    fn subject(&self) -> Subject {
        Subject::Direct {
            initiator: self.initiator.into(),
            responder: self.responder.into(),
            relay: self.relay.map(str::to_string),
        }
    }

    fn subject_slug(&self) -> String {
        self.subject().file_slug()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScheduledSample {
    ordinal: usize,
    sample_index: u32,
    cell_index: usize,
}

#[derive(Serialize)]
struct SuiteEvidence {
    schema: u32,
    suite_id: String,
    source_commit: String,
    source_fingerprint: String,
    source_dirty: bool,
    samples_per_cell: u32,
    duration_ms: u64,
    selected_cells: usize,
    matrix_cells: usize,
    complete: bool,
    host: Option<String>,
    energy_available: bool,
    reference_verified: bool,
    started_unix_ms: u128,
    finished_unix_ms: u128,
    tool_versions: BTreeMap<String, String>,
    reference_proof: serde_json::Value,
    schedule: Vec<EvidenceSample>,
    files: BTreeMap<String, String>,
    failures: Vec<String>,
}

#[derive(Clone, serde::Deserialize, Serialize)]
struct EvidenceSample {
    ordinal: usize,
    sample_index: u32,
    cell: usize,
    scenario: String,
    initiator: String,
    responder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay: Option<String>,
    status: String,
    attempts: u32,
    startup_attempts: u32,
    startup_failures: u32,
    command: String,
    started_unix_ms: Option<u128>,
    finished_unix_ms: Option<u128>,
    exit_code: Option<i32>,
    log: String,
}

#[derive(serde::Deserialize)]
struct PreviousSuite {
    schedule: Vec<EvidenceSample>,
}

struct SampleExecution {
    status: &'static str,
    attempts: u32,
    startup_attempts: u32,
    startup_failures: u32,
    command: String,
    started_unix_ms: Option<u128>,
    finished_unix_ms: Option<u128>,
    exit_code: Option<i32>,
    failure: Option<String>,
}

fn retryable_startup_failure(
    successful: bool,
    measurement_started: bool,
    attempt: u32,
    max_attempts: u32,
) -> bool {
    !successful && !measurement_started && attempt < max_attempts
}

impl SampleExecution {
    fn resumed(previous: Option<&EvidenceSample>) -> Self {
        if let Some(previous) = previous {
            return Self {
                status: "resumed",
                attempts: previous.attempts,
                startup_attempts: previous.startup_attempts,
                startup_failures: previous.startup_failures,
                command: previous.command.clone(),
                started_unix_ms: previous.started_unix_ms,
                finished_unix_ms: previous.finished_unix_ms,
                exit_code: previous.exit_code,
                failure: None,
            };
        }
        Self {
            status: "resumed",
            attempts: 0,
            startup_attempts: 0,
            startup_failures: 0,
            command: String::new(),
            started_unix_ms: None,
            finished_unix_ms: None,
            exit_code: Some(0),
            failure: None,
        }
    }
}

fn matrix() -> Result<Vec<Cell>, String> {
    let catalog = load_catalog().map_err(|error| error.to_string())?;
    let mut cells = Vec::new();
    for manifest in catalog {
        match manifest.topology {
            ScenarioTopology::Direct => {
                for initiator in IMPLEMENTATIONS {
                    for responder in IMPLEMENTATIONS {
                        cells.push(Cell {
                            scenario: manifest.name,
                            scenario_version: manifest.version,
                            initiator,
                            responder,
                            relay: None,
                        });
                    }
                }
            }
            ScenarioTopology::Relay => {
                for relay in IMPLEMENTATIONS {
                    cells.push(Cell {
                        scenario: manifest.name,
                        scenario_version: manifest.version,
                        initiator: "benchmark-wire-driver",
                        responder: "benchmark-wire-driver",
                        relay: Some(relay),
                    });
                }
            }
        }
    }
    Ok(cells)
}

fn counterbalanced_schedule(cell_count: usize, samples: u32) -> Vec<ScheduledSample> {
    let canonical = (0..cell_count).collect::<Vec<_>>();
    let mut schedule = Vec::with_capacity(cell_count * samples as usize);
    for sample_index in 0..samples {
        let order = match sample_index % 3 {
            0 => canonical.clone(),
            1 => canonical.iter().rev().copied().collect(),
            _ => {
                let split = cell_count / 2;
                canonical[split..]
                    .iter()
                    .chain(&canonical[..split])
                    .copied()
                    .collect()
            }
        };
        for cell_index in order {
            schedule.push(ScheduledSample {
                ordinal: schedule.len() + 1,
                sample_index,
                cell_index,
            });
        }
    }
    schedule
}

pub(super) fn run(args: SuiteArgs) {
    let all_cells = matrix().unwrap_or_else(|reason| fail(&format!("catalog: {reason}")));
    if let Some(only) = &args.only_cells {
        if let Some(cell) = only.iter().find(|cell| **cell > all_cells.len()) {
            fail(&format!(
                "--only-cells contains {cell}, but the matrix has {} cells",
                all_cells.len()
            ));
        }
    }
    let selected = (0..all_cells.len())
        .filter(|index| {
            args.only_cells
                .as_ref()
                .is_none_or(|only| only.contains(&(index + 1)))
        })
        .collect::<BTreeSet<_>>();
    println!(
        "release suite: {} selected of {} cells × {} sample(s)",
        selected.len(),
        all_cells.len(),
        args.samples
    );
    println!("participants: Prns and compiled RNS 1.4.0 reference");
    println!(
        "matrix: endpoint scenarios use four pairings; relay scenarios use two relay subjects"
    );
    println!("isolation: one cell at a time; samples run in counterbalanced rounds");
    println!("pass rule: every selected cell must run and conform; energy is optional evidence");
    for (index, cell) in all_cells
        .iter()
        .enumerate()
        .filter(|(index, _)| selected.contains(index))
    {
        if let Some(relay) = cell.relay {
            println!("{:>2}. {:<28} relay={relay}", index + 1, cell.scenario,);
        } else {
            println!(
                "{:>2}. {:<28} initiator={} responder={}",
                index + 1,
                cell.scenario,
                cell.initiator,
                cell.responder
            );
        }
    }
    if args.dry_run {
        return;
    }
    if cfg!(debug_assertions) && !args.smoke {
        fail("release suite must run from target/release/benchmark_runner");
    }
    if !args.smoke && args.samples != 3 {
        fail("publishing-quality release suite requires exactly three samples");
    }
    if let Err(reason) = prepare_reference() {
        fail(&format!("compiled-reference preparation: {reason}"));
    }

    let suite_id = args
        .suite_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("prns-benchmark-suite-{suite_id}")));
    std::fs::create_dir_all(output.join("logs")).expect("create suite output");
    let source_commit = command_line("git", &["rev-parse", "HEAD"]).unwrap_or_default();
    if !is_full_sha(&source_commit) {
        fail("source identity is not a full 40-character Git SHA");
    }
    let source_dirty = command_line(
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .is_some_and(|status| !status.is_empty());
    let source_fingerprint = std::env::var("BENCHMARK_SOURCE_FINGERPRINT").unwrap_or_default();
    if source_fingerprint.len() != 64
        || !source_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        fail("source state is missing its SHA-256 fingerprint; use `cargo benchmark`");
    }
    let mut schedule = counterbalanced_schedule(all_cells.len(), args.samples)
        .into_iter()
        .filter(|sample| selected.contains(&sample.cell_index))
        .collect::<Vec<_>>();
    for (index, sample) in schedule.iter_mut().enumerate() {
        sample.ordinal = index + 1;
    }
    let previous = previous_samples(&output);

    let started_unix_ms = unix_ms();
    let mut evidence = SuiteEvidence {
        schema: 1,
        suite_id: suite_id.clone(),
        source_commit,
        source_fingerprint,
        source_dirty,
        samples_per_cell: args.samples,
        duration_ms: args.duration_ms,
        selected_cells: selected.len(),
        matrix_cells: all_cells.len(),
        complete: false,
        host: None,
        energy_available: false,
        reference_verified: false,
        started_unix_ms,
        finished_unix_ms: started_unix_ms,
        tool_versions: tool_versions(),
        reference_proof: reference_proof(),
        schedule: Vec::with_capacity(schedule.len()),
        files: result_hashes(&output),
        failures: Vec::new(),
    };
    write_suite_evidence(&output, &evidence);
    for scheduled in &schedule {
        let cell = &all_cells[scheduled.cell_index];
        let log_relative = format!(
            "logs/{:02}-sample-{}-{}.log",
            scheduled.cell_index + 1,
            scheduled.sample_index,
            cell.subject_slug()
        );
        let result = if sample_is_complete(
            &output,
            cell,
            scheduled.cell_index,
            scheduled.sample_index,
            &suite_id,
        ) {
            println!(
                "RESUME {}/{} sample={} {} {}",
                scheduled.ordinal,
                schedule.len(),
                scheduled.sample_index,
                cell.scenario,
                cell.subject_slug()
            );
            Ok(SampleExecution::resumed(
                previous.get(&(scheduled.cell_index + 1, scheduled.sample_index)),
            ))
        } else {
            run_sample(cell, &args, &suite_id, scheduled, &output, &log_relative)
        };
        let execution = match result {
            Ok(execution) => {
                if let Some(reason) = &execution.failure {
                    let failure = format!(
                        "cell={} sample={} {} {}: {reason}",
                        scheduled.cell_index + 1,
                        scheduled.sample_index,
                        cell.scenario,
                        cell.subject_slug()
                    );
                    eprintln!("FAIL {failure}");
                    evidence.failures.push(failure);
                } else {
                    println!(
                        "PASS {}/{} sample={} {} {}",
                        scheduled.ordinal,
                        schedule.len(),
                        scheduled.sample_index,
                        cell.scenario,
                        cell.subject_slug()
                    );
                }
                execution
            }
            Err(reason) => {
                let failure = format!(
                    "cell={} sample={} {} {}: {reason}",
                    scheduled.cell_index + 1,
                    scheduled.sample_index,
                    cell.scenario,
                    cell.subject_slug()
                );
                eprintln!("FAIL {failure}");
                evidence.failures.push(failure);
                SampleExecution {
                    status: "fail",
                    attempts: 1,
                    startup_attempts: 0,
                    startup_failures: 0,
                    command: String::new(),
                    started_unix_ms: None,
                    finished_unix_ms: None,
                    exit_code: None,
                    failure: Some(reason),
                }
            }
        };
        evidence.schedule.push(EvidenceSample {
            ordinal: scheduled.ordinal,
            sample_index: scheduled.sample_index,
            cell: scheduled.cell_index + 1,
            scenario: cell.scenario.to_string(),
            initiator: cell.initiator.into(),
            responder: cell.responder.into(),
            relay: cell.relay.map(str::to_string),
            status: execution.status.into(),
            attempts: execution.attempts,
            startup_attempts: execution.startup_attempts,
            startup_failures: execution.startup_failures,
            command: execution.command,
            started_unix_ms: execution.started_unix_ms,
            finished_unix_ms: execution.finished_unix_ms,
            exit_code: execution.exit_code,
            log: log_relative,
        });
        evidence.finished_unix_ms = unix_ms();
        evidence.files = result_hashes(&output);
        write_suite_evidence(&output, &evidence);
    }

    let validation = if args.smoke {
        Ok(ValidatedSuite::default())
    } else {
        validate_suite(
            &output,
            &all_cells,
            &selected,
            args.samples,
            &evidence.source_commit,
            &suite_id,
        )
    };
    let validated = match validation {
        Ok(validated) => validated,
        Err(reasons) => {
            evidence.failures.extend(reasons);
            ValidatedSuite::default()
        }
    };
    let complete =
        evidence.failures.is_empty() && (args.smoke || selected.len() == all_cells.len());
    evidence.complete = complete;
    evidence.host = validated.host;
    evidence.energy_available = validated.energy_available;
    evidence.reference_verified = validated.reference_verified;
    evidence.finished_unix_ms = unix_ms();
    evidence.files = result_hashes(&output);
    write_suite_evidence(&output, &evidence);
    println!(
        "SUMMARY selected={} matrix={} samples={} pass={} fail={} output={}",
        selected.len(),
        all_cells.len(),
        schedule.len(),
        schedule.len().saturating_sub(evidence.failures.len()),
        evidence.failures.len(),
        output.display()
    );
    if !complete {
        eprintln!(
            "RESUME with the same suite ID and output directory; completed samples are retained"
        );
        std::process::exit(1);
    }
}

fn write_suite_evidence(output: &Path, evidence: &SuiteEvidence) {
    let destination = output.join("suite.json");
    let temporary = output.join(".suite.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_string_pretty(evidence).expect("serialize suite evidence") + "\n",
    )
    .expect("write suite checkpoint");
    replace_checkpoint(&temporary, &destination).expect("install suite checkpoint");
}

#[cfg(not(windows))]
fn replace_checkpoint(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_checkpoint(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt as _;

    if !destination.exists() {
        return std::fs::rename(temporary, destination);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }
    let replaced = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replacement = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let success = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if success != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn run_sample(
    cell: &Cell,
    args: &SuiteArgs,
    suite_id: &str,
    scheduled: &ScheduledSample,
    output: &Path,
    log_relative: &str,
) -> Result<SampleExecution, String> {
    const MAX_STARTUP_ATTEMPTS: u32 = 3;
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("run")
        .arg(cell.scenario.as_str())
        .arg("--duration-ms")
        .arg(args.duration_ms.to_string())
        .arg("--sample-index")
        .arg(scheduled.sample_index.to_string())
        .arg("--run-id")
        .arg(format!("{suite_id}-{}", scheduled.cell_index + 1))
        .env("BENCHMARK_RESULTS_DIR", output)
        .env("BENCHMARK_SUITE_ID", suite_id);
    if let Some(relay) = cell.relay {
        command.arg("--relay").arg(relay);
    } else {
        command
            .arg("--initiator")
            .arg(cell.initiator)
            .arg("--responder")
            .arg(cell.responder);
    }
    if args.smoke {
        command.arg("--smoke");
    }
    let subject_option = cell
        .relay
        .map(|relay| format!("--relay {relay}"))
        .unwrap_or_else(|| {
            format!(
                "--initiator {} --responder {}",
                cell.initiator, cell.responder
            )
        });
    let command_text = format!(
        "benchmark_runner run {} {} --duration-ms {} --sample-index {} --run-id {}-{}{}",
        cell.scenario,
        subject_option,
        args.duration_ms,
        scheduled.sample_index,
        suite_id,
        scheduled.cell_index + 1,
        if args.smoke { " --smoke" } else { "" }
    );
    let started_unix_ms = unix_ms();
    let mut combined_stdout = String::new();
    let mut combined_stderr = String::new();
    let mut attempts = 0u32;
    let (exit_code, failure) = loop {
        attempts += 1;
        let child = command.output().map_err(|error| error.to_string())?;
        let stdout = String::from_utf8_lossy(&child.stdout);
        let stderr = String::from_utf8_lossy(&child.stderr);
        let measured = stdout.lines().any(|line| line.contains("MEASURE_DONE"));
        let successful = child.status.success();
        let retry_startup =
            retryable_startup_failure(successful, measured, attempts, MAX_STARTUP_ATTEMPTS);
        let stage = if measured {
            "measurement"
        } else {
            "sample-bootstrap"
        };
        let result = if successful { "pass" } else { "fail" };
        let attempt_line = if measured {
            format!("MEASUREMENT_ATTEMPT stage={stage} attempt={attempts} result={result}\n")
        } else {
            format!("STARTUP_ATTEMPT stage={stage} attempt={attempts} result={result}\n")
        };
        print!("{stdout}{attempt_line}");
        eprint!("{stderr}");
        combined_stdout.push_str(&format!("ATTEMPT {attempts}\n{stdout}{attempt_line}"));
        combined_stderr.push_str(&format!("ATTEMPT {attempts}\n{stderr}"));
        if retry_startup {
            println!(
                "STARTUP_RETRY next_attempt={} reason=child-exited-before-measurement",
                attempts + 1
            );
            combined_stdout.push_str(&format!(
                "STARTUP_RETRY next_attempt={} reason=child-exited-before-measurement\n",
                attempts + 1
            ));
            continue;
        }
        break (
            child.status.code(),
            (!successful).then(|| format!("child exited {}", child.status)),
        );
    };
    let finished_unix_ms = unix_ms();
    let log = format!("STDOUT\n{combined_stdout}\nSTDERR\n{combined_stderr}");
    std::fs::write(output.join(log_relative), log).map_err(|error| error.to_string())?;
    let startup_attempts = combined_stdout
        .lines()
        .filter(|line| line.contains("STARTUP_ATTEMPT"))
        .count() as u32;
    let startup_failures = combined_stdout
        .lines()
        .filter(|line| line.contains("STARTUP_ATTEMPT") && line.contains("result=fail"))
        .count() as u32;
    Ok(SampleExecution {
        status: if failure.is_some() { "fail" } else { "pass" },
        attempts,
        startup_attempts,
        startup_failures,
        command: command_text,
        started_unix_ms: Some(started_unix_ms),
        finished_unix_ms: Some(finished_unix_ms),
        exit_code,
        failure,
    })
}

fn sample_is_complete(
    root: &Path,
    cell: &Cell,
    cell_index: usize,
    sample: u32,
    suite_id: &str,
) -> bool {
    staged_path(root, cell)
        .and_then(|path| load_rows(&path).ok())
        .is_some_and(|rows| {
            rows.iter().any(|row| {
                row.sample_index == sample
                    && row.run_id == format!("{suite_id}-{}", cell_index + 1)
                    && row.scenario == cell.scenario.as_str()
                    && row.scenario_version == cell.scenario_version
                    && row.subject.file_slug() == cell.subject_slug()
                    && row.metric == "settled_clean"
                    && row.value == Some(1.0)
            })
        })
}

#[derive(Default)]
struct ValidatedSuite {
    host: Option<String>,
    energy_available: bool,
    reference_verified: bool,
}

fn validate_suite(
    root: &Path,
    cells: &[Cell],
    selected: &BTreeSet<usize>,
    samples: u32,
    commit: &str,
    suite_id: &str,
) -> Result<ValidatedSuite, Vec<String>> {
    let expected_samples = (0..samples).collect::<BTreeSet<_>>();
    let mut reasons = Vec::new();
    let mut hosts = BTreeSet::new();
    let mut energy_available = false;
    let mut reference_verified = false;
    let require_energy =
        std::env::var_os("BENCHMARK_REQUIRE_ENERGY").as_deref() == Some(std::ffi::OsStr::new("1"));
    for (index, cell) in cells
        .iter()
        .enumerate()
        .filter(|(index, _)| selected.contains(index))
    {
        let Some(path) = staged_path(root, cell) else {
            reasons.push(format!("missing result file for cell {}", index + 1));
            continue;
        };
        let rows = match load_rows(&path) {
            Ok(rows) => rows,
            Err(reason) => {
                reasons.push(reason);
                continue;
            }
        };
        let observed = rows
            .iter()
            .map(|row| row.sample_index)
            .collect::<BTreeSet<_>>();
        if observed != expected_samples {
            reasons.push(format!(
                "cell {} samples {observed:?}, expected {expected_samples:?}",
                index + 1
            ));
        }
        let expected_run_id = format!("{suite_id}-{}", index + 1);
        if rows.iter().any(|row| row.run_id != expected_run_id) {
            reasons.push(format!("cell {} contains a foreign run ID", index + 1));
        }
        if rows.iter().any(|row| {
            row.schema_version != benchmarks::RESULT_SCHEMA_VERSION
                || row.scenario != cell.scenario.as_str()
                || row.scenario_version != cell.scenario_version
                || row.subject.file_slug() != cell.subject_slug()
        }) {
            reasons.push(format!(
                "cell {} contains a foreign schema, scenario version, or subject",
                index + 1
            ));
        }
        if rows.iter().any(|row| row.commit != commit) {
            reasons.push(format!("cell {} is not bound to {commit}", index + 1));
        }
        if rows
            .iter()
            .filter(|row| row.metric == "settled_clean")
            .any(|row| row.value != Some(1.0))
        {
            reasons.push(format!(
                "cell {} contains non-conformant samples",
                index + 1
            ));
        }
        let energy_samples = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.metric.as_str(),
                    "package_joules_raw" | "whole_cell_package_joules_raw"
                ) && row.value.is_some()
            })
            .map(|row| row.sample_index)
            .collect::<BTreeSet<_>>();
        if require_energy && energy_samples != expected_samples {
            reasons.push(format!(
                "cell {} lacks required energy for samples {:?}",
                index + 1,
                expected_samples
                    .difference(&energy_samples)
                    .collect::<Vec<_>>()
            ));
        }
        if cell.initiator == "rns-1.4.0-compiled"
            || cell.responder == "rns-1.4.0-compiled"
            || cell.relay == Some("rns-1.4.0-compiled")
        {
            let proved_samples = rows
                .iter()
                .filter(|row| {
                    row.provenance
                        .get("reference_rns")
                        .is_some_and(|value| value == "1.4.0")
                        && row
                            .provenance
                            .get("reference_compiled")
                            .is_some_and(|value| value == "true")
                })
                .map(|row| row.sample_index)
                .collect::<BTreeSet<_>>();
            if proved_samples != expected_samples {
                reasons.push(format!(
                    "cell {} lacks compiled-reference proof for every sample",
                    index + 1
                ));
            }
        }
        hosts.extend(rows.iter().map(|row| row.host.clone()));
        energy_available |= rows.iter().any(|row| {
            matches!(
                row.metric.as_str(),
                "package_joules_raw" | "whole_cell_package_joules_raw"
            ) && row.value.is_some()
        });
        reference_verified |= rows.iter().any(|row| {
            row.provenance
                .get("reference_rns")
                .is_some_and(|value| value == "1.4.0")
                && row
                    .provenance
                    .get("reference_compiled")
                    .is_some_and(|value| value == "true")
        });
    }
    if hosts.len() != 1 {
        reasons.push(format!("suite must contain one host, found {hosts:?}"));
    }
    if selected.len() == cells.len() && !reference_verified {
        reasons.push("compiled RNS 1.4.0 proof is absent".into());
    }
    if reasons.is_empty() {
        Ok(ValidatedSuite {
            host: hosts.into_iter().next(),
            energy_available,
            reference_verified,
        })
    } else {
        Err(reasons)
    }
}

fn prepare_reference() -> Result<(), String> {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference");
    let python = if cfg!(windows) {
        reference.join(".venv/Scripts/python.exe")
    } else {
        reference.join(".venv/bin/python")
    };
    let status = Command::new(python)
        .arg(reference.join("compiled_reference.py"))
        .arg("--verify-only")
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("preparation exited {status}"))
}

fn staged_path(root: &Path, cell: &Cell) -> Option<PathBuf> {
    jsonl_files(root).into_iter().find(|path| {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(cell.scenario.as_str())
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
        if path.is_dir() && path.file_name().and_then(|name| name.to_str()) != Some("logs") {
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

fn result_hashes(root: &Path) -> BTreeMap<String, String> {
    jsonl_files(root)
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let relative = path.strip_prefix(root).ok()?.to_string_lossy().into_owned();
            Some((relative, format!("{:x}", Sha256::digest(bytes))))
        })
        .collect()
}

fn previous_samples(root: &Path) -> BTreeMap<(usize, u32), EvidenceSample> {
    std::fs::read_to_string(root.join("suite.json"))
        .ok()
        .and_then(|body| serde_json::from_str::<PreviousSuite>(&body).ok())
        .map(|suite| {
            suite
                .schedule
                .into_iter()
                .map(|sample| ((sample.cell, sample.sample_index), sample))
                .collect()
        })
        .unwrap_or_default()
}

fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time follows the Unix epoch")
        .as_millis()
}

fn tool_versions() -> BTreeMap<String, String> {
    #[cfg(windows)]
    let compiler = "cl";
    #[cfg(not(windows))]
    let compiler = "cc";
    let mut versions = ["cargo", "rustc", "uv", compiler]
        .into_iter()
        .filter_map(|tool| command_line(tool, &["--version"]).map(|version| (tool.into(), version)))
        .collect::<BTreeMap<_, _>>();
    if let Ok(flags) = std::env::var("BENCHMARK_BUILD_FLAGS") {
        versions.insert("rust_build_flags".into(), flags);
    }
    versions
}

fn reference_proof() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference/.object-cache/proof.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn fail(reason: &str) -> ! {
    eprintln!("FAIL {reason}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pre_measurement_failures_are_retryable() {
        assert!(retryable_startup_failure(false, false, 1, 3));
        assert!(retryable_startup_failure(false, false, 2, 3));
        assert!(!retryable_startup_failure(false, false, 3, 3));
        assert!(!retryable_startup_failure(false, true, 1, 3));
        assert!(!retryable_startup_failure(true, false, 1, 3));
    }

    fn temporary(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("prns-suite-{label}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn release_matrix_is_manifest_owned_and_complete() {
        let cells = matrix().expect("catalog-backed matrix");
        assert_eq!(cells.len(), 34);
        for scenario in load_catalog().expect("catalog") {
            let subjects = cells
                .iter()
                .filter(|cell| cell.scenario == scenario.name)
                .map(Cell::subject_slug)
                .collect::<BTreeSet<_>>();
            let expected = match scenario.topology {
                ScenarioTopology::Direct => 4,
                ScenarioTopology::Relay => 2,
            };
            assert_eq!(
                subjects.len(),
                expected,
                "{} has its complete topology matrix",
                scenario.name
            );
        }
        for scenario in [
            ScenarioId::RawTransportThroughput,
            ScenarioId::TransportResourceThroughput,
            ScenarioId::TransportResourceThroughputUnleashed,
        ] {
            let raw = cells
                .iter()
                .filter(|cell| cell.scenario == scenario)
                .collect::<Vec<_>>();
            assert_eq!(
                raw.iter()
                    .filter_map(|cell| cell.relay)
                    .collect::<BTreeSet<_>>(),
                IMPLEMENTATIONS.into_iter().collect()
            );
            assert!(raw.iter().all(|cell| {
                cell.initiator == "benchmark-wire-driver"
                    && cell.responder == "benchmark-wire-driver"
            }));
        }
    }

    #[test]
    fn three_rounds_are_counterbalanced_and_complete() {
        let schedule = counterbalanced_schedule(34, 3);
        assert_eq!(schedule.len(), 102);
        for sample in 0..3 {
            let cells = schedule
                .iter()
                .filter(|entry| entry.sample_index == sample)
                .map(|entry| entry.cell_index)
                .collect::<BTreeSet<_>>();
            assert_eq!(cells, (0..34).collect());
        }
        assert_eq!(schedule[0].cell_index, 0);
        assert_eq!(schedule[34].cell_index, 33);
        assert_eq!(schedule[68].cell_index, 17);
    }

    #[test]
    fn resume_skips_only_an_exact_conformant_sample() {
        let root = temporary("resume");
        let cell = matrix().expect("matrix").remove(0);
        let path = root
            .join("test-host")
            .join(cell.scenario.as_str())
            .join(format!("{}.jsonl", cell.subject_slug()));
        std::fs::create_dir_all(path.parent().expect("result parent")).expect("result tree");
        let row = benchmarks::ResultRow {
            schema_version: benchmarks::RESULT_SCHEMA_VERSION,
            run_id: "suite-1-1".into(),
            sample_index: 0,
            scenario: cell.scenario.to_string(),
            scenario_version: cell.scenario_version,
            subject: cell.subject(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            toolchain: "rustc test".into(),
            host: "test-host".into(),
            axis: benchmarks::Axis::Conformance,
            metric: "settled_clean".into(),
            value: Some(1.0),
            unit: "bool".into(),
            device_id: None,
            submitter_id: None,
            provenance: BTreeMap::new(),
        };
        std::fs::write(&path, serde_json::to_string(&row).expect("row JSON") + "\n")
            .expect("result row");
        assert!(sample_is_complete(&root, &cell, 0, 0, "suite-1"));
        assert!(!sample_is_complete(&root, &cell, 0, 1, "suite-1"));
        assert!(!sample_is_complete(&root, &cell, 0, 0, "suite-2"));
        std::fs::remove_dir_all(root).expect("remove owned resume fixture");
    }

    #[test]
    fn relay_resume_requires_the_exact_relay_identity() {
        let root = temporary("relay-resume");
        let cell = matrix()
            .expect("matrix")
            .into_iter()
            .find(|cell| {
                cell.scenario == ScenarioId::RawTransportThroughput
                    && cell.relay == Some("personal-rns")
            })
            .expect("Prns relay cell");
        let path = root
            .join("test-host")
            .join(cell.scenario.as_str())
            .join(format!("{}.jsonl", cell.subject_slug()));
        std::fs::create_dir_all(path.parent().expect("result parent")).expect("result tree");
        let mut row = benchmarks::ResultRow {
            schema_version: benchmarks::RESULT_SCHEMA_VERSION,
            run_id: "suite-1-21".into(),
            sample_index: 0,
            scenario: cell.scenario.to_string(),
            scenario_version: cell.scenario_version,
            subject: benchmarks::Subject::Direct {
                initiator: "benchmark-wire-driver".into(),
                responder: "benchmark-wire-driver".into(),
                relay: Some("rns-1.4.0-compiled".into()),
            },
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            toolchain: "rustc test".into(),
            host: "test-host".into(),
            axis: benchmarks::Axis::Conformance,
            metric: "settled_clean".into(),
            value: Some(1.0),
            unit: "bool".into(),
            device_id: None,
            submitter_id: None,
            provenance: BTreeMap::new(),
        };
        std::fs::write(&path, serde_json::to_string(&row).unwrap() + "\n").unwrap();
        assert!(!sample_is_complete(&root, &cell, 20, 0, "suite-1"));
        row.subject = cell.subject();
        std::fs::write(&path, serde_json::to_string(&row).unwrap() + "\n").unwrap();
        assert!(sample_is_complete(&root, &cell, 20, 0, "suite-1"));
        std::fs::remove_dir_all(root).expect("remove relay resume fixture");
    }

    #[test]
    fn resumed_evidence_retains_the_original_attempt_and_command() {
        let previous = EvidenceSample {
            ordinal: 1,
            sample_index: 0,
            cell: 1,
            scenario: "single-packet-throughput".into(),
            initiator: "personal-rns".into(),
            responder: "personal-rns".into(),
            relay: None,
            status: "pass".into(),
            attempts: 1,
            startup_attempts: 4,
            startup_failures: 0,
            command: "benchmark_runner run ...".into(),
            started_unix_ms: Some(10),
            finished_unix_ms: Some(20),
            exit_code: Some(0),
            log: "logs/sample.log".into(),
        };
        let resumed = SampleExecution::resumed(Some(&previous));
        assert_eq!(resumed.status, "resumed");
        assert_eq!(resumed.attempts, 1);
        assert_eq!(resumed.startup_attempts, 4);
        assert_eq!(resumed.command, previous.command);
        assert_eq!(resumed.started_unix_ms, Some(10));
    }

    #[test]
    fn suite_checkpoint_exists_before_completion_and_updates_atomically() {
        let root = temporary("checkpoint");
        std::fs::create_dir_all(&root).expect("checkpoint directory");
        let mut evidence = SuiteEvidence {
            schema: 1,
            suite_id: "suite-1".into(),
            source_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            source_fingerprint: "a".repeat(64),
            source_dirty: true,
            samples_per_cell: 3,
            duration_ms: 30_000,
            selected_cells: 34,
            matrix_cells: 34,
            complete: false,
            host: None,
            energy_available: false,
            reference_verified: false,
            started_unix_ms: 10,
            finished_unix_ms: 10,
            tool_versions: BTreeMap::new(),
            reference_proof: serde_json::Value::Null,
            schedule: Vec::new(),
            files: BTreeMap::new(),
            failures: Vec::new(),
        };
        write_suite_evidence(&root, &evidence);
        assert!(root.join("suite.json").is_file());
        assert!(!root.join(".suite.json.tmp").exists());

        evidence.schedule.push(EvidenceSample {
            ordinal: 1,
            sample_index: 0,
            cell: 1,
            scenario: "single-packet-throughput".into(),
            initiator: "personal-rns".into(),
            responder: "personal-rns".into(),
            relay: None,
            status: "pass".into(),
            attempts: 1,
            startup_attempts: 0,
            startup_failures: 0,
            command: "benchmark_runner run ...".into(),
            started_unix_ms: Some(11),
            finished_unix_ms: Some(12),
            exit_code: Some(0),
            log: "logs/01.log".into(),
        });
        write_suite_evidence(&root, &evidence);
        let checkpoint: PreviousSuite = serde_json::from_str(
            &std::fs::read_to_string(root.join("suite.json")).expect("checkpoint body"),
        )
        .expect("resumable checkpoint");
        assert_eq!(checkpoint.schedule.len(), 1);
        assert_eq!(checkpoint.schedule[0].attempts, 1);
        assert!(!root.join(".suite.json.tmp").exists());
        std::fs::remove_dir_all(root).expect("remove checkpoint fixture");
    }
}
