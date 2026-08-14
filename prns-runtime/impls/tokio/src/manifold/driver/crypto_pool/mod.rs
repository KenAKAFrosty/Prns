use core::cell::Cell;
use core::num::NonZeroUsize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use heapless::Vec as HeaplessVec;
use tokio::sync::mpsc::UnboundedSender;

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

impl PoolWorkers {
    fn resolve(self) -> NonZeroUsize {
        match self {
            Self::Fixed(workers) => workers,
            Self::Auto => {
                let logical = std::thread::available_parallelism()
                    .map(NonZeroUsize::get)
                    .unwrap_or(6);
                let workers = match performance_cores() {
                    Some(performance) if performance < logical => performance
                        .saturating_sub(MANIFOLD_IO_HEADROOM)
                        .max(MIN_POOL_WORKERS),
                    _ => logical.saturating_sub(MANIFOLD_IO_HEADROOM).max(1),
                };
                NonZeroUsize::new(workers).unwrap_or(NonZeroUsize::MIN)
            }
        }
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

struct CryptoQueue {
    jobs: Mutex<VecDeque<CryptoJob>>,
    len: AtomicUsize,
    backpressure_depth: usize,
    ready: Condvar,
    shutdown: AtomicBool,
}

pub(super) struct CryptoPool {
    queue: Arc<CryptoQueue>,
    workers: Vec<std::thread::JoinHandle<()>>,
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

    pub(super) fn spawn(workers: usize, results: UnboundedSender<CryptoResult>) -> Option<Self> {
        let queue = Arc::new(CryptoQueue {
            jobs: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
            backpressure_depth: crypto_backpressure_depth(workers),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let mut handles = Vec::with_capacity(workers.max(1));
        for _ in 0..workers.max(1) {
            let worker_queue = queue.clone();
            let worker_results = results.clone();
            match std::thread::Builder::new()
                .spawn(move || crypto_worker(&worker_queue, &worker_results))
            {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    queue.shutdown.store(true, Ordering::Release);
                    queue.ready.notify_all();
                    for worker in handles {
                        let _ = worker.join();
                    }
                    return None;
                }
            }
        }
        Some(Self {
            queue,
            workers: handles,
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
        let queue = &*self.queue;
        if let Ok(mut jobs) = queue.jobs.lock() {
            if job.owes_packet_verdict() {
                self.packet_verdicts_owed
                    .set(self.packet_verdicts_owed.get().saturating_add(1));
                self.last_packet_verdict_event
                    .set(Some(std::time::Instant::now()));
            }
            jobs.push_back(job);
            #[cfg(feature = "runtime-metrics")]
            let queue_depth = queue.len.fetch_add(1, Ordering::Release).saturating_add(1);
            #[cfg(not(feature = "runtime-metrics"))]
            let _ = queue.len.fetch_add(1, Ordering::Release);
            #[cfg(feature = "runtime-metrics")]
            {
                self.submitted_jobs
                    .set(self.submitted_jobs.get().saturating_add(1));
                self.maximum_queue_depth
                    .set(self.maximum_queue_depth.get().max(queue_depth));
            }
            drop(jobs);
            queue.ready.notify_one();
        }
    }

    pub(super) fn has_queue_capacity(&self, additional: usize) -> bool {
        let has_capacity = self
            .queue
            .len
            .load(Ordering::Acquire)
            .saturating_add(additional)
            <= self.queue.backpressure_depth;
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
    pub(super) fn record_completed(&self) {
        self.completed_jobs
            .set(self.completed_jobs.get().saturating_add(1));
    }

    #[cfg(feature = "runtime-metrics")]
    pub(super) fn metrics_snapshot(&self) -> CryptoMetricsSnapshot {
        CryptoMetricsSnapshot {
            submitted_jobs: self.submitted_jobs.get(),
            completed_jobs: self.completed_jobs.get(),
            queue_depth: bounded_u32(self.queue.len.load(Ordering::Acquire)),
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
        self.queue.shutdown.store(true, Ordering::Release);
        let mut jobs = self
            .queue
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.clear();
        self.queue.len.store(0, Ordering::Release);
        drop(jobs);
        self.queue.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

const CRYPTO_QUEUE_PER_WORKER: usize = 4;
const MIN_CRYPTO_QUEUE_DEPTH: usize = 16;
const MAX_CRYPTO_QUEUE_DEPTH: usize = 64;

fn crypto_backpressure_depth(workers: usize) -> usize {
    workers
        .saturating_mul(CRYPTO_QUEUE_PER_WORKER)
        .clamp(MIN_CRYPTO_QUEUE_DEPTH, MAX_CRYPTO_QUEUE_DEPTH)
}

fn run_crypto_job(job: CryptoJob) -> CryptoResult {
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
            let valid = Ed25519Verifier::new(job.signing_key.as_ed25519())
                .map(|verifier| {
                    verifier
                        .verify(job.packet_hash.as_bytes(), &job.signature)
                        .is_ok()
                })
                .unwrap_or(false);
            CryptoResult::Verified {
                id: job.id,
                packet_hash: job.packet_hash,
                settlement: job.settlement,
                arrived_at: job.arrived_at,
                valid,
            }
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

fn crypto_worker(queue: &CryptoQueue, results: &UnboundedSender<CryptoResult>) {
    let Ok(mut jobs) = queue.jobs.lock() else {
        return;
    };
    loop {
        if queue.shutdown.load(Ordering::Acquire) {
            return;
        }
        match jobs.pop_front() {
            Some(job) => {
                queue.len.fetch_sub(1, Ordering::Release);
                drop(jobs);
                if results.send(run_crypto_job(job)).is_err() {
                    return;
                }
                let Ok(relocked) = queue.jobs.lock() else {
                    return;
                };
                jobs = relocked;
            }
            None => {
                let Ok(waited) = queue.ready.wait(jobs) else {
                    return;
                };
                jobs = waited;
            }
        }
    }
}

#[cfg(test)]
mod tests;
