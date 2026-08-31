use core::cell::{Cell, RefCell};
use core::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use heapless::Vec as HeaplessVec;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use tokio::sync::Notify;

use crate::crypto::{
    ed25519_sign, ed25519_verify_batch, x25519_diffie_hellman, x25519_keys_for_seal,
    Ed25519Signature, Ed25519Verifier, X25519PublicKey, X25519SharedSecret,
};
use crate::engine::{
    AnnounceVerifyOwed, CommandId, DecryptOwed, DeferredLinkReceiptSign, DeferredProofSign,
    EncryptOwed, InstantMillis, RatchetDecryptOwed, Settlement,
};
use crate::identity::{decrypt_token_in_place_with_ratchets, IdentitySigningPublicKey, OpenedBy};
use crate::interfaces::InterfaceId;
use crate::remote_control::{
    RemoteControlPairingAvailabilityVerification, RemoteControlPairingAvailabilityVerifyOwed,
};
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN;
use crate::routing::links::handshake::{
    link_proof_signature_valid, link_proof_signed_data, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::resources::build_outgoing::{
    seal_staged_resource, BuildOutgoingResourceError, BuildRegions, BuiltResource,
    SealedStagedResource, SALT_REROLL_CAP,
};
use crate::routing::links::resources::send::DeferredResourceBuild;
use crate::routing::links::resources::streamed_open::StreamedOpen;
use crate::routing::links::resources::{
    sealed_transfer_bytes, ResourceBody, ResourceHash, MAP_HASH_LEN, RESOURCE_NONCE_LEN,
};
use crate::routing::links::{LinkId, LinkKey};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::CryptoMetricsSnapshot;

use super::host_protocol::{HostResourceMetadata, HostResourcePayload};

/// How the host runtime runs the engine's asymmetric crypto. `Pooled` offloads verify/seal/sign/decrypt to worker threads and keeps the manifold hot; `Inline` runs them on the manifold thread (the embedded shape, and the mobile default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoPoolConfig {
    Inline,
    Pooled { workers: PoolWorkers },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolWorkers {
    /// Size to the host: available parallelism minus manifold headroom (min 1).
    Auto,
    Fixed(NonZeroUsize),
}

impl CryptoPoolConfig {
    /// `Pooled`/`Auto` on a host that benefits; `Inline` on mobile targets, where the manifold stays single-threaded to protect battery.
    #[must_use]
    pub fn host_default() -> Self {
        if cfg!(any(target_os = "ios", target_os = "android")) {
            Self::Inline
        } else {
            Self::Pooled {
                workers: PoolWorkers::Auto,
            }
        }
    }

    fn with_env_override(self) -> Self {
        let workers_env = std::env::var("PRNS_CRYPTO_WORKERS")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .map(PoolWorkers::Fixed);
        match std::env::var("PRNS_CRYPTO_POOL")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("0" | "off" | "false" | "no") => Self::Inline,
            Some("") | None => match self {
                Self::Inline => Self::Inline,
                Self::Pooled { workers } => Self::Pooled {
                    workers: workers_env.unwrap_or(workers),
                },
            },
            Some(_) => Self::Pooled {
                workers: workers_env.unwrap_or(PoolWorkers::Auto),
            },
        }
    }

    pub(crate) fn resolved_worker_count(self) -> Option<NonZeroUsize> {
        match self.with_env_override() {
            Self::Inline => None,
            Self::Pooled { workers } => Some(workers.resolve()),
        }
    }
}

const MANIFOLD_IO_HEADROOM: usize = 2;
const MIN_POOL_WORKERS: usize = 4;
const MAX_EFFICIENCY_SPILLOVER_WORKERS: usize = 2;

impl PoolWorkers {
    fn resolve(self) -> NonZeroUsize {
        match self {
            Self::Fixed(workers) => workers,
            Self::Auto => {
                let logical = std::thread::available_parallelism()
                    .map(NonZeroUsize::get)
                    .unwrap_or(6);
                let workers = automatic_worker_count(logical, performance_cores());
                NonZeroUsize::new(workers).unwrap_or(NonZeroUsize::MIN)
            }
        }
    }
}

fn automatic_worker_count(logical: usize, performance: Option<usize>) -> usize {
    match performance {
        Some(performance) if performance < logical => {
            let performance_workers = performance
                .saturating_sub(MANIFOLD_IO_HEADROOM)
                .max(MIN_POOL_WORKERS);
            let efficiency_spillover = logical
                .saturating_sub(performance)
                .min(MAX_EFFICIENCY_SPILLOVER_WORKERS);
            performance_workers
                .saturating_add(efficiency_spillover)
                .min(logical.saturating_sub(MANIFOLD_IO_HEADROOM).max(1))
        }
        _ => logical.saturating_sub(MANIFOLD_IO_HEADROOM).max(1),
    }
}

fn performance_cores() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        linux_cpu_list_len("/sys/devices/cpu_core/cpus").or_else(linux_highest_capacity_cores)
    }
    #[cfg(target_os = "macos")]
    {
        macos_sysctl_usize("hw.perflevel0.logicalcpu")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_cpu_list_len(path: &str) -> Option<usize> {
    let raw = std::fs::read_to_string(path).ok()?;
    let count: usize = raw
        .trim()
        .split(',')
        .filter_map(|span| {
            let mut bounds = span
                .split('-')
                .filter_map(|n| n.trim().parse::<usize>().ok());
            let first = bounds.next()?;
            let last = bounds.next().unwrap_or(first);
            last.checked_sub(first).map(|range| range + 1)
        })
        .sum();
    (count > 0).then_some(count)
}

#[cfg(target_os = "linux")]
fn linux_highest_capacity_cores() -> Option<usize> {
    let logical = std::thread::available_parallelism().ok()?.get();
    let capacities: Vec<usize> = (0..logical)
        .filter_map(|cpu| {
            std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity"))
                .ok()
                .and_then(|raw| raw.trim().parse::<usize>().ok())
        })
        .collect();
    let highest = *capacities.iter().max()?;
    let count = capacities.iter().filter(|&&c| c == highest).count();
    (count < capacities.len()).then_some(count)
}

#[cfg(target_os = "macos")]
fn macos_sysctl_usize(name: &str) -> Option<usize> {
    let output = std::process::Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

pub(super) struct EngineVerifyJob {
    pub(super) packet_hash: PacketHash,
    pub(super) signing_key: IdentitySigningPublicKey,
    pub(super) signature: Ed25519Signature,
    pub(super) id: CommandId,
    pub(super) settlement: Settlement,
    pub(super) arrived_at: InstantMillis,
}

pub(super) struct StagedSealJob {
    pub(super) link_id: LinkId,
    pub(super) key: LinkKey,
    pub(super) sdu: usize,
    pub(super) nonce_prefixed_bytes: usize,
    pub(super) plaintext: Vec<u8>,
    pub(super) seal_iv: [u8; 16],
    pub(super) salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
}

pub(super) struct ResourceBuildJob {
    pub(super) owed: DeferredResourceBuild,
    pub(super) data: HostResourcePayload,
    pub(super) compressed_candidate: Option<HostResourcePayload>,
    pub(super) metadata: HostResourceMetadata,
    pub(super) seal_iv: [u8; 16],
    pub(super) nonces: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP + 1],
}

pub(super) struct OpenSpanJob {
    pub(super) link_id: LinkId,
    pub(super) hash: ResourceHash,
    pub(super) span_start: usize,
    pub(super) state: StreamedOpen,
    pub(super) bytes: Vec<u8>,
}

#[allow(clippy::large_enum_variant)]
pub(super) enum CryptoJob {
    Verify(EngineVerifyJob),
    BuildResource(Box<ResourceBuildJob>),
    SealStaged(Box<StagedSealJob>),
    OpenSpan(Box<OpenSpanJob>),
    SealScalars(EncryptOwed),
    Sign(DeferredProofSign),
    Decrypt(DecryptOwed),
    DecryptWithRatchets(Box<RatchetDecryptOwed>),
    VerifyLinkProof(LinkProofVerifyOwed),
    SignLinkProof(LinkProofSignOwed),
    SignLinkReceipt(DeferredLinkReceiptSign),
    VerifyAnnounce(AnnounceVerifyOwed),
    VerifyRemoteControlPairingAvailability(RemoteControlPairingAvailabilityVerifyOwed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CryptoJobClass {
    Verify,
    Latency,
    Bulk,
}

const BULK_BYTES_PER_WORK_UNIT: usize = 8 * 1024;

impl CryptoJob {
    fn owes_packet_verdict(&self) -> bool {
        !matches!(self, Self::BuildResource(_) | Self::SealStaged(_))
    }

    fn scheduling_class(&self) -> CryptoJobClass {
        match self {
            Self::Verify(_) => CryptoJobClass::Verify,
            Self::BuildResource(_) | Self::SealStaged(_) | Self::OpenSpan(_) => {
                CryptoJobClass::Bulk
            }
            Self::SealScalars(_)
            | Self::Sign(_)
            | Self::Decrypt(_)
            | Self::DecryptWithRatchets(_)
            | Self::VerifyLinkProof(_)
            | Self::SignLinkProof(_)
            | Self::SignLinkReceipt(_)
            | Self::VerifyAnnounce(_)
            | Self::VerifyRemoteControlPairingAvailability(_) => CryptoJobClass::Latency,
        }
    }

    /// A deliberately coarse service-time estimate. One unit is approximately one small
    /// asymmetric operation; bulk jobs add a unit per 8 KiB so a resource-sized seal cannot look
    /// equivalent to a receipt verification merely because both occupy one ring slot.
    fn estimated_work(&self) -> usize {
        match self {
            Self::BuildResource(job) => 1 + job.data.len().div_ceil(BULK_BYTES_PER_WORK_UNIT),
            Self::SealStaged(job) => 1 + job.plaintext.len().div_ceil(BULK_BYTES_PER_WORK_UNIT),
            Self::OpenSpan(job) => 1 + job.bytes.len().div_ceil(BULK_BYTES_PER_WORK_UNIT),
            Self::VerifyLinkProof(_) | Self::SignLinkProof(_) => 3,
            Self::SealScalars(_) | Self::Decrypt(_) | Self::DecryptWithRatchets(_) => 2,
            Self::Verify(_)
            | Self::Sign(_)
            | Self::SignLinkReceipt(_)
            | Self::VerifyAnnounce(_)
            | Self::VerifyRemoteControlPairingAvailability(_) => 1,
        }
    }
}

struct ScheduledCryptoJob {
    job: CryptoJob,
    class: CryptoJobClass,
    work: usize,
}

struct ScheduledCryptoResult {
    result: CryptoResult,
    work: usize,
}

#[allow(clippy::large_enum_variant)]
pub(super) enum CryptoResult {
    Verified {
        id: CommandId,
        packet_hash: PacketHash,
        settlement: Settlement,
        arrived_at: InstantMillis,
        valid: bool,
    },
    Sealed {
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
    },
    Signed {
        target: InterfaceId,
        packet_hash: PacketHash,
        signature: Ed25519Signature,
    },
    LinkReceiptSigned {
        target: InterfaceId,
        link_id: LinkId,
        packet_hash: PacketHash,
        signature: Ed25519Signature,
    },
    Decrypted {
        owed: DecryptOwed,
        shared: X25519SharedSecret,
    },
    RatchetDecrypted {
        owed: Box<RatchetDecryptOwed>,
        opened: Option<(OpenedBy, HeaplessVec<u8, MAX_RATCHET_DECRYPT_PAYLOAD_LEN>)>,
    },
    LinkProofVerified {
        owed: LinkProofVerifyOwed,
        shared: Option<X25519SharedSecret>,
    },
    LinkProofSigned {
        owed: LinkProofSignOwed,
        responder_encryption: X25519PublicKey,
        shared: X25519SharedSecret,
        signature: Ed25519Signature,
    },
    AnnounceVerified {
        owed: AnnounceVerifyOwed,
        valid: bool,
    },
    RemoteControlPairingAvailabilityVerified {
        owed: RemoteControlPairingAvailabilityVerifyOwed,
        verification: RemoteControlPairingAvailabilityVerification,
    },
    ResourceBuilt {
        ticket: crate::routing::links::resources::table::DeferredResourceBuildTicket,
        request_data: HostResourcePayload,
        transfer: Vec<u8>,
        names: Vec<u8>,
        outcome: Result<BuiltResource, BuildOutgoingResourceError>,
    },
    StagedSealed {
        link_id: LinkId,
        stream_nonce: [u8; RESOURCE_NONCE_LEN],
        nonce_prefixed_bytes: usize,
        transfer: Vec<u8>,
        names: Vec<u8>,
        outcome: Result<SealedStagedResource, BuildOutgoingResourceError>,
    },
    SpanOpened {
        link_id: LinkId,
        hash: ResourceHash,
        span_start: usize,
        state: StreamedOpen,
        bytes: Vec<u8>,
    },
}

impl CryptoResult {
    pub(super) fn settles_packet_verdict(&self) -> bool {
        !matches!(self, Self::ResourceBuilt { .. } | Self::StagedSealed { .. })
    }
}

pub(super) struct CryptoCompletion {
    pub(super) worker: usize,
    pub(super) result: CryptoResult,
    pub(super) work: usize,
}

struct CryptoPoolState {
    queued_jobs: AtomicUsize,
    /// Durable readiness behind the coalescing `Notify`: a cancelled manifold wait can lose its
    /// place in Tokio's waiter queue, but it cannot lose this count or strand a result ring.
    ready_results: AtomicUsize,
    /// Armed only while the manifold can actually sleep waiting for a completion. Workers keep
    /// payloads in their SPSC rings and enter Tokio's wake path only on this state transition.
    completion_wake_armed: AtomicBool,
    backpressure_depth: usize,
    shutdown: AtomicBool,
}

struct CryptoWorker {
    /// The manifold owns this producer and the worker owns its matching consumer.
    job_producer: RefCell<Option<Producer<ScheduledCryptoJob>>>,
    /// The worker owns this ring's producer and the manifold owns this consumer.
    result_consumer: RefCell<Option<Consumer<ScheduledCryptoResult>>>,
    /// Set only across the worker's final empty-ring observation and park. Active workers observe
    /// their SPSC ring directly, so a submit does not pay an unconditional kernel wake.
    wake_armed: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    outstanding_jobs: Cell<usize>,
    outstanding_work: Cell<usize>,
    tail_class: Cell<Option<CryptoJobClass>>,
    tail_run: Cell<usize>,
}

pub(super) struct CryptoPool {
    state: Arc<CryptoPoolState>,
    workers: Vec<CryptoWorker>,
    verify_batch_target: usize,
    next_completion: Cell<usize>,
    #[cfg(feature = "runtime-metrics")]
    submitted_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    completed_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    maximum_queue_depth: Cell<usize>,
    #[cfg(feature = "runtime-metrics")]
    backpressure_deferrals: Cell<u64>,
    packet_verdicts_owed: Cell<usize>,
    packet_verdict_hot_turns: Cell<usize>,
}

impl CryptoPool {
    // Once the last verdict lands, give the manifold a short deterministic chance to receive the
    // next packet without parking. Activity, rather than a wall-clock read on every select pass,
    // is the useful signal here. The bounded depth reaches the measured throughput plateau while
    // still guaranteeing that an idle manifold returns to parking.
    const PACKET_VERDICT_HOT_TURNS: usize = 512;

    pub(super) fn spawn(workers: usize, completion_wake: Arc<Notify>) -> Option<Self> {
        let worker_count = workers.max(1);
        let state = Arc::new(CryptoPoolState {
            queued_jobs: AtomicUsize::new(0),
            ready_results: AtomicUsize::new(0),
            completion_wake_armed: AtomicBool::new(false),
            backpressure_depth: crypto_backpressure_depth(workers),
            shutdown: AtomicBool::new(false),
        });
        let mut worker_slots: Vec<CryptoWorker> = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            let (job_producer, job_consumer) = RingBuffer::new(CRYPTO_WORKER_JOB_RING_DEPTH);
            let (result_producer, result_consumer) =
                RingBuffer::new(CRYPTO_WORKER_RESULT_RING_DEPTH);
            let worker_state = state.clone();
            let worker_completion_wake = completion_wake.clone();
            let wake_armed = Arc::new(AtomicBool::new(false));
            let worker_wake_armed = wake_armed.clone();
            match std::thread::Builder::new()
                .name(format!("prns-crypto-{worker}"))
                .spawn(move || {
                    crypto_worker(
                        &worker_state,
                        job_consumer,
                        result_producer,
                        &worker_completion_wake,
                        &worker_wake_armed,
                    );
                }) {
                Ok(handle) => worker_slots.push(CryptoWorker {
                    job_producer: RefCell::new(Some(job_producer)),
                    result_consumer: RefCell::new(Some(result_consumer)),
                    wake_armed,
                    handle: Some(handle),
                    outstanding_jobs: Cell::new(0),
                    outstanding_work: Cell::new(0),
                    tail_class: Cell::new(None),
                    tail_run: Cell::new(0),
                }),
                Err(_) => {
                    state.shutdown.store(true, Ordering::Release);
                    for slot in &worker_slots {
                        if let Some(handle) = &slot.handle {
                            handle.thread().unpark();
                        }
                    }
                    for slot in &mut worker_slots {
                        if let Some(handle) = slot.handle.take() {
                            let _ = handle.join();
                        }
                    }
                    return None;
                }
            }
        }
        Some(Self {
            state,
            workers: worker_slots,
            verify_batch_target: verify_batch_target(worker_count, performance_cores()),
            next_completion: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            submitted_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            completed_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            maximum_queue_depth: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            backpressure_deferrals: Cell::new(0),
            packet_verdicts_owed: Cell::new(0),
            packet_verdict_hot_turns: Cell::new(0),
        })
    }

    pub(super) fn submit(&self, job: CryptoJob) {
        let owes_packet_verdict = job.owes_packet_verdict();
        let class = job.scheduling_class();
        let work = job.estimated_work();
        let selected_worker = self.worker_for(class, work);
        let queue_depth = self
            .state
            .queued_jobs
            .fetch_add(1, Ordering::Release)
            .saturating_add(1);
        let worker =
            self.push_scheduled_job(selected_worker, ScheduledCryptoJob { job, class, work });
        self.record_submitted_to_worker(worker, class, work);
        self.wake_worker_if_armed(worker);
        if owes_packet_verdict {
            if self.packet_verdicts_owed.get() == 0 {
                self.packet_verdict_hot_turns.set(0);
            }
            self.packet_verdicts_owed
                .set(self.packet_verdicts_owed.get().saturating_add(1));
        }
        #[cfg(feature = "runtime-metrics")]
        {
            self.submitted_jobs
                .set(self.submitted_jobs.get().saturating_add(1));
            self.maximum_queue_depth
                .set(self.maximum_queue_depth.get().max(queue_depth));
        }
        #[cfg(not(feature = "runtime-metrics"))]
        let _ = queue_depth;
    }

    /// Submit only the LINK receipt signs already exposed by the current inbound pass. The caller
    /// never waits to fill this batch: one receipt takes this same path and is woken immediately.
    /// Publishing every ring entry before waking its selected worker lets real ingress backlog be
    /// claimed as one worker chunk instead of paying one park/unpark and one ring-head publication
    /// per packet.
    pub(super) fn submit_link_receipts(&self, receipts: &mut Vec<DeferredLinkReceiptSign>) {
        let count = receipts.len();
        if count == 0 {
            return;
        }

        let queue_depth = self
            .state
            .queued_jobs
            .fetch_add(count, Ordering::Release)
            .saturating_add(count);
        let class = CryptoJobClass::Latency;
        let work = 1;
        let mut touched_workers: HeaplessVec<usize, MAX_CRYPTO_QUEUE_DEPTH> = HeaplessVec::new();
        let mut pair_affinity: Option<(LinkId, usize)> = None;
        for receipt in receipts.drain(..) {
            let link_id = receipt.link_id;
            let paired = pair_affinity.filter(|(paired_link, _)| *paired_link == link_id);
            let selected_worker = paired.map_or_else(
                || self.worker_for(class, work),
                |(_, paired_worker)| paired_worker,
            );
            let worker = self.push_scheduled_job(
                selected_worker,
                ScheduledCryptoJob {
                    job: CryptoJob::SignLinkReceipt(receipt),
                    class,
                    work,
                },
            );
            self.record_submitted_to_worker(worker, class, work);
            pair_affinity = if paired.is_some() {
                None
            } else {
                Some((link_id, worker))
            };
            if !touched_workers.contains(&worker) {
                let _ = touched_workers.push(worker);
            }
        }

        // The manifold is the sole producer for every job ring. Delaying only these wake syscalls
        // until all already-ready work is visible cannot delay a cold worker beyond this submit;
        // a worker that is already active may consume the entries concurrently without a wake.
        for worker in touched_workers {
            self.wake_worker_if_armed(worker);
        }

        if self.packet_verdicts_owed.get() == 0 {
            self.packet_verdict_hot_turns.set(0);
        }
        self.packet_verdicts_owed
            .set(self.packet_verdicts_owed.get().saturating_add(count));
        #[cfg(feature = "runtime-metrics")]
        {
            self.submitted_jobs
                .set(self.submitted_jobs.get().saturating_add(count as u64));
            self.maximum_queue_depth
                .set(self.maximum_queue_depth.get().max(queue_depth));
        }
        #[cfg(not(feature = "runtime-metrics"))]
        let _ = queue_depth;
    }

    fn push_scheduled_job(&self, selected_worker: usize, scheduled: ScheduledCryptoJob) -> usize {
        let mut pending = Some(scheduled);
        loop {
            let mut worker = selected_worker;
            for _ in 0..self.workers.len() {
                let Some(job) = pending.take() else {
                    unreachable!("a crypto job is either pending or was pushed");
                };
                let pushed = match self.workers[worker].job_producer.borrow_mut().as_mut() {
                    Some(producer) => producer.push(job),
                    None => Err(PushError::Full(job)),
                };
                match pushed {
                    Ok(()) => return worker,
                    Err(PushError::Full(job)) => pending = Some(job),
                }
                worker += 1;
                if worker == self.workers.len() {
                    worker = 0;
                }
            }
            std::thread::yield_now();
        }
    }

    fn record_submitted_to_worker(&self, worker: usize, class: CryptoJobClass, work: usize) {
        let slot = &self.workers[worker];
        slot.outstanding_jobs
            .set(slot.outstanding_jobs.get().saturating_add(1));
        slot.outstanding_work
            .set(slot.outstanding_work.get().saturating_add(work));
        if slot.tail_class.get() == Some(class) {
            slot.tail_run.set(slot.tail_run.get().saturating_add(1));
        } else {
            slot.tail_class.set(Some(class));
            slot.tail_run.set(1);
        }
    }

    fn wake_worker_if_armed(&self, worker: usize) {
        let slot = &self.workers[worker];
        if slot.wake_armed.load(Ordering::Acquire) && slot.wake_armed.swap(false, Ordering::AcqRel)
        {
            if let Some(handle) = &slot.handle {
                handle.thread().unpark();
            }
        }
    }

    fn worker_for(&self, class: CryptoJobClass, work: usize) -> usize {
        let least_loaded = self
            .workers
            .iter()
            .enumerate()
            .map(|(worker, slot)| (worker, slot.outstanding_work.get()))
            .min_by_key(|&(worker, load)| (load, worker))
            .unwrap_or_default();

        if class != CryptoJobClass::Verify {
            return least_loaded.0;
        }

        // Fill a bounded verification run when it costs no more than one target batch of skew.
        // This creates true dalek batches without idling a lightly loaded worker behind arbitrary
        // affinity, and bulk work immediately disqualifies itself through its estimated load.
        let affinity_slack = work.saturating_mul(self.verify_batch_target.saturating_sub(1));
        self.workers
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                slot.tail_class.get() == Some(CryptoJobClass::Verify)
                    && slot.tail_run.get() < self.verify_batch_target
                    && slot.outstanding_work.get() <= least_loaded.1.saturating_add(affinity_slack)
            })
            .max_by_key(|&(worker, slot)| {
                (
                    slot.tail_run.get(),
                    core::cmp::Reverse(slot.outstanding_work.get()),
                    core::cmp::Reverse(worker),
                )
            })
            .map_or(least_loaded.0, |(worker, _)| worker)
    }

    pub(super) fn record_completed(&self, worker: usize, work: usize) {
        if let Some(slot) = self.workers.get(worker) {
            let outstanding = slot.outstanding_jobs.get();
            debug_assert!(outstanding > 0, "a worker completed a job it did not own");
            slot.outstanding_jobs.set(outstanding.saturating_sub(1));
            slot.outstanding_work
                .set(slot.outstanding_work.get().saturating_sub(work));
            if outstanding == 1 {
                slot.tail_class.set(None);
                slot.tail_run.set(0);
            }
        }
        #[cfg(feature = "runtime-metrics")]
        self.completed_jobs
            .set(self.completed_jobs.get().saturating_add(1));
    }

    pub(super) fn pop_completion(&self) -> Option<CryptoCompletion> {
        // Start after the last successful worker so one continuously full ring cannot starve the
        // other workers' continuations.
        let mut worker = self.next_completion.get();
        for _ in 0..self.workers.len() {
            let result = self.workers[worker]
                .result_consumer
                .borrow_mut()
                .as_mut()
                .and_then(|consumer| consumer.pop().ok());
            if let Some(scheduled) = result {
                let previous = self.state.ready_results.fetch_sub(1, Ordering::AcqRel);
                debug_assert!(previous > 0, "a result ring held an uncounted completion");
                self.next_completion
                    .set(if worker + 1 == self.workers.len() {
                        0
                    } else {
                        worker + 1
                    });
                return Some(CryptoCompletion {
                    worker,
                    result: scheduled.result,
                    work: scheduled.work,
                });
            }
            worker += 1;
            if worker == self.workers.len() {
                worker = 0;
            }
        }
        None
    }

    pub(super) fn has_completion(&self) -> bool {
        self.state.ready_results.load(Ordering::Acquire) > 0
    }

    pub(super) fn disarm_completion_wait(&self) {
        self.state
            .completion_wake_armed
            .store(false, Ordering::Release);
    }

    /// Returns true when a completion is already durable; otherwise arms the single Tokio wake
    /// and closes the producer race with a second readiness observation before the caller waits.
    pub(super) fn prepare_completion_wait(&self) -> bool {
        if self.has_completion() {
            self.state
                .completion_wake_armed
                .store(false, Ordering::Release);
            return true;
        }
        self.state
            .completion_wake_armed
            .store(true, Ordering::Release);
        if self.has_completion() {
            self.state
                .completion_wake_armed
                .store(false, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub(super) fn has_queue_capacity(&self, additional: usize) -> bool {
        let has_capacity = self
            .state
            .queued_jobs
            .load(Ordering::Acquire)
            .saturating_add(additional)
            <= self.state.backpressure_depth;
        #[cfg(feature = "runtime-metrics")]
        if !has_capacity {
            self.backpressure_deferrals
                .set(self.backpressure_deferrals.get().saturating_add(1));
        }
        has_capacity
    }

    pub(super) fn take_packet_verdict_hot_turn(&self) -> bool {
        if self.packet_verdicts_owed.get() > 0 {
            return true;
        }
        let remaining = self.packet_verdict_hot_turns.get();
        self.packet_verdict_hot_turns
            .set(remaining.saturating_sub(1));
        remaining > 0
    }

    pub(super) fn packet_verdict_settled(&self) {
        let owed = self.packet_verdicts_owed.get();
        debug_assert!(owed > 0, "a packet verdict landed that no submit counted");
        let remaining = owed.saturating_sub(1);
        self.packet_verdicts_owed.set(remaining);
        if remaining == 0 {
            self.packet_verdict_hot_turns
                .set(Self::PACKET_VERDICT_HOT_TURNS);
        }
    }

    #[cfg(feature = "runtime-metrics")]
    pub(super) fn metrics_snapshot(&self) -> CryptoMetricsSnapshot {
        CryptoMetricsSnapshot {
            submitted_jobs: self.submitted_jobs.get(),
            completed_jobs: self.completed_jobs.get(),
            queue_depth: bounded_u32(self.state.queued_jobs.load(Ordering::Acquire)),
            maximum_queue_depth: bounded_u32(self.maximum_queue_depth.get()),
            backpressure_deferrals: self.backpressure_deferrals.get(),
            packet_verdicts_owed: bounded_u32(self.packet_verdicts_owed.get()),
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl Drop for CryptoPool {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Release);
        for worker in &self.workers {
            if let Some(handle) = &worker.handle {
                handle.thread().unpark();
            }
        }
        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
            worker.job_producer.get_mut().take();
            worker.result_consumer.get_mut().take();
        }
        self.state.queued_jobs.store(0, Ordering::Release);
        self.state.ready_results.store(0, Ordering::Release);
    }
}

const CRYPTO_QUEUE_PER_WORKER: usize = 2;
const MIN_CRYPTO_QUEUE_DEPTH: usize = 16;
const MAX_CRYPTO_QUEUE_DEPTH: usize = 64;
const CRYPTO_WORKER_JOB_RING_DEPTH: usize = 16;
// Claim only the jobs visible at the start of a worker pass, then execute the first immediately.
// This amortizes the SPSC ring's head publication without coalescing or delaying a lone job.
const CRYPTO_WORKER_BATCH_DEPTH: usize = 8;
// A bad signature makes batch verification do work that the exact per-job fallback must repeat.
// Cool down locally so sustained hostile input pays at most one speculative batch per window.
const CRYPTO_BATCH_FAILURE_COOLDOWN_JOBS: usize = 32;
// A worker must always be able to return a command-sized burst while the single manifold is still
// submitting it. This is storage headroom only; admission remains governed by the much smaller
// `crypto_backpressure_depth` above.
const CRYPTO_WORKER_RESULT_RING_DEPTH: usize = 128;

fn crypto_backpressure_depth(workers: usize) -> usize {
    workers
        .saturating_mul(CRYPTO_QUEUE_PER_WORKER)
        .clamp(MIN_CRYPTO_QUEUE_DEPTH, MAX_CRYPTO_QUEUE_DEPTH)
}

fn verify_batch_target(workers: usize, performance_cores: Option<usize>) -> usize {
    let workers = workers.max(1);
    let effective_parallelism = performance_cores.unwrap_or(workers).clamp(1, workers);
    crypto_backpressure_depth(workers)
        .div_ceil(effective_parallelism)
        .clamp(2, CRYPTO_WORKER_BATCH_DEPTH)
}

const WORKER_VERIFIER_CACHE_DEPTH: usize = 8;
type WorkerVerifierCache = [Option<Ed25519Verifier>; WORKER_VERIFIER_CACHE_DEPTH];

fn run_crypto_job(job: CryptoJob, verifier_cache: &mut WorkerVerifierCache) -> CryptoResult {
    match job {
        CryptoJob::BuildResource(job) => {
            let ResourceBuildJob {
                owed,
                data,
                compressed_candidate,
                metadata,
                seal_iv,
                nonces,
            } = *job;
            let shape = owed.shape();
            let ticket = owed.ticket();
            let mut transfer = vec![0u8; shape.transfer_bytes()];
            let mut names = vec![0u8; shape.part_count() * MAP_HASH_LEN];
            let mut fresh_nonces = nonces.into_iter();
            let outcome = owed.execute(
                &ResourceBody {
                    data: data.as_slice(),
                    compressed_candidate: compressed_candidate
                        .as_ref()
                        .map(HostResourcePayload::as_slice),
                    metadata: metadata.as_engine(),
                },
                &seal_iv,
                || fresh_nonces.next().unwrap_or_default(),
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut names,
                },
            );
            CryptoResult::ResourceBuilt {
                ticket,
                request_data: data,
                transfer,
                names,
                outcome,
            }
        }
        CryptoJob::SealStaged(job) => {
            let StagedSealJob {
                link_id,
                key,
                sdu,
                nonce_prefixed_bytes,
                plaintext,
                seal_iv,
                salts,
            } = *job;
            let mut stream_nonce = [0u8; RESOURCE_NONCE_LEN];
            stream_nonce.copy_from_slice(&plaintext[16..16 + RESOURCE_NONCE_LEN]);
            let stream_len = nonce_prefixed_bytes - RESOURCE_NONCE_LEN;
            let mut transfer = plaintext;
            transfer.resize(sealed_transfer_bytes(stream_len), 0);
            let mut names = vec![0u8; transfer.len().div_ceil(sdu) * MAP_HASH_LEN];
            let mut fresh_salts = salts.into_iter();
            let outcome = seal_staged_resource(
                &key,
                &seal_iv,
                || fresh_salts.next().unwrap_or_default(),
                sdu,
                nonce_prefixed_bytes,
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut names,
                },
            );
            CryptoResult::StagedSealed {
                link_id,
                stream_nonce,
                nonce_prefixed_bytes,
                transfer,
                names,
                outcome,
            }
        }
        CryptoJob::OpenSpan(job) => {
            let OpenSpanJob {
                link_id,
                hash,
                span_start,
                mut state,
                mut bytes,
            } = *job;
            state.chew_span(&mut bytes);
            CryptoResult::SpanOpened {
                link_id,
                hash,
                span_start,
                state,
                bytes,
            }
        }
        CryptoJob::Verify(job) => {
            let valid = cached_verifier(verifier_cache, job.signing_key.as_ed25519()).is_some_and(
                |verifier| {
                    verifier
                        .verify(job.packet_hash.as_bytes(), &job.signature)
                        .is_ok()
                },
            );
            verified_result(job, valid)
        }
        CryptoJob::SealScalars(owed) => {
            let (ephemeral_public, shared) =
                x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
            CryptoResult::Sealed {
                owed,
                ephemeral_public,
                shared,
            }
        }
        CryptoJob::Sign(job) => {
            let signature = ed25519_sign(&job.signing_secret, job.packet_hash.as_bytes());
            CryptoResult::Signed {
                target: job.target,
                packet_hash: job.packet_hash,
                signature,
            }
        }
        CryptoJob::SignLinkReceipt(job) => {
            let signature = ed25519_sign(&job.signing_secret, job.packet_hash.as_bytes());
            CryptoResult::LinkReceiptSigned {
                target: job.target,
                link_id: job.link_id,
                packet_hash: job.packet_hash,
                signature,
            }
        }
        CryptoJob::Decrypt(owed) => {
            let shared = x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
            CryptoResult::Decrypted { owed, shared }
        }
        CryptoJob::DecryptWithRatchets(mut owed) => {
            let opened = decrypt_token_in_place_with_ratchets(
                &owed.ratchet_secrets,
                &owed.encryption_secret,
                &owed.identity,
                owed.identity_key_fallback,
                &mut owed.token,
            )
            .ok()
            .map(|opened| {
                let mut buf = HeaplessVec::new();
                let _ = buf.extend_from_slice(opened.plaintext);
                (opened.opened_by, buf)
            });
            CryptoResult::RatchetDecrypted { owed, opened }
        }
        CryptoJob::VerifyLinkProof(owed) => {
            let shared = link_proof_signature_valid(&owed)
                .then(|| x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption));
            CryptoResult::LinkProofVerified { owed, shared }
        }
        CryptoJob::SignLinkProof(owed) => {
            let (responder_encryption, shared) =
                x25519_keys_for_seal(&owed.ephemeral_secret, &owed.request.initiator_encryption);
            let signed_data = link_proof_signed_data(
                &owed.request.link_id,
                &responder_encryption,
                owed.responder_signing.as_ed25519(),
                owed.mtu,
                owed.request.mode,
            );
            let signature = ed25519_sign(&owed.signing_secret, &signed_data);
            CryptoResult::LinkProofSigned {
                owed,
                responder_encryption,
                shared,
                signature,
            }
        }
        CryptoJob::VerifyAnnounce(owed) => {
            let valid = Announce::from_wire_unverified(&owed.header, &owed.payload)
                .is_ok_and(|announce| announce.signature_is_valid());
            CryptoResult::AnnounceVerified { owed, valid }
        }
        CryptoJob::VerifyRemoteControlPairingAvailability(owed) => {
            let verification = owed.verify();
            CryptoResult::RemoteControlPairingAvailabilityVerified { owed, verification }
        }
    }
}

fn verified_result(job: EngineVerifyJob, valid: bool) -> CryptoResult {
    CryptoResult::Verified {
        id: job.id,
        packet_hash: job.packet_hash,
        settlement: job.settlement,
        arrived_at: job.arrived_at,
        valid,
    }
}

fn cached_verifier<'a>(
    cache: &'a mut WorkerVerifierCache,
    public: &crate::crypto::Ed25519PublicKey,
) -> Option<&'a Ed25519Verifier> {
    if let Some(index) = cache
        .iter()
        .position(|entry| matches!(entry, Some(verifier) if verifier.public_key() == public))
    {
        cache.swap(0, index);
        return cache[0].as_ref();
    }
    let verifier = Ed25519Verifier::new(public).ok()?;
    cache.rotate_right(1);
    cache[0] = Some(verifier);
    cache[0].as_ref()
}

fn crypto_worker(
    state: &CryptoPoolState,
    mut jobs: Consumer<ScheduledCryptoJob>,
    mut results: Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
    wake_armed: &AtomicBool,
) {
    let mut verifier_cache = core::array::from_fn(|_| None);
    let mut batch_failure_cooldown = 0usize;
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            return;
        }
        let available = jobs.slots().min(CRYPTO_WORKER_BATCH_DEPTH);
        if available == 0 {
            // Arm before the second ring observation so a concurrent producer either sees the
            // arm and wakes us or leaves a job that prevents the park. `Thread::unpark` permits
            // cover the final interval between that observation and entering the kernel.
            wake_armed.store(true, Ordering::Release);
            if jobs.slots() == 0 && !state.shutdown.load(Ordering::Acquire) {
                std::thread::park();
            }
            wake_armed.store(false, Ordering::Release);
            continue;
        }
        let Ok(chunk) = jobs.read_chunk(available) else {
            continue;
        };
        state.queued_jobs.fetch_sub(available, Ordering::Release);
        let mut jobs = chunk.into_iter().peekable();
        while let Some(scheduled) = jobs.next() {
            let ScheduledCryptoJob { job, class, work } = scheduled;
            match job {
                CryptoJob::Verify(job) => {
                    if !matches!(
                        jobs.peek(),
                        Some(ScheduledCryptoJob {
                            job: CryptoJob::Verify(_),
                            ..
                        })
                    ) {
                        if !run_and_publish_crypto_job(
                            ScheduledCryptoJob {
                                job: CryptoJob::Verify(job),
                                class,
                                work,
                            },
                            &mut verifier_cache,
                            state,
                            &mut results,
                            completion_wake,
                        ) {
                            return;
                        }
                        continue;
                    }
                    let mut verification_jobs = HeaplessVec::new();
                    if verification_jobs
                        .push(ScheduledVerifyJob { job, work })
                        .is_err()
                    {
                        return;
                    }
                    while matches!(
                        jobs.peek(),
                        Some(ScheduledCryptoJob {
                            job: CryptoJob::Verify(_),
                            ..
                        })
                    ) {
                        let Some(ScheduledCryptoJob {
                            job: CryptoJob::Verify(job),
                            work,
                            ..
                        }) = jobs.next()
                        else {
                            unreachable!("a peeked verification job remains a verification job");
                        };
                        if verification_jobs
                            .push(ScheduledVerifyJob { job, work })
                            .is_err()
                        {
                            return;
                        }
                    }
                    if !run_and_publish_verification_jobs(
                        verification_jobs,
                        &mut verifier_cache,
                        &mut batch_failure_cooldown,
                        state,
                        &mut results,
                        completion_wake,
                    ) {
                        return;
                    }
                }
                CryptoJob::SignLinkReceipt(job) => {
                    let mut receipt_jobs = HeaplessVec::new();
                    if receipt_jobs
                        .push(ScheduledCryptoJob {
                            job: CryptoJob::SignLinkReceipt(job),
                            class,
                            work,
                        })
                        .is_err()
                    {
                        return;
                    }
                    while matches!(
                        jobs.peek(),
                        Some(ScheduledCryptoJob {
                            job: CryptoJob::SignLinkReceipt(_),
                            ..
                        })
                    ) {
                        let Some(job) = jobs.next() else {
                            unreachable!("a peeked LINK receipt job remains available");
                        };
                        if receipt_jobs.push(job).is_err() {
                            return;
                        }
                    }
                    if !run_and_publish_link_receipt_jobs(
                        receipt_jobs,
                        state,
                        &mut results,
                        completion_wake,
                    ) {
                        return;
                    }
                }
                job => {
                    if !run_and_publish_crypto_job(
                        ScheduledCryptoJob { job, class, work },
                        &mut verifier_cache,
                        state,
                        &mut results,
                        completion_wake,
                    ) {
                        return;
                    }
                }
            }
        }
    }
}

fn run_and_publish_link_receipt_jobs(
    jobs: HeaplessVec<ScheduledCryptoJob, CRYPTO_WORKER_BATCH_DEPTH>,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let mut completed = HeaplessVec::<ScheduledCryptoResult, CRYPTO_WORKER_BATCH_DEPTH>::new();
    for ScheduledCryptoJob { job, work, .. } in jobs {
        let CryptoJob::SignLinkReceipt(job) = job else {
            unreachable!("the LINK receipt batch contains only LINK receipt jobs");
        };
        let signature = ed25519_sign(&job.signing_secret, job.packet_hash.as_bytes());
        if completed
            .push(ScheduledCryptoResult {
                result: CryptoResult::LinkReceiptSigned {
                    target: job.target,
                    link_id: job.link_id,
                    packet_hash: job.packet_hash,
                    signature,
                },
                work,
            })
            .is_err()
        {
            return false;
        }
    }
    publish_crypto_results(completed, state, results, completion_wake)
}

struct ScheduledVerifyJob {
    job: EngineVerifyJob,
    work: usize,
}

fn run_and_publish_verification_jobs(
    jobs: HeaplessVec<ScheduledVerifyJob, CRYPTO_WORKER_BATCH_DEPTH>,
    verifier_cache: &mut WorkerVerifierCache,
    batch_failure_cooldown: &mut usize,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let batch_valid = if jobs.len() >= 2 && *batch_failure_cooldown == 0 {
        match verify_job_batch(&jobs, verifier_cache) {
            Some(true) => true,
            Some(false) => {
                *batch_failure_cooldown = CRYPTO_BATCH_FAILURE_COOLDOWN_JOBS;
                false
            }
            None => false,
        }
    } else {
        *batch_failure_cooldown = batch_failure_cooldown.saturating_sub(jobs.len());
        false
    };

    for scheduled in jobs {
        let ScheduledVerifyJob { job, work } = scheduled;
        let valid = batch_valid
            || cached_verifier(verifier_cache, job.signing_key.as_ed25519()).is_some_and(
                |verifier| {
                    verifier
                        .verify(job.packet_hash.as_bytes(), &job.signature)
                        .is_ok()
                },
            );
        if !publish_crypto_result(
            verified_result(job, valid),
            work,
            state,
            results,
            completion_wake,
        ) {
            return false;
        }
    }
    true
}

/// `None` means the batch contains a key whose legacy individual-verification semantics must be
/// retained. `Some(false)` means dalek rejected the batch and exact per-job fallback is required.
fn verify_job_batch(
    jobs: &[ScheduledVerifyJob],
    verifier_cache: &mut WorkerVerifierCache,
) -> Option<bool> {
    for ScheduledVerifyJob { job, .. } in jobs {
        cached_verifier(verifier_cache, job.signing_key.as_ed25519())?;
    }

    let mut messages: HeaplessVec<&[u8], CRYPTO_WORKER_BATCH_DEPTH> = HeaplessVec::new();
    let mut signatures: HeaplessVec<Ed25519Signature, CRYPTO_WORKER_BATCH_DEPTH> =
        HeaplessVec::new();
    let mut verifiers: HeaplessVec<&Ed25519Verifier, CRYPTO_WORKER_BATCH_DEPTH> =
        HeaplessVec::new();
    for ScheduledVerifyJob { job, .. } in jobs {
        let public = job.signing_key.as_ed25519();
        let verifier = verifier_cache
            .iter()
            .flatten()
            .find(|verifier| verifier.public_key() == public)?;
        if verifier.is_weak() {
            return None;
        }
        if messages.push(job.packet_hash.as_bytes()).is_err()
            || signatures.push(job.signature).is_err()
            || verifiers.push(verifier).is_err()
        {
            return None;
        }
    }
    Some(ed25519_verify_batch(&messages, &signatures, &verifiers).is_ok())
}

fn run_and_publish_crypto_job(
    scheduled: ScheduledCryptoJob,
    verifier_cache: &mut WorkerVerifierCache,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let ScheduledCryptoJob { job, work, .. } = scheduled;
    let result = run_crypto_job(job, verifier_cache);
    publish_crypto_result(result, work, state, results, completion_wake)
}

fn publish_crypto_result(
    result: CryptoResult,
    work: usize,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let mut pending = Some(ScheduledCryptoResult { result, work });
    // Reserve readiness before publishing into the ring. The manifold may be draining a different
    // worker concurrently; counting first prevents it from observing an uncounted result between
    // the ring's publish and a later atomic increment.
    state.ready_results.fetch_add(1, Ordering::Release);
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            state.ready_results.fetch_sub(1, Ordering::Release);
            return false;
        }
        let Some(result) = pending.take() else {
            unreachable!("a crypto result is either pending or was pushed");
        };
        match results.push(result) {
            Ok(()) => {
                notify_completion_if_armed(state, completion_wake);
                return true;
            }
            Err(PushError::Full(result)) => {
                pending = Some(result);
                notify_completion_if_armed(state, completion_wake);
                std::thread::yield_now();
            }
        }
    }
}

fn publish_crypto_results(
    pending: HeaplessVec<ScheduledCryptoResult, CRYPTO_WORKER_BATCH_DEPTH>,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let count = pending.len();
    if count == 0 {
        return true;
    }
    state.ready_results.fetch_add(count, Ordering::Release);
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            state.ready_results.fetch_sub(count, Ordering::Release);
            return false;
        }
        if results.slots() < count {
            notify_completion_if_armed(state, completion_wake);
            std::thread::yield_now();
            continue;
        }
        let Ok(chunk) = results.write_chunk_uninit(count) else {
            continue;
        };
        let written = chunk.fill_from_iter(pending);
        debug_assert_eq!(written, count);
        notify_completion_if_armed(state, completion_wake);
        return true;
    }
}

fn notify_completion_if_armed(state: &CryptoPoolState, completion_wake: &Notify) {
    if state.completion_wake_armed.swap(false, Ordering::AcqRel) {
        completion_wake.notify_one();
    }
}

#[cfg(test)]
mod tests;
