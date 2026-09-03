#![expect(clippy::expect_used, clippy::panic)]

use core::time::Duration;
use std::fmt;
use std::num::NonZeroUsize;

use personal_rns::prelude::*;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    CryptoMetricsSnapshot, CryptoPoolConfig, EgressMetricsSnapshot, ManifoldMetricsSnapshot,
    PoolWorkers, SchedulerPolicy, SchedulerPolicyError, SchedulerPolicyInput, SegmentCompression,
};
use tokio::io::AsyncReadExt as _;

const RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
const PROBE_COUNT: usize = 256;
const WARMUP_PROBES: usize = 16;
const EXPERIMENT_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    if cfg!(debug_assertions) {
        panic!("build this probe with --release");
    }
    let config = parse_arguments().unwrap_or_else(|error| panic!("{error}"));
    print_policy(&config);

    let receiver_destination = destination(ResourceStrategy::Accept {
        max_uncompressed_bytes: RESOURCE_BYTES,
        accept_compressed: false,
    });
    let receiver_hash = receiver_destination
        .destination_hash()
        .expect("valid receiver destination");
    let tcp_server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("bind localhost server");
    let server_address = tcp_server
        .local_addr()
        .expect("read localhost server address")
        .to_string();
    let receiver = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        pre_configured_destinations: [receiver_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    })
    .with_crypto_pool(config.workers.crypto_pool())
    .with_scheduler_policy(config.scheduler_policy);
    let receiver_handle = receiver.handle();
    let _server = receiver_handle.supervise(tcp_server);

    let (announce_sender, mut announce_receiver) = tokio::sync::mpsc::unbounded_channel();
    let sender = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        pre_configured_destinations: [destination(ResourceStrategy::AcceptNone)],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = announce_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(server_address));
        },
        persistence: NoPersistence,
    })
    .with_crypto_pool(config.workers.crypto_pool())
    .with_scheduler_policy(config.scheduler_policy);
    let sender_handle = sender.handle();

    let announcer = receiver_handle.clone();
    let announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: receiver_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let experiment = async {
        loop {
            let heard = announce_receiver.recv().await.expect("sender remains live");
            if heard == receiver_hash {
                break;
            }
        }
        announce_task.abort();
        let link_id = sender_handle
            .establish_link(receiver_hash)
            .await
            .expect("establish localhost link");

        run_probes(&sender_handle, link_id, WARMUP_PROBES).await;
        let usage_before = process_usage();
        let idle = run_probes(&sender_handle, link_id, PROBE_COUNT).await;
        let before = sender_handle
            .metrics_snapshot()
            .await
            .expect("runtime metrics are available");
        let receiver_before = receiver_handle
            .metrics_snapshot()
            .await
            .expect("receiver runtime metrics are available");

        let resource_sender = sender_handle.clone();
        let resource_started = std::time::Instant::now();
        let resource = tokio::spawn(async move {
            resource_sender
                .send_resource_with_compression(
                    link_id,
                    RESOURCE_BYTES,
                    tokio::io::repeat(0xA5).take(RESOURCE_BYTES),
                    SegmentCompression::Never,
                )
                .await
        });
        tokio::task::yield_now().await;
        let mixed = run_probes(&sender_handle, link_id, PROBE_COUNT).await;
        resource
            .await
            .expect("resource task remains live")
            .expect("resource transfer settles");
        let resource_micros = elapsed_micros(resource_started);
        let after = sender_handle
            .metrics_snapshot()
            .await
            .expect("runtime metrics are available");
        let receiver_after = receiver_handle
            .metrics_snapshot()
            .await
            .expect("receiver runtime metrics are available");
        let usage = process_usage().saturating_sub(usage_before);

        print_latency("idle", &idle);
        print_latency("mixed", &mixed);
        println!(
            "RESULT resource_bytes={RESOURCE_BYTES} resource_micros={resource_micros} resource_mib_per_sec={:.2}",
            RESOURCE_BYTES as f64 * 1_000_000.0 / resource_micros.max(1) as f64
                / (1024.0 * 1024.0),
        );
        println!(
            "USAGE cpu_micros={} voluntary_context_switches={} involuntary_context_switches={}",
            usage.cpu_micros, usage.voluntary_context_switches, usage.involuntary_context_switches,
        );
        print_metrics(
            "sender",
            before.crypto.unwrap_or_default(),
            after.crypto.unwrap_or_default(),
            before.manifold,
            after.manifold,
            before.egress,
            after.egress,
        );
        print_metrics(
            "receiver",
            receiver_before.crypto.unwrap_or_default(),
            receiver_after.crypto.unwrap_or_default(),
            receiver_before.manifold,
            receiver_after.manifold,
            receiver_before.egress,
            receiver_after.egress,
        );
    };

    tokio::select! {
        result = tokio::time::timeout(EXPERIMENT_TIMEOUT, experiment) => {
            result.expect("scheduler probe completes within its deadline");
        }
        result = receiver.run() => {
            result.expect("receiver runs");
            panic!("receiver stopped during the scheduler probe");
        }
        result = sender.run() => {
            result.expect("sender runs");
            panic!("sender stopped during the scheduler probe");
        }
    }
}

struct ProbeConfig {
    scheduler_policy: SchedulerPolicy,
    workers: ProbeWorkers,
}

#[derive(Clone, Copy)]
enum ProbeWorkers {
    Auto,
    Fixed(NonZeroUsize),
}

impl ProbeWorkers {
    fn crypto_pool(self) -> CryptoPoolConfig {
        let workers = match self {
            Self::Auto => PoolWorkers::Auto,
            Self::Fixed(workers) => PoolWorkers::Fixed(workers),
        };
        CryptoPoolConfig::Pooled { workers }
    }
}

impl fmt::Display for ProbeWorkers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Fixed(workers) => workers.fmt(formatter),
        }
    }
}

struct PolicyValues {
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

impl PolicyValues {
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

    fn into_policy(self) -> Result<SchedulerPolicy, SchedulerPolicyError> {
        SchedulerPolicy::new(SchedulerPolicyInput {
            turn_work: nonzero(self.turn_work),
            completion_batch: nonzero(self.completion_batch),
            inbound_total: nonzero(self.inbound_total),
            inbound_per_lane: nonzero(self.inbound_per_lane),
            command_batch: nonzero(self.command_batch),
            owed_work_batch: nonzero(self.owed_work_batch),
            hot_turns: self.hot_turns,
            worker_hot_idle_turns: self.worker_hot_idle_turns,
            worker_spin_idle_turns: self.worker_spin_idle_turns,
            interactive_batch: nonzero(self.interactive_batch),
        })
    }
}

#[derive(Debug)]
enum ProbeArgumentError {
    MissingValue { argument: String },
    UnknownArgument { argument: String },
    InvalidInteger { argument: String, value: String },
    ZeroNotAllowed { argument: String },
    InvalidWorkerSelection { value: String },
    InvalidPolicy(SchedulerPolicyError),
}

impl fmt::Display for ProbeArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue { argument } => write!(formatter, "missing value for {argument}"),
            Self::UnknownArgument { argument } => write!(formatter, "unknown argument {argument}"),
            Self::InvalidInteger { argument, value } => {
                write!(formatter, "invalid integer {value:?} for {argument}")
            }
            Self::ZeroNotAllowed { argument } => {
                write!(formatter, "{argument} must be greater than zero")
            }
            Self::InvalidWorkerSelection { value } => {
                write!(
                    formatter,
                    "workers must be auto or a positive integer, got {value:?}"
                )
            }
            Self::InvalidPolicy(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ProbeArgumentError {}

fn parse_arguments() -> Result<ProbeConfig, ProbeArgumentError> {
    let mut values = PolicyValues::production();
    let mut workers = ProbeWorkers::Auto;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--turn-work" => {
                values.turn_work =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--completion-batch" => {
                values.completion_batch =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--inbound-total" => {
                values.inbound_total =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--inbound-per-lane" => {
                values.inbound_per_lane =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--command-batch" => {
                values.command_batch =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--owed-work-batch" => {
                values.owed_work_batch =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--hot-turns" => {
                values.hot_turns = parse_usize(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--worker-hot-idle-turns" => {
                values.worker_hot_idle_turns =
                    parse_usize(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--worker-spin-idle-turns" => {
                values.worker_spin_idle_turns =
                    parse_usize(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--interactive-batch" => {
                values.interactive_batch =
                    parse_positive(&argument, next_value(&argument, &mut arguments)?)?
            }
            "--workers" => {
                workers = parse_workers(next_value(&argument, &mut arguments)?)?;
            }
            _ => return Err(ProbeArgumentError::UnknownArgument { argument }),
        }
    }
    let scheduler_policy = values
        .into_policy()
        .map_err(ProbeArgumentError::InvalidPolicy)?;
    Ok(ProbeConfig {
        scheduler_policy,
        workers,
    })
}

fn next_value(
    argument: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<String, ProbeArgumentError> {
    arguments
        .next()
        .ok_or_else(|| ProbeArgumentError::MissingValue {
            argument: argument.to_owned(),
        })
}

fn parse_positive(argument: &str, value: String) -> Result<usize, ProbeArgumentError> {
    let parsed = parse_usize(argument, value)?;
    if parsed == 0 {
        return Err(ProbeArgumentError::ZeroNotAllowed {
            argument: argument.to_owned(),
        });
    }
    Ok(parsed)
}

fn parse_usize(argument: &str, value: String) -> Result<usize, ProbeArgumentError> {
    value
        .parse::<usize>()
        .map_err(|_| ProbeArgumentError::InvalidInteger {
            argument: argument.to_owned(),
            value,
        })
}

fn parse_workers(value: String) -> Result<ProbeWorkers, ProbeArgumentError> {
    if value == "auto" {
        return Ok(ProbeWorkers::Auto);
    }
    let workers = value
        .parse::<usize>()
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or(ProbeArgumentError::InvalidWorkerSelection { value })?;
    Ok(ProbeWorkers::Fixed(workers))
}

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("validated positive policy value")
}

fn print_policy(config: &ProbeConfig) {
    let policy = config.scheduler_policy;
    println!(
        "POLICY workers={} turn_work={} completion_batch={} inbound_total={} inbound_per_lane={} command_batch={} owed_work_batch={} hot_turns={} worker_hot_idle_turns={} worker_spin_idle_turns={} interactive_batch={}",
        config.workers,
        policy.turn_work(),
        policy.completion_batch(),
        policy.inbound_total(),
        policy.inbound_per_lane(),
        policy.command_batch(),
        policy.owed_work_batch(),
        policy.hot_turns(),
        policy.worker_hot_idle_turns(),
        policy.worker_spin_idle_turns(),
        policy.interactive_batch(),
    );
}

#[derive(Clone, Copy, Default)]
struct ProcessUsage {
    cpu_micros: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
}

impl ProcessUsage {
    fn saturating_sub(self, earlier: Self) -> Self {
        Self {
            cpu_micros: self.cpu_micros.saturating_sub(earlier.cpu_micros),
            voluntary_context_switches: self
                .voluntary_context_switches
                .saturating_sub(earlier.voluntary_context_switches),
            involuntary_context_switches: self
                .involuntary_context_switches
                .saturating_sub(earlier.involuntary_context_switches),
        }
    }
}

#[cfg(unix)]
fn process_usage() -> ProcessUsage {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return ProcessUsage::default();
    }
    let usage = unsafe { usage.assume_init() };
    ProcessUsage {
        cpu_micros: timeval_micros(usage.ru_utime).saturating_add(timeval_micros(usage.ru_stime)),
        voluntary_context_switches: usage.ru_nvcsw.max(0) as u64,
        involuntary_context_switches: usage.ru_nivcsw.max(0) as u64,
    }
}

#[cfg(unix)]
fn timeval_micros(value: libc::timeval) -> u64 {
    (value.tv_sec.max(0) as u64)
        .saturating_mul(1_000_000)
        .saturating_add(value.tv_usec.max(0) as u64)
}

#[cfg(not(unix))]
fn process_usage() -> ProcessUsage {
    ProcessUsage::default()
}

async fn run_probes(handle: &PrnsNodeHandle, link_id: LinkId, count: usize) -> Vec<u64> {
    let mut latencies = Vec::with_capacity(count);
    for sequence in 0..count {
        let started = std::time::Instant::now();
        handle
            .send_link_packet(link_id, &(sequence as u64).to_be_bytes())
            .await
            .expect("control probe settles");
        latencies.push(elapsed_micros(started));
    }
    latencies.sort_unstable();
    latencies
}

fn print_latency(phase: &str, latencies: &[u64]) {
    println!(
        "RESULT phase={phase} probes={} p50_micros={} p99_micros={} max_micros={}",
        latencies.len(),
        percentile(latencies, 50),
        percentile(latencies, 99),
        latencies.last().copied().unwrap_or_default(),
    );
}

fn print_metrics(
    role: &str,
    before_crypto: CryptoMetricsSnapshot,
    after_crypto: CryptoMetricsSnapshot,
    before_manifold: ManifoldMetricsSnapshot,
    after_manifold: ManifoldMetricsSnapshot,
    before_egress: EgressMetricsSnapshot,
    after_egress: EgressMetricsSnapshot,
) {
    println!(
        "METRICS role={role} bulk_jobs={} bulk_queue_wait_max_micros={} bulk_service_max_micros={} latency_jobs={} latency_queue_wait_max_micros={} latency_service_max_micros={} verify_jobs={} verify_queue_wait_max_micros={} verify_service_max_micros={} work_deferrals={} turns={} budget_yields={} turn_max_micros={} completion_batch_max={} inbound_batch_max={} command_batch_max={} owed_batch_max={} timer_lateness_max_ms={} pacer_lateness_max_ms={} egress_frames={} egress_full_drops={}",
        after_crypto
            .bulk
            .completed_jobs
            .saturating_sub(before_crypto.bulk.completed_jobs),
        after_crypto.bulk.maximum_queue_wait_micros,
        after_crypto.bulk.maximum_service_micros,
        after_crypto
            .latency
            .completed_jobs
            .saturating_sub(before_crypto.latency.completed_jobs),
        after_crypto.latency.maximum_queue_wait_micros,
        after_crypto.latency.maximum_service_micros,
        after_crypto
            .verify
            .completed_jobs
            .saturating_sub(before_crypto.verify.completed_jobs),
        after_crypto.verify.maximum_queue_wait_micros,
        after_crypto.verify.maximum_service_micros,
        after_crypto
            .work_backpressure_deferrals
            .saturating_sub(before_crypto.work_backpressure_deferrals),
        after_manifold.turns.saturating_sub(before_manifold.turns),
        after_manifold
            .budget_yields
            .saturating_sub(before_manifold.budget_yields),
        after_manifold.maximum_turn_micros,
        after_manifold.maximum_completion_batch,
        after_manifold.maximum_inbound_batch,
        after_manifold.maximum_command_batch,
        after_manifold.maximum_owed_work_batch,
        after_manifold.maximum_timer_lateness_ms,
        after_manifold.maximum_pacer_lateness_ms,
        after_egress
            .enqueued_frames
            .saturating_sub(before_egress.enqueued_frames),
        after_egress
            .full_lane_drops
            .saturating_sub(before_egress.full_lane_drops),
    );
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = sorted.len().saturating_sub(1).saturating_mul(percentile) / 100;
    sorted.get(index).copied().unwrap_or_default()
}

fn elapsed_micros(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn destination(resource_strategy: ResourceStrategy) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy,
        maximum_request_bytes: Default::default(),
        app_name: "scheduler-probe",
        aspects: &["localhost"],
        identity: try_generate_identity_secret().expect("generate benchmark identity"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
