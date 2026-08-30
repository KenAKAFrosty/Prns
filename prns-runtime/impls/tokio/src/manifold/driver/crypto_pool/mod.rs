use core::cell::{Cell, RefCell};
use core::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use heapless::Vec as HeaplessVec;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use tokio::sync::Notify;

use crate::crypto::{
    ed25519_sign, x25519_diffie_hellman, x25519_keys_for_seal, Ed25519Signature, Ed25519Verifier,
    X25519PublicKey, X25519SharedSecret,
};
use crate::engine::{
    AnnounceVerifyOwed, CommandId, DecryptOwed, DeferredProofSign, EncryptOwed, InstantMillis,
    RatchetDecryptOwed, Settlement,
};
use crate::identity::{decrypt_token_in_place_with_ratchets, IdentitySigningPublicKey, OpenedBy};
use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN;
use crate::routing::links::handshake::{
    link_proof_signature_valid, link_proof_signed_data, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::resources::build_outgoing::{
    seal_staged_resource, BuildOutgoingResourceError, BuildRegions, SealedStagedResource,
    SALT_REROLL_CAP,
};
use crate::routing::links::resources::streamed_open::StreamedOpen;
use crate::routing::links::resources::{
    sealed_transfer_bytes, ResourceHash, MAP_HASH_LEN, RESOURCE_NONCE_LEN,
};
use crate::routing::links::{LinkId, LinkKey};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::CryptoMetricsSnapshot;

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
    SealStaged(Box<StagedSealJob>),
    OpenSpan(Box<OpenSpanJob>),
    SealScalars(EncryptOwed),
    Sign(DeferredProofSign),
    Decrypt(DecryptOwed),
    DecryptWithRatchets(Box<RatchetDecryptOwed>),
    VerifyLinkProof(LinkProofVerifyOwed),
    SignLinkProof(LinkProofSignOwed),
    VerifyAnnounce(AnnounceVerifyOwed),
}

impl CryptoJob {
    fn owes_packet_verdict(&self) -> bool {
        !matches!(self, Self::SealStaged(_))
    }
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
        !matches!(self, Self::StagedSealed { .. })
    }
}

pub(super) struct CryptoCompletion {
    pub(super) worker: usize,
    pub(super) result: CryptoResult,
}

struct CryptoPoolState {
    queued_jobs: AtomicUsize,
    /// Durable readiness behind the coalescing `Notify`: a cancelled manifold wait can lose its
    /// place in Tokio's waiter queue, but it cannot lose this count or strand a result ring.
    ready_results: AtomicUsize,
    backpressure_depth: usize,
    shutdown: AtomicBool,
}

struct CryptoWorker {
    /// The manifold owns this producer and the worker owns its matching consumer.
    job_producer: RefCell<Option<Producer<CryptoJob>>>,
    /// The worker owns this ring's producer and the manifold owns this consumer.
    result_consumer: RefCell<Option<Consumer<CryptoResult>>>,
    handle: Option<std::thread::JoinHandle<()>>,
    outstanding_jobs: Cell<usize>,
}

pub(super) struct CryptoPool {
    state: Arc<CryptoPoolState>,
    workers: Vec<CryptoWorker>,
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
    last_packet_verdict_event: Cell<Option<std::time::Instant>>,
}

impl CryptoPool {
    const PACKET_VERDICT_LINGER: Duration = Duration::from_micros(200);

    pub(super) fn spawn(workers: usize, completion_wake: Arc<Notify>) -> Option<Self> {
        let worker_count = workers.max(1);
        let state = Arc::new(CryptoPoolState {
            queued_jobs: AtomicUsize::new(0),
            ready_results: AtomicUsize::new(0),
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
            match std::thread::Builder::new()
                .name(format!("prns-crypto-{worker}"))
                .spawn(move || {
                    crypto_worker(
                        &worker_state,
                        job_consumer,
                        result_producer,
                        &worker_completion_wake,
                    );
                }) {
                Ok(handle) => worker_slots.push(CryptoWorker {
                    job_producer: RefCell::new(Some(job_producer)),
                    result_consumer: RefCell::new(Some(result_consumer)),
                    handle: Some(handle),
                    outstanding_jobs: Cell::new(0),
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
            last_packet_verdict_event: Cell::new(None),
        })
    }

    pub(super) fn submit(&self, job: CryptoJob) {
        let owes_packet_verdict = job.owes_packet_verdict();
        let selected_worker = self.worker_for();
        let queue_depth = self
            .state
            .queued_jobs
            .fetch_add(1, Ordering::Release)
            .saturating_add(1);
        let mut pending = Some(job);
        let worker = loop {
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
                    Ok(()) => break,
                    Err(PushError::Full(job)) => pending = Some(job),
                }
                worker += 1;
                if worker == self.workers.len() {
                    worker = 0;
                }
            }
            if pending.is_none() {
                break worker;
            }
            std::thread::yield_now();
        };
        let slot = &self.workers[worker];
        slot.outstanding_jobs
            .set(slot.outstanding_jobs.get().saturating_add(1));
        if let Some(handle) = &self.workers[worker].handle {
            handle.thread().unpark();
        }
        if owes_packet_verdict {
            self.packet_verdicts_owed
                .set(self.packet_verdicts_owed.get().saturating_add(1));
            self.last_packet_verdict_event
                .set(Some(std::time::Instant::now()));
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

    fn worker_for(&self) -> usize {
        self.workers
            .iter()
            .enumerate()
            .map(|(worker, slot)| (worker, slot.outstanding_jobs.get()))
            .min_by_key(|&(worker, load)| (load, worker))
            .map_or(0, |(worker, _)| worker)
    }

    pub(super) fn record_completed(&self, worker: usize) {
        if let Some(slot) = self.workers.get(worker) {
            let outstanding = slot.outstanding_jobs.get();
            debug_assert!(outstanding > 0, "a worker completed a job it did not own");
            slot.outstanding_jobs.set(outstanding.saturating_sub(1));
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
            if let Some(result) = result {
                let previous = self.state.ready_results.fetch_sub(1, Ordering::AcqRel);
                debug_assert!(previous > 0, "a result ring held an uncounted completion");
                self.next_completion
                    .set(if worker + 1 == self.workers.len() {
                        0
                    } else {
                        worker + 1
                    });
                return Some(CryptoCompletion { worker, result });
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

    pub(super) fn awaits_packet_verdict(&self) -> bool {
        self.packet_verdicts_owed.get() > 0
            || self
                .last_packet_verdict_event
                .get()
                .is_some_and(|at| at.elapsed() < Self::PACKET_VERDICT_LINGER)
    }

    pub(super) fn packet_verdict_settled(&self) {
        let owed = self.packet_verdicts_owed.get();
        debug_assert!(owed > 0, "a packet verdict landed that no submit counted");
        self.packet_verdicts_owed.set(owed.saturating_sub(1));
        self.last_packet_verdict_event
            .set(Some(std::time::Instant::now()));
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
const CRYPTO_WORKER_CLAIM_DEPTH: usize = 8;
// A worker must always be able to return a command-sized burst while the single manifold is still
// submitting it. This is storage headroom only; admission remains governed by the much smaller
// `crypto_backpressure_depth` above.
const CRYPTO_WORKER_RESULT_RING_DEPTH: usize = 128;

fn crypto_backpressure_depth(workers: usize) -> usize {
    workers
        .saturating_mul(CRYPTO_QUEUE_PER_WORKER)
        .clamp(MIN_CRYPTO_QUEUE_DEPTH, MAX_CRYPTO_QUEUE_DEPTH)
}

const WORKER_VERIFIER_CACHE_DEPTH: usize = 8;
type WorkerVerifierCache = [Option<Ed25519Verifier>; WORKER_VERIFIER_CACHE_DEPTH];

fn run_crypto_job(job: CryptoJob, verifier_cache: &mut WorkerVerifierCache) -> CryptoResult {
    match job {
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
    mut jobs: Consumer<CryptoJob>,
    mut results: Producer<CryptoResult>,
    completion_wake: &Notify,
) {
    let mut verifier_cache = core::array::from_fn(|_| None);
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            return;
        }
        let available = jobs.slots().min(CRYPTO_WORKER_CLAIM_DEPTH);
        if available == 0 {
            std::thread::park();
            continue;
        }
        let Ok(chunk) = jobs.read_chunk(available) else {
            continue;
        };
        state.queued_jobs.fetch_sub(available, Ordering::Release);
        for job in chunk {
            let result = run_crypto_job(job, &mut verifier_cache);
            if !publish_crypto_result(result, state, &mut results, completion_wake) {
                return;
            }
        }
    }
}

fn publish_crypto_result(
    result: CryptoResult,
    state: &CryptoPoolState,
    results: &mut Producer<CryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let mut pending = Some(result);
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
                completion_wake.notify_one();
                return true;
            }
            Err(PushError::Full(result)) => {
                pending = Some(result);
                completion_wake.notify_one();
                std::thread::yield_now();
            }
        }
    }
}

#[cfg(test)]
mod tests;
