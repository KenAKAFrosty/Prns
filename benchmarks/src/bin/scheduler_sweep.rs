#![expect(clippy::expect_used)]

use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use personal_rns::runtime::{SchedulerPolicy, SchedulerPolicyError, SchedulerPolicyInput};

fn main() {
    if let Err(error) = run() {
        eprintln!("scheduler sweep failed: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), SweepError> {
    let arguments = SweepArguments::parse()?;
    let probe = arguments.probe_path()?;
    let candidates = selected_candidates(arguments.mode, &arguments.only)?;
    let candidate_count = candidates.len();
    let sample_count = arguments.samples.get();
    let run_count = candidate_count.saturating_mul(sample_count);
    let mut completed_runs = 0usize;
    let mut observations: Vec<Vec<Observation>> = (0..candidate_count)
        .map(|_| Vec::with_capacity(sample_count))
        .collect();

    for sample in 1..=sample_count {
        let mut order: Vec<_> = (0..candidate_count).collect();
        if sample % 2 == 0 {
            order.reverse();
        }
        for index in order {
            let candidate = &candidates[index];
            completed_runs += 1;
            eprintln!(
                "running {completed_runs}/{run_count}: {} sample {sample}/{sample_count}",
                candidate.name
            );
            let observation = run_probe_with_retries(&probe, candidate, sample, arguments.retries)?;
            println!(
                "SAMPLE candidate={} sample={} idle_p50_micros={} idle_p99_micros={} mixed_p50_micros={} mixed_p99_micros={} resource_micros={} cpu_micros={} voluntary_context_switches={} involuntary_context_switches={} minor_page_faults={} major_page_faults={}",
                candidate.name,
                sample,
                observation.idle_p50_micros,
                observation.idle_p99_micros,
                observation.mixed_p50_micros,
                observation.mixed_p99_micros,
                observation.resource_micros,
                observation.cpu_micros,
                observation.voluntary_context_switches,
                observation.involuntary_context_switches,
                observation.minor_page_faults,
                observation.major_page_faults,
            );
            observations[index].push(observation);
        }
    }

    let summaries: Vec<_> = candidates
        .into_iter()
        .zip(observations)
        .map(|(candidate, observations)| CandidateSummary {
            candidate,
            observation: Observation::median(observations),
        })
        .collect();

    for summary in &summaries {
        print_summary("SUMMARY", summary);
    }

    let mut frontier: Vec<_> = summaries
        .iter()
        .filter(|candidate| {
            !summaries.iter().any(|other| {
                other.candidate.name != candidate.candidate.name
                    && other.observation.dominates(candidate.observation)
            })
        })
        .collect();
    frontier.sort_by_key(|summary| summary.observation.mixed_p99_micros);
    for summary in frontier {
        print_summary("PARETO", summary);
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum SweepMode {
    Quick,
    Coarse,
}

struct SweepArguments {
    mode: SweepMode,
    samples: NonZeroUsize,
    retries: usize,
    probe: Option<PathBuf>,
    only: HashSet<String>,
}

impl SweepArguments {
    fn parse() -> Result<Self, SweepError> {
        let mut mode = SweepMode::Quick;
        let mut samples = NonZeroUsize::MIN;
        let mut retries = 1usize;
        let mut probe = None;
        let mut only = HashSet::new();
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--quick" => mode = SweepMode::Quick,
                "--coarse" => mode = SweepMode::Coarse,
                "--samples" => {
                    let value = next_argument(&argument, &mut arguments)?;
                    samples = value
                        .parse::<NonZeroUsize>()
                        .map_err(|_| SweepError::InvalidPositiveInteger { argument, value })?;
                }
                "--probe" => {
                    probe = Some(PathBuf::from(next_argument(&argument, &mut arguments)?));
                }
                "--retries" => {
                    let value = next_argument(&argument, &mut arguments)?;
                    retries = value
                        .parse::<usize>()
                        .map_err(|_| SweepError::InvalidInteger { argument, value })?;
                }
                "--only" => {
                    let value = next_argument(&argument, &mut arguments)?;
                    only.extend(
                        value
                            .split(',')
                            .filter(|candidate| !candidate.is_empty())
                            .map(str::to_owned),
                    );
                }
                _ => return Err(SweepError::UnknownArgument(argument)),
            }
        }
        Ok(Self {
            mode,
            samples,
            retries,
            probe,
            only,
        })
    }

    fn probe_path(&self) -> Result<PathBuf, SweepError> {
        let path = match &self.probe {
            Some(path) => path.clone(),
            None => std::env::current_exe()
                .map_err(SweepError::CurrentExecutable)?
                .with_file_name(format!(
                    "scheduler_hol_probe{}",
                    std::env::consts::EXE_SUFFIX
                )),
        };
        if !path.is_file() {
            return Err(SweepError::ProbeNotFound(path));
        }
        Ok(path)
    }
}

fn next_argument(
    argument: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<String, SweepError> {
    arguments
        .next()
        .ok_or_else(|| SweepError::MissingValue(argument.to_owned()))
}

#[derive(Clone, Copy)]
enum WorkerSelection {
    Auto,
    Fixed(NonZeroUsize),
}

impl WorkerSelection {
    fn fixed(workers: usize) -> Self {
        Self::Fixed(NonZeroUsize::new(workers).expect("candidate worker count is positive"))
    }
}

impl fmt::Display for WorkerSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Fixed(workers) => workers.fmt(formatter),
        }
    }
}

#[derive(Clone)]
struct Candidate {
    name: String,
    policy: SchedulerPolicy,
    workers: WorkerSelection,
}

#[derive(Clone, Copy)]
struct PolicyPoint {
    turn_work: usize,
    completion_batch: usize,
    inbound_total: usize,
    inbound_per_lane: usize,
    command_batch: usize,
    owed_work_batch: usize,
    hot_turns: usize,
    worker_hot_idle_turns: usize,
    worker_spin_idle_turns: usize,
    interactive_batch: usize,
}

impl PolicyPoint {
    fn production() -> Self {
        let policy = SchedulerPolicy::production();
        Self {
            turn_work: policy.turn_work(),
            completion_batch: policy.completion_batch(),
            inbound_total: policy.inbound_total(),
            inbound_per_lane: policy.inbound_per_lane(),
            command_batch: policy.command_batch(),
            owed_work_batch: policy.owed_work_batch(),
            hot_turns: policy.hot_turns(),
            worker_hot_idle_turns: policy.worker_hot_idle_turns(),
            worker_spin_idle_turns: policy.worker_spin_idle_turns(),
            interactive_batch: policy.interactive_batch(),
        }
    }

    fn policy(self) -> Result<SchedulerPolicy, SchedulerPolicyError> {
        SchedulerPolicy::new(SchedulerPolicyInput {
            turn_work: positive(self.turn_work),
            completion_batch: positive(self.completion_batch),
            inbound_total: positive(self.inbound_total),
            inbound_per_lane: positive(self.inbound_per_lane),
            command_batch: positive(self.command_batch),
            owed_work_batch: positive(self.owed_work_batch),
            hot_turns: self.hot_turns,
            worker_hot_idle_turns: self.worker_hot_idle_turns,
            worker_spin_idle_turns: self.worker_spin_idle_turns,
            interactive_batch: positive(self.interactive_batch),
        })
    }
}

fn positive(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("candidate policy value is positive")
}

fn selected_candidates(
    mode: SweepMode,
    only: &HashSet<String>,
) -> Result<Vec<Candidate>, SweepError> {
    let mut candidates = candidates(mode)?;
    if only.is_empty() {
        return Ok(candidates);
    }
    let available: HashSet<_> = candidates
        .iter()
        .map(|candidate| candidate.name.as_str())
        .collect();
    if let Some(name) = only.iter().find(|name| !available.contains(name.as_str())) {
        return Err(SweepError::UnknownCandidate(name.clone()));
    }
    candidates.retain(|candidate| only.contains(&candidate.name));
    Ok(candidates)
}

fn candidates(mode: SweepMode) -> Result<Vec<Candidate>, SweepError> {
    let mut candidates = Vec::new();
    add_candidate(
        &mut candidates,
        "production-auto",
        WorkerSelection::Auto,
        |_| {},
    )?;
    for workers in [4, 6, 8] {
        add_candidate(
            &mut candidates,
            &format!("workers-{workers}"),
            WorkerSelection::fixed(workers),
            |_| {},
        )?;
    }
    add_candidate(&mut candidates, "turn-32", WorkerSelection::Auto, |point| {
        point.turn_work = 32;
    })?;
    add_candidate(&mut candidates, "turn-96", WorkerSelection::Auto, |point| {
        point.turn_work = 96;
    })?;
    add_candidate(
        &mut candidates,
        "hot-turns-4",
        WorkerSelection::Auto,
        |point| point.hot_turns = 4,
    )?;
    add_candidate(
        &mut candidates,
        "hot-turns-64",
        WorkerSelection::Auto,
        |point| point.hot_turns = 64,
    )?;
    add_candidate(
        &mut candidates,
        "idle-cold",
        WorkerSelection::Auto,
        |point| {
            point.worker_hot_idle_turns = 0;
            point.worker_spin_idle_turns = 0;
        },
    )?;
    add_candidate(
        &mut candidates,
        "spin-2048",
        WorkerSelection::Auto,
        |point| {
            point.worker_spin_idle_turns = 2_048;
        },
    )?;
    add_candidate(
        &mut candidates,
        "spin-32768",
        WorkerSelection::Auto,
        |point| {
            point.worker_spin_idle_turns = 32_768;
        },
    )?;
    add_candidate(
        &mut candidates,
        "interactive-2",
        WorkerSelection::Auto,
        |point| point.interactive_batch = 2,
    )?;
    add_candidate(
        &mut candidates,
        "interactive-4",
        WorkerSelection::Auto,
        |point| point.interactive_batch = 4,
    )?;
    add_candidate(
        &mut candidates,
        "latency-bias",
        WorkerSelection::fixed(6),
        |point| {
            point.turn_work = 32;
            point.completion_batch = 8;
            point.inbound_total = 12;
            point.inbound_per_lane = 4;
            point.command_batch = 8;
            point.owed_work_batch = 4;
            point.hot_turns = 4;
            point.worker_hot_idle_turns = 8;
            point.worker_spin_idle_turns = 32_768;
            point.interactive_batch = 4;
        },
    )?;
    add_candidate(
        &mut candidates,
        "throughput-bias",
        WorkerSelection::fixed(8),
        |point| {
            point.turn_work = 96;
            point.completion_batch = 24;
            point.inbound_total = 36;
            point.inbound_per_lane = 12;
            point.command_batch = 24;
            point.owed_work_batch = 16;
            point.hot_turns = 64;
            point.worker_hot_idle_turns = 0;
            point.worker_spin_idle_turns = 0;
        },
    )?;

    if let SweepMode::Coarse = mode {
        add_coarse_candidates(&mut candidates)?;
    }
    Ok(candidates)
}

fn add_coarse_candidates(candidates: &mut Vec<Candidate>) -> Result<(), SweepError> {
    add_candidate(candidates, "completion-8", WorkerSelection::Auto, |point| {
        point.completion_batch = 8;
    })?;
    add_candidate(
        candidates,
        "completion-24",
        WorkerSelection::Auto,
        |point| {
            point.completion_batch = 24;
        },
    )?;
    add_candidate(
        candidates,
        "inbound-total-12",
        WorkerSelection::Auto,
        |point| {
            point.inbound_total = 12;
            point.inbound_per_lane = 4;
        },
    )?;
    add_candidate(
        candidates,
        "inbound-total-36",
        WorkerSelection::Auto,
        |point| {
            point.inbound_total = 36;
        },
    )?;
    add_candidate(
        candidates,
        "inbound-lane-4",
        WorkerSelection::Auto,
        |point| {
            point.inbound_per_lane = 4;
        },
    )?;
    add_candidate(
        candidates,
        "inbound-lane-12",
        WorkerSelection::Auto,
        |point| {
            point.inbound_per_lane = 12;
        },
    )?;
    add_candidate(candidates, "command-8", WorkerSelection::Auto, |point| {
        point.command_batch = 8;
    })?;
    add_candidate(candidates, "command-24", WorkerSelection::Auto, |point| {
        point.command_batch = 24;
    })?;
    add_candidate(candidates, "owed-4", WorkerSelection::Auto, |point| {
        point.owed_work_batch = 4;
    })?;
    add_candidate(candidates, "owed-16", WorkerSelection::Auto, |point| {
        point.owed_work_batch = 16;
    })?;
    add_candidate(candidates, "worker-hot-8", WorkerSelection::Auto, |point| {
        point.worker_hot_idle_turns = 8;
    })?;
    for (index, workers) in [4, 6, 8, 6, 8, 4, 6, 8].into_iter().enumerate() {
        let turn_work = [32, 48, 64, 80, 96, 48, 80, 64][index];
        let completion_batch = [8, 12, 16, 20, 24, 16, 8, 24][index];
        let inbound_total = [12, 18, 24, 30, 36, 24, 18, 30][index];
        let inbound_per_lane = [4, 6, 8, 10, 12, 4, 6, 10][index];
        let command_batch = [8, 12, 16, 20, 24, 8, 20, 12][index];
        let owed_work_batch = [4, 6, 8, 12, 16, 12, 6, 16][index];
        let hot_turns = [4, 8, 16, 32, 64, 32, 8, 64][index];
        let worker_hot_idle_turns = [0, 2, 8, 0, 2, 8, 2, 0][index];
        let worker_spin_idle_turns = [0, 2_048, 8_192, 32_768, 2_048, 0, 32_768, 8_192][index];
        let interactive_batch = [2, 4, 8, 4, 8, 2, 4, 8][index];
        add_candidate(
            candidates,
            &format!("balanced-{}", index + 1),
            WorkerSelection::fixed(workers),
            |point| {
                point.turn_work = turn_work;
                point.completion_batch = completion_batch;
                point.inbound_total = inbound_total;
                point.inbound_per_lane = inbound_per_lane;
                point.command_batch = command_batch;
                point.owed_work_batch = owed_work_batch;
                point.hot_turns = hot_turns;
                point.worker_hot_idle_turns = worker_hot_idle_turns;
                point.worker_spin_idle_turns = worker_spin_idle_turns;
                point.interactive_batch = interactive_batch;
            },
        )?;
    }
    Ok(())
}

fn add_candidate(
    candidates: &mut Vec<Candidate>,
    name: &str,
    workers: WorkerSelection,
    tune: impl FnOnce(&mut PolicyPoint),
) -> Result<(), SweepError> {
    let mut point = PolicyPoint::production();
    tune(&mut point);
    let policy = point
        .policy()
        .map_err(|source| SweepError::InvalidCandidate {
            candidate: name.to_owned(),
            source,
        })?;
    candidates.push(Candidate {
        name: name.to_owned(),
        policy,
        workers,
    });
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Observation {
    idle_p50_micros: u64,
    idle_p99_micros: u64,
    mixed_p50_micros: u64,
    mixed_p99_micros: u64,
    resource_micros: u64,
    cpu_micros: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
    minor_page_faults: u64,
    major_page_faults: u64,
}

impl Observation {
    fn median(observations: Vec<Self>) -> Self {
        Self {
            idle_p50_micros: median_field(&observations, |value| value.idle_p50_micros),
            idle_p99_micros: median_field(&observations, |value| value.idle_p99_micros),
            mixed_p50_micros: median_field(&observations, |value| value.mixed_p50_micros),
            mixed_p99_micros: median_field(&observations, |value| value.mixed_p99_micros),
            resource_micros: median_field(&observations, |value| value.resource_micros),
            cpu_micros: median_field(&observations, |value| value.cpu_micros),
            voluntary_context_switches: median_field(&observations, |value| {
                value.voluntary_context_switches
            }),
            involuntary_context_switches: median_field(&observations, |value| {
                value.involuntary_context_switches
            }),
            minor_page_faults: median_field(&observations, |value| value.minor_page_faults),
            major_page_faults: median_field(&observations, |value| value.major_page_faults),
        }
    }

    fn dominates(self, other: Self) -> bool {
        let no_worse = self.mixed_p99_micros <= other.mixed_p99_micros
            && self.resource_micros <= other.resource_micros
            && self.cpu_micros <= other.cpu_micros;
        let better = self.mixed_p99_micros < other.mixed_p99_micros
            || self.resource_micros < other.resource_micros
            || self.cpu_micros < other.cpu_micros;
        no_worse && better
    }
}

fn median_field(observations: &[Observation], field: impl Fn(Observation) -> u64) -> u64 {
    let mut values: Vec<_> = observations.iter().copied().map(field).collect();
    values.sort_unstable();
    values[values.len() / 2]
}

struct CandidateSummary {
    candidate: Candidate,
    observation: Observation,
}

fn run_probe(path: &Path, candidate: &Candidate) -> Result<Observation, SweepError> {
    let policy = candidate.policy;
    let output = Command::new(path)
        .env_remove("PRNS_CRYPTO_POOL")
        .env_remove("PRNS_CRYPTO_WORKERS")
        .args(["--workers", &candidate.workers.to_string()])
        .args(["--turn-work", &policy.turn_work().to_string()])
        .args(["--completion-batch", &policy.completion_batch().to_string()])
        .args(["--inbound-total", &policy.inbound_total().to_string()])
        .args(["--inbound-per-lane", &policy.inbound_per_lane().to_string()])
        .args(["--command-batch", &policy.command_batch().to_string()])
        .args(["--owed-work-batch", &policy.owed_work_batch().to_string()])
        .args(["--hot-turns", &policy.hot_turns().to_string()])
        .args([
            "--worker-hot-idle-turns",
            &policy.worker_hot_idle_turns().to_string(),
        ])
        .args([
            "--worker-spin-idle-turns",
            &policy.worker_spin_idle_turns().to_string(),
        ])
        .args([
            "--interactive-batch",
            &policy.interactive_batch().to_string(),
        ])
        .output()
        .map_err(|source| SweepError::Launch {
            candidate: candidate.name.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(SweepError::ProbeFailed {
            candidate: candidate.name.clone(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    parse_observation(&candidate.name, &String::from_utf8_lossy(&output.stdout))
}

fn run_probe_with_retries(
    path: &Path,
    candidate: &Candidate,
    sample: usize,
    retries: usize,
) -> Result<Observation, SweepError> {
    for attempt in 0..=retries {
        match run_probe(path, candidate) {
            Ok(observation) => return Ok(observation),
            Err(error) if attempt < retries => eprintln!(
                "RETRY candidate={} sample={} failed_attempt={} error={}",
                candidate.name,
                sample,
                attempt + 1,
                error
            ),
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn parse_observation(candidate: &str, output: &str) -> Result<Observation, SweepError> {
    let idle = find_line(output, "RESULT phase=idle ", candidate)?;
    let mixed = find_line(output, "RESULT phase=mixed ", candidate)?;
    let resource = find_line(output, "RESULT resource_bytes=", candidate)?;
    let usage = find_line(output, "USAGE cpu_micros=", candidate)?;
    Ok(Observation {
        idle_p50_micros: metric(idle, "p50_micros", candidate)?,
        idle_p99_micros: metric(idle, "p99_micros", candidate)?,
        mixed_p50_micros: metric(mixed, "p50_micros", candidate)?,
        mixed_p99_micros: metric(mixed, "p99_micros", candidate)?,
        resource_micros: metric(resource, "resource_micros", candidate)?,
        cpu_micros: metric(usage, "cpu_micros", candidate)?,
        voluntary_context_switches: metric(usage, "voluntary_context_switches", candidate)?,
        involuntary_context_switches: metric(usage, "involuntary_context_switches", candidate)?,
        minor_page_faults: metric(usage, "minor_page_faults", candidate)?,
        major_page_faults: metric(usage, "major_page_faults", candidate)?,
    })
}

fn find_line<'a>(output: &'a str, prefix: &str, candidate: &str) -> Result<&'a str, SweepError> {
    output
        .lines()
        .find(|line| line.starts_with(prefix))
        .ok_or_else(|| SweepError::MissingOutput {
            candidate: candidate.to_owned(),
            prefix: prefix.to_owned(),
        })
}

fn metric(line: &str, name: &str, candidate: &str) -> Result<u64, SweepError> {
    let value = line
        .split_ascii_whitespace()
        .find_map(|field| field.split_once('=').filter(|(key, _)| *key == name))
        .map(|(_, value)| value)
        .ok_or_else(|| SweepError::MissingMetric {
            candidate: candidate.to_owned(),
            metric: name.to_owned(),
        })?;
    value.parse::<u64>().map_err(|_| SweepError::InvalidMetric {
        candidate: candidate.to_owned(),
        metric: name.to_owned(),
        value: value.to_owned(),
    })
}

fn print_summary(kind: &str, summary: &CandidateSummary) {
    let observation = summary.observation;
    let resource_mib_per_sec = 64.0 * 1_000_000.0 / observation.resource_micros.max(1) as f64;
    println!(
        "{kind} candidate={} workers={} idle_p50_micros={} idle_p99_micros={} mixed_p50_micros={} mixed_p99_micros={} resource_micros={} resource_mib_per_sec={resource_mib_per_sec:.2} cpu_micros={} voluntary_context_switches={} involuntary_context_switches={} minor_page_faults={} major_page_faults={}",
        summary.candidate.name,
        summary.candidate.workers,
        observation.idle_p50_micros,
        observation.idle_p99_micros,
        observation.mixed_p50_micros,
        observation.mixed_p99_micros,
        observation.resource_micros,
        observation.cpu_micros,
        observation.voluntary_context_switches,
        observation.involuntary_context_switches,
        observation.minor_page_faults,
        observation.major_page_faults,
    );
}

#[derive(Debug)]
enum SweepError {
    MissingValue(String),
    UnknownArgument(String),
    InvalidPositiveInteger {
        argument: String,
        value: String,
    },
    InvalidInteger {
        argument: String,
        value: String,
    },
    CurrentExecutable(std::io::Error),
    ProbeNotFound(PathBuf),
    UnknownCandidate(String),
    InvalidCandidate {
        candidate: String,
        source: SchedulerPolicyError,
    },
    Launch {
        candidate: String,
        source: std::io::Error,
    },
    ProbeFailed {
        candidate: String,
        status: ExitStatus,
        stderr: String,
    },
    MissingOutput {
        candidate: String,
        prefix: String,
    },
    MissingMetric {
        candidate: String,
        metric: String,
    },
    InvalidMetric {
        candidate: String,
        metric: String,
        value: String,
    },
}

impl fmt::Display for SweepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(argument) => write!(formatter, "missing value for {argument}"),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument {argument}"),
            Self::InvalidPositiveInteger { argument, value } => {
                write!(
                    formatter,
                    "{argument} requires a positive integer, got {value:?}"
                )
            }
            Self::InvalidInteger { argument, value } => {
                write!(formatter, "{argument} requires an integer, got {value:?}")
            }
            Self::CurrentExecutable(error) => {
                write!(formatter, "could not locate the sweep executable: {error}")
            }
            Self::ProbeNotFound(path) => write!(
                formatter,
                "probe not found at {}; build both scheduler binaries or pass --probe",
                path.display()
            ),
            Self::UnknownCandidate(candidate) => {
                write!(formatter, "unknown candidate {candidate:?}")
            }
            Self::InvalidCandidate { candidate, source } => {
                write!(formatter, "candidate {candidate} is invalid: {source}")
            }
            Self::Launch { candidate, source } => {
                write!(
                    formatter,
                    "could not launch candidate {candidate}: {source}"
                )
            }
            Self::ProbeFailed {
                candidate,
                status,
                stderr,
            } => write!(
                formatter,
                "candidate {candidate} exited with {status}: {}",
                stderr.trim()
            ),
            Self::MissingOutput { candidate, prefix } => {
                write!(formatter, "candidate {candidate} omitted output {prefix:?}")
            }
            Self::MissingMetric { candidate, metric } => {
                write!(formatter, "candidate {candidate} omitted metric {metric}")
            }
            Self::InvalidMetric {
                candidate,
                metric,
                value,
            } => write!(
                formatter,
                "candidate {candidate} emitted invalid {metric} value {value:?}"
            ),
        }
    }
}

impl std::error::Error for SweepError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_candidates_are_unique_and_validated() {
        let candidates = candidates(SweepMode::Coarse).expect("candidate grid is valid");
        let unique: HashSet<_> = candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect();
        assert_eq!((candidates.len(), unique.len()), (34, 34));
    }

    #[test]
    fn probe_output_parses_into_one_typed_observation() {
        let output = "RESULT phase=idle probes=256 p50_micros=61 p99_micros=120 max_micros=200\nRESULT phase=mixed probes=256 p50_micros=140 p99_micros=3200 max_micros=4000\nRESULT resource_bytes=67108864 resource_micros=250000 resource_mib_per_sec=256.00\nUSAGE cpu_micros=620000 voluntary_context_switches=2800 involuntary_context_switches=40 minor_page_faults=6000 major_page_faults=0\n";
        assert_eq!(
            parse_observation("test", output).expect("complete output parses"),
            Observation {
                idle_p50_micros: 61,
                idle_p99_micros: 120,
                mixed_p50_micros: 140,
                mixed_p99_micros: 3_200,
                resource_micros: 250_000,
                cpu_micros: 620_000,
                voluntary_context_switches: 2_800,
                involuntary_context_switches: 40,
                minor_page_faults: 6_000,
                major_page_faults: 0,
            }
        );
    }

    #[test]
    fn pareto_dominance_requires_no_regression_and_one_improvement() {
        let baseline = Observation {
            idle_p50_micros: 60,
            idle_p99_micros: 120,
            mixed_p50_micros: 140,
            mixed_p99_micros: 4_000,
            resource_micros: 260_000,
            cpu_micros: 640_000,
            voluntary_context_switches: 3_000,
            involuntary_context_switches: 40,
            minor_page_faults: 6_000,
            major_page_faults: 0,
        };
        let better = Observation {
            mixed_p99_micros: 3_900,
            ..baseline
        };
        let tradeoff = Observation {
            mixed_p99_micros: 3_800,
            cpu_micros: 650_000,
            ..baseline
        };
        assert_eq!(
            (
                better.dominates(baseline),
                baseline.dominates(baseline),
                tradeoff.dominates(baseline)
            ),
            (true, false, false)
        );
    }
}
