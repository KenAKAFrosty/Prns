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
    AnnounceSignCompleted, AnnounceSignOwed, AnnounceVerification, AnnounceVerifyOwed,
    ChannelAckSignCompleted, ChannelAckSignOwed, ChannelAckVerification, ChannelAckVerifyOwed,
    CryptoOwed, DecryptOwed, EncryptCompleted, EncryptOwed, EstablishLinkCompleted,
    EstablishLinkOwed, IdentifySignCompleted, IdentifySignOwed, LinkIdentityVerification,
    LinkIdentityVerifyOwed, LinkReceiptSignCompleted, LinkReceiptSignOwed, ProofSignCompleted,
    ProofSignOwed, RatchetDecryptOwed, ReceiptProofVerification, ReceiptProofVerifyOwed,
    ResourceOpenSpanResidence, TunnelSynthesizeSignCompleted, TunnelSynthesizeSignOwed,
    TunnelSynthesizeVerification, TunnelSynthesizeVerifyOwed, WholeResourceOpenReservation,
};
use crate::identity::{decrypt_token_in_place_with_ratchets, OpenedBy};
use crate::manifold::grant_lane::HeapFrameSlot;
use crate::remote_control::{
    RemoteControlPairingAvailabilityVerification, RemoteControlPairingAvailabilityVerifyOwed,
};
use crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN;
use crate::routing::links::handshake::{
    link_proof_signature_valid, link_proof_signed_data, LinkProofSignOwed, LinkProofVerifyOwed,
};
use crate::routing::links::resources::build_outgoing::{
    seal_staged_resource, BuildOutgoingResourceError, BuildRegions, BuiltResource,
    SealedStagedResource, SALT_REROLL_CAP,
};
use crate::routing::links::resources::receive::part_hash::{
    ResourcePartHashPlan, ResourcePartHashResult,
};
use crate::routing::links::resources::send::{
    ResourceBuildPlan, ResourceBuildWorkspace, ResourceSealPlan,
};
use crate::routing::links::resources::streamed_open::StreamedOpen;
use crate::routing::links::resources::{
    sealed_transfer_bytes, ResourceBody, ResourceHash, MAP_HASH_LEN, RESOURCE_NONCE_LEN,
};
use crate::routing::links::LinkId;
#[cfg(feature = "runtime-metrics")]
use crate::runtime::{CryptoMetricsSnapshot, CryptoWorkClassMetricsSnapshot};

use super::host_protocol::{
    HostResourceDigestPreparation, HostResourceMetadata, HostResourcePayload,
};
use super::scheduling_policy::{SchedulerPolicy, MAX_INTERACTIVE_CRYPTO_BATCH};

mod worker_placement;

use worker_placement::{performance_core_count, CryptoWorkerLayout, CryptoWorkerRole};

/// How the host runtime runs the engine's asymmetric crypto. `Pooled` offloads verify/seal/sign/decrypt to worker threads and keeps the manifold hot; `Inline` runs them on the manifold thread (the embedded shape, and the mobile default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoPoolConfig {
    Inline,
    Pooled {
        workers: PoolWorkers,
        placement: CryptoWorkerPlacement,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolWorkers {
    /// Size to the host: available parallelism minus manifold headroom (min 1).
    Auto,
    Fixed(NonZeroUsize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoWorkerPlacement {
    SchedulerManaged,
    CoreClassAware,
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedCryptoPoolConfig {
    pub(crate) workers: NonZeroUsize,
    pub(crate) placement: CryptoWorkerPlacement,
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
                placement: CryptoWorkerPlacement::CoreClassAware,
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
                Self::Pooled { workers, placement } => Self::Pooled {
                    workers: workers_env.unwrap_or(workers),
                    placement,
                },
            },
            Some(_) => Self::Pooled {
                workers: workers_env.unwrap_or(PoolWorkers::Auto),
                placement: CryptoWorkerPlacement::CoreClassAware,
            },
        }
    }

    pub(crate) fn resolved(self) -> Option<ResolvedCryptoPoolConfig> {
        match self.with_env_override() {
            Self::Inline => None,
            Self::Pooled { workers, placement } => Some(ResolvedCryptoPoolConfig {
                workers: workers.resolve(),
                placement,
            }),
        }
    }

    pub(crate) fn resolved_worker_count(self) -> Option<NonZeroUsize> {
        self.resolved().map(|resolved| resolved.workers)
    }
}

const MANIFOLD_IO_HEADROOM: usize = 2;
const MIN_POOL_WORKERS: usize = 4;
const MAX_EFFICIENCY_SPILLOVER_WORKERS: usize = 2;
const RESOURCE_PART_HASH_CONCURRENCY: usize = 1;

impl PoolWorkers {
    fn resolve(self) -> NonZeroUsize {
        match self {
            Self::Fixed(workers) => workers,
            Self::Auto => {
                let logical = std::thread::available_parallelism()
                    .map(NonZeroUsize::get)
                    .unwrap_or(6);
                let workers = automatic_worker_count(logical, performance_core_count());
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

pub(super) struct StagedSealJob {
    pub(super) plan: ResourceSealPlan,
    pub(super) plaintext: Vec<u8>,
    pub(super) seal_iv: [u8; 16],
    pub(super) salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
}

pub(super) struct ResourceBuildJob {
    pub(super) plan: ResourceBuildPlan,
    pub(super) workspace: ResourceBuildWorkspace,
    pub(super) data: HostResourcePayload,
    pub(super) compressed_candidate: Option<HostResourcePayload>,
    pub(super) metadata: HostResourceMetadata,
    pub(super) digest: HostResourceDigestPreparation,
    pub(super) seal_iv: [u8; 16],
    pub(super) nonces: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP + 1],
}

pub(super) enum ResourcePartHashBuffer {
    GrantSlot {
        source: crate::interfaces::InterfaceId,
        frame: HeapFrameSlot,
        part: std::ops::Range<usize>,
    },
    Copied(Vec<u8>),
}

impl ResourcePartHashBuffer {
    pub(super) fn part(&self) -> &[u8] {
        match self {
            Self::GrantSlot { frame, part, .. } => &frame.bytes[part.clone()],
            Self::Copied(part) => part,
        }
    }

    pub(super) fn byte_len(&self) -> usize {
        self.part().len()
    }

    pub(super) fn return_target(self) -> Option<(crate::interfaces::InterfaceId, HeapFrameSlot)> {
        match self {
            Self::GrantSlot {
                source,
                frame,
                part: _,
            } => Some((source, frame)),
            Self::Copied(_) => None,
        }
    }
}

impl AsRef<[u8]> for ResourcePartHashBuffer {
    fn as_ref(&self) -> &[u8] {
        self.part()
    }
}

pub(super) struct ResourcePartHashJob {
    pub(super) plan: ResourcePartHashPlan,
    pub(super) buffer: ResourcePartHashBuffer,
}

pub(super) struct OpenSpanJob {
    pub(super) link_id: LinkId,
    pub(super) hash: ResourceHash,
    pub(super) span_start: usize,
    pub(super) state: StreamedOpen,
    pub(super) residence: ResourceOpenSpanResidence,
    pub(super) buffer: OpenSpanBuffer,
}

pub(super) enum OpenSpanBuffer {
    Span(Vec<u8>),
    Transfer {
        bytes: Vec<u8>,
        span: core::ops::Range<usize>,
    },
}

pub(super) struct ResourceDecompressionJob {
    pub(super) link_id: LinkId,
    pub(super) hash: ResourceHash,
    pub(super) stream: Vec<u8>,
    pub(super) uncompressed_data_bytes: u64,
}

pub(super) enum OpenedSpanResult {
    InPlace {
        byte_len: usize,
    },
    Owned(Vec<u8>),
    Transfer {
        bytes: Vec<u8>,
        span_byte_len: usize,
    },
}

#[allow(clippy::large_enum_variant)]
pub(super) enum CryptoJob {
    VerifySignature(SignatureVerifyJob),
    BuildResource(Box<ResourceBuildJob>),
    HashResourcePart(Box<ResourcePartHashJob>),
    DecompressResource(Box<ResourceDecompressionJob>),
    SealStaged(Box<StagedSealJob>),
    OpenSpan(Box<OpenSpanJob>),
    SealScalars(EncryptOwed),
    SignProof(ProofSignOwed),
    Decrypt(DecryptOwed),
    DecryptWithRatchets(Box<RatchetDecryptOwed>),
    VerifyLinkProof(LinkProofVerifyOwed),
    SignLinkProof(LinkProofSignOwed),
    SignLink(LinkSignJob),
    SignTunnelSynthesize(TunnelSynthesizeSignOwed),
    EstablishLink(EstablishLinkOwed),
    SignAnnounce(AnnounceSignOwed),
    VerifyAnnounce(AnnounceVerifyOwed),
    VerifyRemoteControlPairingAvailability(RemoteControlPairingAvailabilityVerifyOwed),
    #[cfg(test)]
    ScheduledTest(ScheduledTestJob),
}

#[cfg(test)]
pub(super) struct ScheduledTestJob {
    pub(super) id: u8,
    pub(super) class: CryptoJobClass,
    pub(super) started: Option<std::sync::mpsc::Sender<()>>,
    pub(super) release: Option<std::sync::mpsc::Receiver<()>>,
}

pub(super) enum SignatureVerifyJob {
    ReceiptProof(ReceiptProofVerifyOwed),
    ChannelAck(ChannelAckVerifyOwed),
    LinkIdentity(LinkIdentityVerifyOwed),
    TunnelSynthesize(TunnelSynthesizeVerifyOwed),
}

impl SignatureVerifyJob {
    fn inputs(&self) -> (&crate::crypto::Ed25519PublicKey, &[u8], &Ed25519Signature) {
        match self {
            Self::ReceiptProof(owed) => (
                owed.signing_key.as_ed25519(),
                owed.packet_hash.as_bytes(),
                &owed.signature,
            ),
            Self::ChannelAck(owed) => (
                &owed.signing_key,
                owed.packet_hash.as_bytes(),
                &owed.signature,
            ),
            Self::LinkIdentity(owed) => (&owed.signing_key, &owed.signed_data, &owed.signature),
            Self::TunnelSynthesize(owed) => {
                (&owed.signing_key, &owed.signed_region, &owed.signature)
            }
        }
    }

    fn complete(self, valid: bool) -> CryptoResult {
        match self {
            Self::ReceiptProof(owed) => receipt_proof_verified(
                owed,
                if valid {
                    ReceiptProofVerification::Valid
                } else {
                    ReceiptProofVerification::Invalid
                },
            ),
            Self::ChannelAck(owed) => CryptoResult::ChannelAckVerified {
                owed,
                verification: if valid {
                    ChannelAckVerification::Valid
                } else {
                    ChannelAckVerification::Invalid
                },
            },
            Self::LinkIdentity(owed) => CryptoResult::LinkIdentityVerified {
                owed,
                verification: if valid {
                    LinkIdentityVerification::Valid
                } else {
                    LinkIdentityVerification::Invalid
                },
            },
            Self::TunnelSynthesize(owed) => CryptoResult::TunnelSynthesizeVerified {
                owed,
                verification: if valid {
                    TunnelSynthesizeVerification::Valid
                } else {
                    TunnelSynthesizeVerification::Invalid
                },
            },
        }
    }
}

pub(super) enum LinkSignJob {
    ChannelAck(ChannelAckSignOwed),
    Receipt(LinkReceiptSignOwed),
    Identify(IdentifySignOwed),
}

impl LinkSignJob {
    fn link_id(&self) -> LinkId {
        match self {
            Self::ChannelAck(owed) => owed.link_id,
            Self::Receipt(owed) => owed.link_id,
            Self::Identify(owed) => owed.identify.link_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CryptoJobClass {
    Verify,
    Latency,
    Bulk,
}

#[cfg(feature = "runtime-metrics")]
impl CryptoJobClass {
    const fn index(self) -> usize {
        match self {
            Self::Verify => 0,
            Self::Latency => 1,
            Self::Bulk => 2,
        }
    }
}

const BULK_BYTES_PER_WORK_UNIT: usize = 8 * 1024;

impl CryptoJob {
    pub(super) fn from_owed(owed: CryptoOwed) -> Self {
        match owed {
            CryptoOwed::ReceiptProofVerify(owed) => {
                Self::VerifySignature(SignatureVerifyJob::ReceiptProof(owed))
            }
            CryptoOwed::ChannelAckVerify(owed) => {
                Self::VerifySignature(SignatureVerifyJob::ChannelAck(owed))
            }
            CryptoOwed::LinkIdentityVerify(owed) => {
                Self::VerifySignature(SignatureVerifyJob::LinkIdentity(owed))
            }
            CryptoOwed::TunnelSynthesizeVerify(owed) => {
                Self::VerifySignature(SignatureVerifyJob::TunnelSynthesize(owed))
            }
            CryptoOwed::Encrypt(owed) => Self::SealScalars(owed),
            CryptoOwed::Decrypt(owed) => Self::Decrypt(owed),
            CryptoOwed::RatchetDecrypt(owed) => Self::DecryptWithRatchets(Box::new(owed)),
            CryptoOwed::LinkProofVerify(owed) => Self::VerifyLinkProof(owed),
            CryptoOwed::LinkProofSign(owed) => Self::SignLinkProof(owed),
            CryptoOwed::ProofSign(owed) => Self::SignProof(owed),
            CryptoOwed::LinkReceiptSign(owed) => Self::SignLink(LinkSignJob::Receipt(owed)),
            CryptoOwed::ChannelAckSign(owed) => Self::SignLink(LinkSignJob::ChannelAck(owed)),
            CryptoOwed::IdentifySign(owed) => Self::SignLink(LinkSignJob::Identify(owed)),
            CryptoOwed::TunnelSynthesizeSign(owed) => Self::SignTunnelSynthesize(owed),
            CryptoOwed::EstablishLink(owed) => Self::EstablishLink(owed),
            CryptoOwed::AnnounceSign(owed) => Self::SignAnnounce(owed),
            CryptoOwed::AnnounceVerify(owed) => Self::VerifyAnnounce(owed),
            CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => {
                Self::VerifyRemoteControlPairingAvailability(owed)
            }
        }
    }

    fn owes_packet_verdict(&self) -> bool {
        match self {
            Self::BuildResource(_) | Self::HashResourcePart(_) | Self::SealStaged(_) => false,
            #[cfg(test)]
            Self::ScheduledTest(_) => false,
            _ => true,
        }
    }

    pub(super) fn scheduling_class(&self) -> CryptoJobClass {
        match self {
            Self::VerifySignature(_) => CryptoJobClass::Verify,
            Self::BuildResource(_)
            | Self::HashResourcePart(_)
            | Self::DecompressResource(_)
            | Self::SealStaged(_)
            | Self::OpenSpan(_) => CryptoJobClass::Bulk,
            Self::SealScalars(_)
            | Self::SignProof(_)
            | Self::Decrypt(_)
            | Self::DecryptWithRatchets(_)
            | Self::VerifyLinkProof(_)
            | Self::SignLinkProof(_)
            | Self::SignLink(_)
            | Self::SignTunnelSynthesize(_)
            | Self::EstablishLink(_)
            | Self::SignAnnounce(_)
            | Self::VerifyAnnounce(_)
            | Self::VerifyRemoteControlPairingAvailability(_) => CryptoJobClass::Latency,
            #[cfg(test)]
            Self::ScheduledTest(job) => job.class,
        }
    }

    /// A deliberately coarse service-time estimate. One unit is approximately one small
    /// asymmetric operation; bulk jobs add a unit per 8 KiB so a resource-sized seal cannot look
    /// equivalent to a receipt verification merely because both occupy one ring slot.
    pub(super) fn estimated_work(&self) -> usize {
        match self {
            Self::BuildResource(job) => 1 + job.data.len().div_ceil(BULK_BYTES_PER_WORK_UNIT),
            Self::HashResourcePart(job) => {
                1 + job.buffer.byte_len().div_ceil(BULK_BYTES_PER_WORK_UNIT)
            }
            Self::DecompressResource(job) => {
                1 + job.stream.len().div_ceil(BULK_BYTES_PER_WORK_UNIT)
            }
            Self::SealStaged(job) => 1 + job.plaintext.len().div_ceil(BULK_BYTES_PER_WORK_UNIT),
            Self::OpenSpan(job) => {
                let byte_len = match &job.buffer {
                    OpenSpanBuffer::Span(bytes) => bytes.len(),
                    OpenSpanBuffer::Transfer { span, .. } => span.len(),
                };
                1 + byte_len.div_ceil(BULK_BYTES_PER_WORK_UNIT)
            }
            Self::VerifyLinkProof(_) | Self::SignLinkProof(_) | Self::EstablishLink(_) => 3,
            Self::SealScalars(_) | Self::Decrypt(_) | Self::DecryptWithRatchets(_) => 2,
            Self::VerifySignature(_)
            | Self::SignProof(_)
            | Self::SignLink(_)
            | Self::SignTunnelSynthesize(_)
            | Self::SignAnnounce(_)
            | Self::VerifyAnnounce(_)
            | Self::VerifyRemoteControlPairingAvailability(_) => 1,
            #[cfg(test)]
            Self::ScheduledTest(_) => 1,
        }
    }
}

struct ScheduledCryptoJob {
    job: CryptoJob,
    class: CryptoJobClass,
    work: usize,
    timing: JobTiming,
}

struct ScheduledCryptoResult {
    result: CryptoResult,
    class: CryptoJobClass,
    work: usize,
    timing: CompletedJobTiming,
}

struct JobTiming {
    #[cfg(feature = "runtime-metrics")]
    submitted_at: std::time::Instant,
}

impl JobTiming {
    fn submitted() -> Self {
        Self {
            #[cfg(feature = "runtime-metrics")]
            submitted_at: std::time::Instant::now(),
        }
    }

    fn start(self) -> JobExecutionTimer {
        JobExecutionTimer {
            #[cfg(feature = "runtime-metrics")]
            queue_wait_micros: elapsed_micros(self.submitted_at),
            #[cfg(feature = "runtime-metrics")]
            started_at: std::time::Instant::now(),
        }
    }
}

struct JobExecutionTimer {
    #[cfg(feature = "runtime-metrics")]
    queue_wait_micros: u64,
    #[cfg(feature = "runtime-metrics")]
    started_at: std::time::Instant,
}

impl JobExecutionTimer {
    fn finish(self) -> CompletedJobTiming {
        CompletedJobTiming {
            #[cfg(feature = "runtime-metrics")]
            queue_wait_micros: self.queue_wait_micros,
            #[cfg(feature = "runtime-metrics")]
            service_micros: elapsed_micros(self.started_at),
        }
    }
}

pub(super) struct CompletedJobTiming {
    #[cfg(feature = "runtime-metrics")]
    queue_wait_micros: u64,
    #[cfg(feature = "runtime-metrics")]
    service_micros: u64,
}

impl CompletedJobTiming {
    pub(super) fn unmeasured() -> Self {
        Self {
            #[cfg(feature = "runtime-metrics")]
            queue_wait_micros: 0,
            #[cfg(feature = "runtime-metrics")]
            service_micros: 0,
        }
    }
}

#[cfg(feature = "runtime-metrics")]
#[derive(Default)]
struct CryptoWorkClassMetrics {
    submitted_jobs: Cell<u64>,
    completed_jobs: Cell<u64>,
    outstanding_work: Cell<u64>,
    maximum_outstanding_work: Cell<u64>,
    maximum_queue_wait_micros: Cell<u64>,
    maximum_service_micros: Cell<u64>,
}

#[cfg(feature = "runtime-metrics")]
impl CryptoWorkClassMetrics {
    fn submit(&self, work: usize) {
        let work = u64::try_from(work).unwrap_or(u64::MAX);
        self.submitted_jobs
            .set(self.submitted_jobs.get().saturating_add(1));
        let outstanding = self.outstanding_work.get().saturating_add(work);
        self.outstanding_work.set(outstanding);
        self.maximum_outstanding_work
            .set(self.maximum_outstanding_work.get().max(outstanding));
    }

    fn complete(&self, work: usize, timing: &CompletedJobTiming) {
        self.completed_jobs
            .set(self.completed_jobs.get().saturating_add(1));
        self.outstanding_work.set(
            self.outstanding_work
                .get()
                .saturating_sub(u64::try_from(work).unwrap_or(u64::MAX)),
        );
        self.maximum_queue_wait_micros.set(
            self.maximum_queue_wait_micros
                .get()
                .max(timing.queue_wait_micros),
        );
        self.maximum_service_micros
            .set(self.maximum_service_micros.get().max(timing.service_micros));
    }

    fn snapshot(&self) -> CryptoWorkClassMetricsSnapshot {
        CryptoWorkClassMetricsSnapshot {
            submitted_jobs: self.submitted_jobs.get(),
            completed_jobs: self.completed_jobs.get(),
            outstanding_work: self.outstanding_work.get(),
            maximum_outstanding_work: self.maximum_outstanding_work.get(),
            maximum_queue_wait_micros: self.maximum_queue_wait_micros.get(),
            maximum_service_micros: self.maximum_service_micros.get(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub(super) enum CryptoResult {
    ReceiptProofVerified {
        owed: ReceiptProofVerifyOwed,
        verification: ReceiptProofVerification,
    },
    ChannelAckVerified {
        owed: ChannelAckVerifyOwed,
        verification: ChannelAckVerification,
    },
    LinkIdentityVerified {
        owed: LinkIdentityVerifyOwed,
        verification: LinkIdentityVerification,
    },
    TunnelSynthesizeVerified {
        owed: TunnelSynthesizeVerifyOwed,
        verification: TunnelSynthesizeVerification,
    },
    Encrypted(EncryptCompleted),
    ProofSigned(ProofSignCompleted),
    LinkReceiptSigned(LinkReceiptSignCompleted),
    ChannelAckSigned(ChannelAckSignCompleted),
    IdentifySigned(IdentifySignCompleted),
    TunnelSynthesizeSigned(TunnelSynthesizeSignCompleted),
    LinkEstablished(EstablishLinkCompleted),
    AnnounceSigned(AnnounceSignCompleted),
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
    AnnounceVerification(AnnounceVerification),
    RemoteControlPairingAvailabilityVerification(RemoteControlPairingAvailabilityVerification),
    ResourceBuilt {
        reservation: crate::routing::links::resources::table::ResourceBuildReservation,
        request_data: HostResourcePayload,
        transfer: Vec<u8>,
        names: Vec<u8>,
        outcome: Result<BuiltResource, BuildOutgoingResourceError>,
    },
    ResourcePartHashed(ResourcePartHashResult<ResourcePartHashBuffer>),
    ResourceDecompressed {
        link_id: LinkId,
        hash: ResourceHash,
        plaintext: Vec<u8>,
    },
    StagedSealed {
        reservation: crate::routing::links::resources::send::ResourceSealReservation,
        transfer: Vec<u8>,
        names: Vec<u8>,
        outcome: Result<SealedStagedResource, BuildOutgoingResourceError>,
    },
    WholeResourceOpenUnavailable {
        reservation: WholeResourceOpenReservation,
    },
    SpanOpened {
        link_id: LinkId,
        hash: ResourceHash,
        span_start: usize,
        state: StreamedOpen,
        residence: ResourceOpenSpanResidence,
        opened: OpenedSpanResult,
    },
    #[cfg(test)]
    ScheduledTest(u8),
}

impl CryptoResult {
    pub(super) fn settles_packet_verdict(&self) -> bool {
        !matches!(
            self,
            Self::ResourceBuilt { .. }
                | Self::ResourcePartHashed(_)
                | Self::StagedSealed { .. }
                | Self::WholeResourceOpenUnavailable { .. }
        )
    }
}

pub(super) struct CryptoCompletion {
    pub(super) worker: Option<usize>,
    pub(super) result: CryptoResult,
    pub(super) class: CryptoJobClass,
    pub(super) work: usize,
    pub(super) timing: CompletedJobTiming,
}

struct CryptoPoolState {
    queued_jobs: AtomicUsize,
    /// Durable readiness behind the coalescing `Notify`: a cancelled manifold wait can lose its
    /// place in Tokio's waiter queue, but it cannot lose this count or strand a result ring.
    ready_results: AtomicUsize,
    /// Armed only while the manifold can actually sleep waiting for a completion. Workers keep
    /// payloads in their SPSC rings and enter Tokio's wake path only on this state transition.
    completion_wake_armed: AtomicBool,
    warm_worker: AtomicUsize,
    backpressure_depth: usize,
    shutdown: AtomicBool,
}

struct CryptoWorker {
    interactive_job_producer: RefCell<Option<Producer<ScheduledCryptoJob>>>,
    bulk_job_producer: RefCell<Option<Producer<ScheduledCryptoJob>>>,
    /// The worker owns this ring's producer and the manifold owns this consumer.
    result_consumer: RefCell<Option<Consumer<ScheduledCryptoResult>>>,
    wake_on_submit: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    role: CryptoWorkerRole,
    outstanding_jobs: Cell<usize>,
    outstanding_work: Cell<usize>,
    tail_class: Cell<Option<CryptoJobClass>>,
    tail_run: Cell<usize>,
}

enum CryptoWorkerStart {
    Run,
    Abort,
}

pub(super) struct CryptoPool {
    state: Arc<CryptoPoolState>,
    workers: Vec<CryptoWorker>,
    verify_batch_target: usize,
    maximum_outstanding_work: usize,
    resource_part_hash_jobs: Cell<usize>,
    next_equal_load: Cell<usize>,
    next_completion: Cell<usize>,
    #[cfg(feature = "runtime-metrics")]
    submitted_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    completed_jobs: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    maximum_queue_depth: Cell<usize>,
    #[cfg(feature = "runtime-metrics")]
    backpressure_deferrals: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    work_backpressure_deferrals: Cell<u64>,
    #[cfg(feature = "runtime-metrics")]
    work_class_metrics: [CryptoWorkClassMetrics; 3],
    packet_verdicts_owed: Cell<usize>,
    packet_verdict_hot_turns: Cell<usize>,
}

impl CryptoPool {
    // Once the last verdict lands, give the manifold a short deterministic chance to receive the
    // next packet without parking. Activity, rather than a wall-clock read on every select pass,
    // is the useful signal here. The bounded depth reaches the measured throughput plateau while
    // still guaranteeing that an idle manifold returns to parking.
    const PACKET_VERDICT_HOT_TURNS: usize = 512;

    #[cfg(test)]
    fn spawn(workers: usize, completion_wake: Arc<Notify>) -> Option<Self> {
        Self::spawn_with_policy(
            workers,
            completion_wake,
            SchedulerPolicy::production(),
            CryptoWorkerPlacement::SchedulerManaged,
        )
    }

    pub(super) fn spawn_with_policy(
        workers: usize,
        completion_wake: Arc<Notify>,
        scheduler_policy: SchedulerPolicy,
        placement: CryptoWorkerPlacement,
    ) -> Option<Self> {
        let worker_count = workers.max(1);
        let state = Arc::new(CryptoPoolState {
            queued_jobs: AtomicUsize::new(0),
            ready_results: AtomicUsize::new(0),
            completion_wake_armed: AtomicBool::new(false),
            warm_worker: AtomicUsize::new(NO_WARM_WORKER),
            backpressure_depth: crypto_backpressure_depth(workers),
            shutdown: AtomicBool::new(false),
        });
        let worker_layout = CryptoWorkerLayout::resolve(placement, worker_count);
        let placement_requires_affinity = worker_layout.requires_affinity();
        let (placement_ready_sender, placement_ready_receiver) = std::sync::mpsc::channel();
        let mut worker_starts = Vec::with_capacity(worker_count);
        let mut worker_slots: Vec<CryptoWorker> = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            let role = worker_layout.role(worker);
            let affinity = worker_layout.affinity(worker);
            let worker_ready_sender = placement_ready_sender.clone();
            let (worker_start_sender, worker_start_receiver) = std::sync::mpsc::channel();
            let (interactive_job_producer, interactive_job_consumer) =
                RingBuffer::new(CRYPTO_WORKER_JOB_RING_DEPTH);
            let (bulk_job_producer, bulk_job_consumer) =
                RingBuffer::new(CRYPTO_WORKER_JOB_RING_DEPTH);
            let (result_producer, result_consumer) =
                RingBuffer::new(CRYPTO_WORKER_RESULT_RING_DEPTH);
            let worker_state = state.clone();
            let worker_completion_wake = completion_wake.clone();
            let wake_on_submit = Arc::new(AtomicBool::new(false));
            let worker_wake_on_submit = wake_on_submit.clone();
            match std::thread::Builder::new()
                .name(format!("prns-crypto-{worker}"))
                .spawn(move || {
                    if placement_requires_affinity {
                        let placement_ready = affinity.apply_to_current_thread();
                        if worker_ready_sender.send(placement_ready).is_err() {
                            return;
                        }
                    }
                    match worker_start_receiver.recv() {
                        Ok(CryptoWorkerStart::Run) => {}
                        Ok(CryptoWorkerStart::Abort) | Err(_) => return,
                    }
                    crypto_worker(CryptoWorkerRun {
                        worker,
                        state: &worker_state,
                        interactive_jobs: interactive_job_consumer,
                        bulk_jobs: bulk_job_consumer,
                        results: result_producer,
                        completion_wake: &worker_completion_wake,
                        wake_on_submit: &worker_wake_on_submit,
                        scheduler_policy,
                    });
                }) {
                Ok(handle) => {
                    worker_starts.push(worker_start_sender);
                    worker_slots.push(CryptoWorker {
                        interactive_job_producer: RefCell::new(Some(interactive_job_producer)),
                        bulk_job_producer: RefCell::new(Some(bulk_job_producer)),
                        result_consumer: RefCell::new(Some(result_consumer)),
                        wake_on_submit,
                        handle: Some(handle),
                        role,
                        outstanding_jobs: Cell::new(0),
                        outstanding_work: Cell::new(0),
                        tail_class: Cell::new(None),
                        tail_run: Cell::new(0),
                    });
                }
                Err(_) => {
                    state.shutdown.store(true, Ordering::Release);
                    for start in worker_starts.drain(..) {
                        let _ = start.send(CryptoWorkerStart::Abort);
                    }
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
        drop(placement_ready_sender);
        let mut placement_enabled = true;
        if placement_requires_affinity {
            let placement_deadline = std::time::Instant::now() + WORKER_PLACEMENT_START_TIMEOUT;
            for _ in 0..worker_count {
                let remaining =
                    placement_deadline.saturating_duration_since(std::time::Instant::now());
                if !placement_ready_receiver
                    .recv_timeout(remaining)
                    .is_ok_and(|outcome| outcome.is_ok())
                {
                    placement_enabled = false;
                    break;
                }
            }
        }
        if !placement_enabled {
            state.shutdown.store(true, Ordering::Release);
            for start in worker_starts {
                let _ = start.send(CryptoWorkerStart::Abort);
            }
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
            drop(worker_slots);
            if placement == CryptoWorkerPlacement::CoreClassAware {
                return Self::spawn_with_policy(
                    workers,
                    completion_wake,
                    scheduler_policy,
                    CryptoWorkerPlacement::SchedulerManaged,
                );
            }
            return None;
        }
        let mut workers_started = true;
        for start in worker_starts {
            workers_started &= start.send(CryptoWorkerStart::Run).is_ok();
        }
        if !workers_started {
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
        Some(Self {
            state,
            workers: worker_slots,
            verify_batch_target: verify_batch_target(worker_count, performance_core_count()),
            maximum_outstanding_work: crypto_backpressure_work(worker_count),
            resource_part_hash_jobs: Cell::new(0),
            next_equal_load: Cell::new(0),
            next_completion: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            submitted_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            completed_jobs: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            maximum_queue_depth: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            backpressure_deferrals: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            work_backpressure_deferrals: Cell::new(0),
            #[cfg(feature = "runtime-metrics")]
            work_class_metrics: std::array::from_fn(|_| CryptoWorkClassMetrics::default()),
            packet_verdicts_owed: Cell::new(0),
            packet_verdict_hot_turns: Cell::new(0),
        })
    }

    pub(super) fn submit(&self, job: CryptoJob) {
        let owes_packet_verdict = job.owes_packet_verdict();
        let is_resource_part_hash = matches!(&job, CryptoJob::HashResourcePart(_));
        let class = job.scheduling_class();
        let work = job.estimated_work();
        let selected_worker = self.worker_for(class, work);
        let queue_depth = self
            .state
            .queued_jobs
            .fetch_add(1, Ordering::Release)
            .saturating_add(1);
        let worker = self.push_scheduled_job(
            selected_worker,
            ScheduledCryptoJob {
                job,
                class,
                work,
                timing: JobTiming::submitted(),
            },
        );
        self.record_submitted_to_worker(worker, class, work);
        if is_resource_part_hash {
            self.resource_part_hash_jobs
                .set(self.resource_part_hash_jobs.get().saturating_add(1));
        }
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

    /// Submit only the link-bound signs already exposed by the current inbound pass. The caller
    /// never waits to fill this batch: one signature takes this same path and is woken immediately.
    /// Publishing every ring entry before waking its selected worker lets real ingress backlog be
    /// claimed as one worker chunk instead of paying one park/unpark and one ring-head publication
    /// per packet.
    pub(super) fn submit_link_signs(&self, signs: &mut Vec<LinkSignJob>) {
        let count = signs.len();
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
        for sign in signs.drain(..) {
            let link_id = sign.link_id();
            let paired = pair_affinity.filter(|(paired_link, _)| *paired_link == link_id);
            let selected_worker = paired.map_or_else(
                || self.worker_for(class, work),
                |(_, paired_worker)| paired_worker,
            );
            let worker = self.push_scheduled_job(
                selected_worker,
                ScheduledCryptoJob {
                    job: CryptoJob::SignLink(sign),
                    class,
                    work,
                    timing: JobTiming::submitted(),
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
        let mut pending = scheduled;
        loop {
            let mut worker = selected_worker;
            for _ in 0..self.workers.len() {
                if !self.workers[worker].role.accepts(pending.class) {
                    worker += 1;
                    if worker == self.workers.len() {
                        worker = 0;
                    }
                    continue;
                }
                let producer = if pending.class == CryptoJobClass::Bulk {
                    &self.workers[worker].bulk_job_producer
                } else {
                    &self.workers[worker].interactive_job_producer
                };
                let pushed = match producer.borrow_mut().as_mut() {
                    Some(producer) => producer.push(pending),
                    None => Err(PushError::Full(pending)),
                };
                match pushed {
                    Ok(()) => return worker,
                    Err(PushError::Full(returned)) => pending = returned,
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
        #[cfg(feature = "runtime-metrics")]
        self.work_class_metrics[class.index()].submit(work);
    }

    fn wake_worker_if_armed(&self, worker: usize) {
        let slot = &self.workers[worker];
        if slot.wake_on_submit.swap(false, Ordering::AcqRel) {
            if let Some(handle) = &slot.handle {
                handle.thread().unpark();
            }
        }
    }

    fn worker_for(&self, class: CryptoJobClass, work: usize) -> usize {
        let worker_count = self.workers.len();
        let start = self
            .next_equal_load
            .get()
            .min(worker_count.saturating_sub(1));
        let mut least_loaded = None;
        for offset in 0..worker_count {
            let worker = (start + offset) % worker_count;
            if !self.workers[worker].role.accepts(class) {
                continue;
            }
            let load = self.workers[worker].outstanding_work.get();
            if least_loaded.is_none_or(|(_, least_load)| load < least_load) {
                least_loaded = Some((worker, load));
            }
        }
        let Some(mut least_loaded) = least_loaded else {
            return 0;
        };
        let warm_worker = self.state.warm_worker.load(Ordering::Acquire);
        if warm_worker < worker_count
            && self.workers[warm_worker].role.accepts(class)
            && self.workers[warm_worker].outstanding_work.get() == least_loaded.1
        {
            least_loaded = (
                warm_worker,
                self.workers[warm_worker].outstanding_work.get(),
            );
        }
        self.next_equal_load
            .set(if least_loaded.0 + 1 == worker_count {
                0
            } else {
                least_loaded.0 + 1
            });

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
                slot.role.accepts(class)
                    && slot.tail_class.get() == Some(CryptoJobClass::Verify)
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

    pub(super) fn record_completed(
        &self,
        worker: usize,
        _class: CryptoJobClass,
        work: usize,
        _timing: &CompletedJobTiming,
    ) {
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
        {
            self.completed_jobs
                .set(self.completed_jobs.get().saturating_add(1));
            self.work_class_metrics[_class.index()].complete(work, _timing);
        }
    }

    pub(super) fn has_resource_part_hash_capacity(&self) -> bool {
        self.resource_part_hash_jobs.get() < RESOURCE_PART_HASH_CONCURRENCY
    }

    pub(super) fn resource_part_hash_completed(&self) {
        let outstanding = self.resource_part_hash_jobs.get();
        debug_assert!(outstanding > 0, "an uncounted resource part hash completed");
        self.resource_part_hash_jobs
            .set(outstanding.saturating_sub(1));
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
                    worker: Some(worker),
                    result: scheduled.result,
                    class: scheduled.class,
                    work: scheduled.work,
                    timing: scheduled.timing,
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

    pub(super) fn has_work_capacity(&self, additional: usize) -> bool {
        let outstanding = self
            .workers
            .iter()
            .map(|worker| worker.outstanding_work.get())
            .fold(0usize, usize::saturating_add);
        let has_capacity = outstanding == 0
            || outstanding.saturating_add(additional) <= self.maximum_outstanding_work;
        #[cfg(feature = "runtime-metrics")]
        if !has_capacity {
            self.work_backpressure_deferrals
                .set(self.work_backpressure_deferrals.get().saturating_add(1));
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
            work_backpressure_deferrals: self.work_backpressure_deferrals.get(),
            packet_verdicts_owed: bounded_u32(self.packet_verdicts_owed.get()),
            verify: self.work_class_metrics[CryptoJobClass::Verify.index()].snapshot(),
            latency: self.work_class_metrics[CryptoJobClass::Latency.index()].snapshot(),
            bulk: self.work_class_metrics[CryptoJobClass::Bulk.index()].snapshot(),
        }
    }
}

#[cfg(feature = "runtime-metrics")]
fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(feature = "runtime-metrics")]
fn elapsed_micros(started_at: std::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX)
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
            worker.interactive_job_producer.get_mut().take();
            worker.bulk_job_producer.get_mut().take();
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
const WORKER_PLACEMENT_START_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(1);
// Claim only the jobs visible at the start of a worker pass, then execute the first immediately.
// This amortizes the SPSC ring's head publication without coalescing or delaying a lone job.
const NO_WARM_WORKER: usize = usize::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerIdleStep {
    Poll,
    Spin,
    Park,
}

struct WorkerIdleBackoff {
    hot_turns: usize,
    spin_turns: usize,
}

impl WorkerIdleBackoff {
    const fn cold() -> Self {
        Self {
            hot_turns: 0,
            spin_turns: 0,
        }
    }

    fn refresh(&mut self, scheduler_policy: SchedulerPolicy) {
        self.hot_turns = scheduler_policy.worker_hot_idle_turns();
        self.spin_turns = scheduler_policy.worker_spin_idle_turns();
    }

    fn next_step(&mut self) -> WorkerIdleStep {
        if self.hot_turns > 0 {
            self.hot_turns -= 1;
            return WorkerIdleStep::Poll;
        }
        if self.spin_turns > 0 {
            self.spin_turns -= 1;
            return WorkerIdleStep::Spin;
        }
        WorkerIdleStep::Park
    }
}

fn claim_warm_worker(state: &CryptoPoolState, worker: usize) -> bool {
    let current = state.warm_worker.load(Ordering::Acquire);
    current == worker
        || (current == NO_WARM_WORKER
            && state
                .warm_worker
                .compare_exchange(NO_WARM_WORKER, worker, Ordering::AcqRel, Ordering::Acquire)
                .is_ok())
}

fn release_warm_worker(state: &CryptoPoolState, worker: usize) {
    let _ = state.warm_worker.compare_exchange(
        worker,
        NO_WARM_WORKER,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

// A bad signature makes batch verification do work that the exact per-job fallback must repeat.
// Cool down locally so sustained hostile input pays at most one speculative batch per window.
const CRYPTO_BATCH_FAILURE_COOLDOWN_JOBS: usize = 32;
// A worker must always be able to return a command-sized burst while the single manifold is still
// submitting it. This is storage headroom only; admission remains governed by the much smaller
// `crypto_backpressure_depth` above.
const CRYPTO_WORKER_RESULT_RING_DEPTH: usize = 128;
const CRYPTO_WORK_PER_WORKER: usize = 256;

fn crypto_backpressure_depth(workers: usize) -> usize {
    workers
        .saturating_mul(CRYPTO_QUEUE_PER_WORKER)
        .clamp(MIN_CRYPTO_QUEUE_DEPTH, MAX_CRYPTO_QUEUE_DEPTH)
}

fn crypto_backpressure_work(workers: usize) -> usize {
    workers.max(1).saturating_mul(CRYPTO_WORK_PER_WORKER)
}

fn verify_batch_target(workers: usize, performance_cores: Option<usize>) -> usize {
    let workers = workers.max(1);
    let effective_parallelism = performance_cores.unwrap_or(workers).clamp(1, workers);
    crypto_backpressure_depth(workers)
        .div_ceil(effective_parallelism)
        .clamp(2, MAX_INTERACTIVE_CRYPTO_BATCH)
}

const WORKER_VERIFIER_CACHE_DEPTH: usize = 8;
type WorkerVerifierCache = [Option<Ed25519Verifier>; WORKER_VERIFIER_CACHE_DEPTH];

fn run_crypto_job(job: CryptoJob, verifier_cache: &mut WorkerVerifierCache) -> CryptoResult {
    match job {
        CryptoJob::BuildResource(job) => run_resource_build_job(*job),
        CryptoJob::HashResourcePart(job) => {
            let ResourcePartHashJob { plan, buffer } = *job;
            CryptoResult::ResourcePartHashed(plan.calculate(buffer))
        }
        CryptoJob::DecompressResource(job) => {
            let ResourceDecompressionJob {
                link_id,
                hash,
                stream,
                uncompressed_data_bytes,
            } = *job;
            let maximum = prns_runtime::resource_compression::resource_decompression_bound(
                uncompressed_data_bytes,
            );
            let plaintext =
                prns_runtime::resource_compression::decompress_bounded(&stream, maximum)
                    .unwrap_or_default();
            CryptoResult::ResourceDecompressed {
                link_id,
                hash,
                plaintext,
            }
        }
        CryptoJob::SealStaged(job) => {
            let StagedSealJob {
                plan,
                plaintext,
                seal_iv,
                salts,
            } = *job;
            let nonce_prefixed_bytes = plan.nonce_prefixed_bytes();
            let stream_len = nonce_prefixed_bytes - RESOURCE_NONCE_LEN;
            let mut transfer = plaintext;
            transfer.resize(sealed_transfer_bytes(stream_len), 0);
            let mut names = vec![0u8; transfer.len().div_ceil(plan.sdu()) * MAP_HASH_LEN];
            let mut fresh_salts = salts.into_iter();
            let outcome = seal_staged_resource(
                plan.key(),
                &seal_iv,
                || fresh_salts.next().unwrap_or_default(),
                plan.sdu(),
                nonce_prefixed_bytes,
                BuildRegions {
                    transfer: &mut transfer,
                    hashmap: &mut names,
                },
            );
            CryptoResult::StagedSealed {
                reservation: plan.reservation(),
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
                residence,
                buffer,
            } = *job;
            let opened = match buffer {
                OpenSpanBuffer::Span(mut bytes) => {
                    state.chew_span(&mut bytes);
                    OpenedSpanResult::Owned(bytes)
                }
                OpenSpanBuffer::Transfer { mut bytes, span } => {
                    state.chew_span(&mut bytes[span.clone()]);
                    OpenedSpanResult::Transfer {
                        bytes,
                        span_byte_len: span.len(),
                    }
                }
            };
            CryptoResult::SpanOpened {
                link_id,
                hash,
                span_start,
                state,
                residence,
                opened,
            }
        }
        CryptoJob::VerifySignature(job) => {
            let valid = {
                let (public, message, signature) = job.inputs();
                cached_verifier(verifier_cache, public)
                    .is_some_and(|verifier| verifier.verify(message, signature).is_ok())
            };
            job.complete(valid)
        }
        CryptoJob::SealScalars(owed) => {
            let (ephemeral_public, shared) =
                x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
            CryptoResult::Encrypted(EncryptCompleted {
                owed,
                ephemeral_public,
                shared,
            })
        }
        CryptoJob::SignProof(job) => {
            let signature = ed25519_sign(&job.signing_secret, job.packet_hash.as_bytes());
            CryptoResult::ProofSigned(ProofSignCompleted {
                target: job.target,
                packet_hash: job.packet_hash,
                signature,
            })
        }
        CryptoJob::SignLink(job) => run_link_sign_job(job).into(),
        CryptoJob::SignTunnelSynthesize(owed) => {
            let signature = ed25519_sign(&owed.signing_secret, &owed.signed_region);
            CryptoResult::TunnelSynthesizeSigned(TunnelSynthesizeSignCompleted { owed, signature })
        }
        CryptoJob::EstablishLink(owed) => CryptoResult::LinkEstablished(owed.fulfill()),
        CryptoJob::SignAnnounce(owed) => CryptoResult::AnnounceSigned(owed.fulfill()),
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
        CryptoJob::VerifyAnnounce(owed) => CryptoResult::AnnounceVerification(owed.verify()),
        CryptoJob::VerifyRemoteControlPairingAvailability(owed) => {
            CryptoResult::RemoteControlPairingAvailabilityVerification(owed.verify())
        }
        #[cfg(test)]
        CryptoJob::ScheduledTest(mut job) => {
            if let Some(started) = job.started.take() {
                let _ = started.send(());
            }
            if let Some(release) = job.release.take() {
                let _ = release.recv();
            }
            CryptoResult::ScheduledTest(job.id)
        }
    }
}

pub(super) fn run_crypto_job_inline(job: CryptoJob) -> CryptoResult {
    let mut verifier_cache: WorkerVerifierCache = std::array::from_fn(|_| None);
    run_crypto_job(job, &mut verifier_cache)
}

pub(super) fn run_resource_build_job(job: ResourceBuildJob) -> CryptoResult {
    let ResourceBuildJob {
        plan,
        workspace,
        data,
        compressed_candidate,
        metadata,
        digest,
        seal_iv,
        nonces,
    } = job;
    let shape = plan.shape();
    let reservation = plan.reservation();
    let mut transfer = match workspace {
        ResourceBuildWorkspace::Allocate => vec![0u8; shape.transfer_bytes()],
        ResourceBuildWorkspace::Owned(transfer) => transfer,
    };
    let mut names = vec![0u8; shape.part_count() * MAP_HASH_LEN];
    let mut fresh_nonces = nonces.into_iter();
    let body = ResourceBody {
        data: data.as_slice(),
        compressed_candidate: compressed_candidate
            .as_ref()
            .map(HostResourcePayload::as_slice),
        metadata: metadata.as_engine(),
    };
    let regions = BuildRegions {
        transfer: &mut transfer,
        hashmap: &mut names,
    };
    let outcome = match digest {
        HostResourceDigestPreparation::Calculate => plan.execute(
            &body,
            &seal_iv,
            || fresh_nonces.next().unwrap_or_default(),
            regions,
        ),
        #[cfg(feature = "parallel-resource-hash")]
        HostResourceDigestPreparation::Prepared(prepared) => plan.execute_with_prepared_digest(
            &body,
            prepared,
            &seal_iv,
            || fresh_nonces.next().unwrap_or_default(),
            regions,
        ),
    };
    CryptoResult::ResourceBuilt {
        reservation,
        request_data: data,
        transfer,
        names,
        outcome,
    }
}

fn receipt_proof_verified(
    owed: ReceiptProofVerifyOwed,
    verification: ReceiptProofVerification,
) -> CryptoResult {
    CryptoResult::ReceiptProofVerified { owed, verification }
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

struct CryptoWorkerRun<'a> {
    worker: usize,
    state: &'a CryptoPoolState,
    interactive_jobs: Consumer<ScheduledCryptoJob>,
    bulk_jobs: Consumer<ScheduledCryptoJob>,
    results: Producer<ScheduledCryptoResult>,
    completion_wake: &'a Notify,
    wake_on_submit: &'a AtomicBool,
    scheduler_policy: SchedulerPolicy,
}

fn crypto_worker(run: CryptoWorkerRun<'_>) {
    let CryptoWorkerRun {
        worker,
        state,
        mut interactive_jobs,
        mut bulk_jobs,
        mut results,
        completion_wake,
        wake_on_submit,
        scheduler_policy,
    } = run;
    let mut verifier_cache = core::array::from_fn(|_| None);
    let mut batch_failure_cooldown = 0usize;
    let mut idle_backoff = WorkerIdleBackoff::cold();
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            return;
        }
        let interactive_available = interactive_jobs
            .slots()
            .min(scheduler_policy.interactive_batch());
        if interactive_available > 0 {
            let Ok(chunk) = interactive_jobs.read_chunk(interactive_available) else {
                continue;
            };
            if claim_warm_worker(state, worker) {
                idle_backoff.refresh(scheduler_policy);
            }
            state
                .queued_jobs
                .fetch_sub(interactive_available, Ordering::Release);
            if !run_worker_jobs(
                chunk.into_iter(),
                &mut verifier_cache,
                &mut batch_failure_cooldown,
                state,
                &mut results,
                completion_wake,
            ) {
                return;
            }
            continue;
        }
        let bulk_available = bulk_jobs.slots().min(1);
        if bulk_available > 0 {
            let Ok(chunk) = bulk_jobs.read_chunk(bulk_available) else {
                continue;
            };
            if claim_warm_worker(state, worker) {
                idle_backoff.refresh(scheduler_policy);
            }
            state
                .queued_jobs
                .fetch_sub(bulk_available, Ordering::Release);
            if !run_worker_jobs(
                chunk.into_iter(),
                &mut verifier_cache,
                &mut batch_failure_cooldown,
                state,
                &mut results,
                completion_wake,
            ) {
                return;
            }
            continue;
        }
        match idle_backoff.next_step() {
            WorkerIdleStep::Poll => continue,
            WorkerIdleStep::Spin => {
                core::hint::spin_loop();
                continue;
            }
            WorkerIdleStep::Park => {}
        }
        release_warm_worker(state, worker);
        wake_on_submit.swap(true, Ordering::AcqRel);
        if interactive_jobs.slots() == 0
            && bulk_jobs.slots() == 0
            && !state.shutdown.load(Ordering::Acquire)
        {
            std::thread::park();
        }
    }
}

fn run_worker_jobs(
    jobs: impl Iterator<Item = ScheduledCryptoJob>,
    verifier_cache: &mut WorkerVerifierCache,
    batch_failure_cooldown: &mut usize,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let mut jobs = jobs.peekable();
    while let Some(scheduled) = jobs.next() {
        let ScheduledCryptoJob {
            job,
            class,
            work,
            timing,
        } = scheduled;
        match job {
            CryptoJob::VerifySignature(job) => {
                if !matches!(
                    jobs.peek(),
                    Some(ScheduledCryptoJob {
                        job: CryptoJob::VerifySignature(_),
                        ..
                    })
                ) {
                    if !run_and_publish_crypto_job(
                        ScheduledCryptoJob {
                            job: CryptoJob::VerifySignature(job),
                            class,
                            work,
                            timing,
                        },
                        verifier_cache,
                        state,
                        results,
                        completion_wake,
                    ) {
                        return false;
                    }
                    continue;
                }
                let mut verification_jobs = HeaplessVec::new();
                if verification_jobs
                    .push(ScheduledVerifyJob {
                        job,
                        work,
                        timing: timing.start(),
                    })
                    .is_err()
                {
                    return false;
                }
                while let Some(ScheduledCryptoJob {
                    job: CryptoJob::VerifySignature(job),
                    work,
                    timing,
                    ..
                }) =
                    jobs.next_if(|scheduled| matches!(scheduled.job, CryptoJob::VerifySignature(_)))
                {
                    if verification_jobs
                        .push(ScheduledVerifyJob {
                            job,
                            work,
                            timing: timing.start(),
                        })
                        .is_err()
                    {
                        return false;
                    }
                }
                if !run_and_publish_verification_jobs(
                    verification_jobs,
                    verifier_cache,
                    batch_failure_cooldown,
                    state,
                    results,
                    completion_wake,
                ) {
                    return false;
                }
            }
            CryptoJob::SignLink(job) => {
                let mut sign_jobs = HeaplessVec::new();
                if sign_jobs
                    .push(ScheduledLinkSignJob {
                        job,
                        work,
                        timing: timing.start(),
                    })
                    .is_err()
                {
                    return false;
                }
                while let Some(ScheduledCryptoJob {
                    job: CryptoJob::SignLink(job),
                    work,
                    timing,
                    ..
                }) = jobs.next_if(|scheduled| matches!(scheduled.job, CryptoJob::SignLink(_)))
                {
                    if sign_jobs
                        .push(ScheduledLinkSignJob {
                            job,
                            work,
                            timing: timing.start(),
                        })
                        .is_err()
                    {
                        return false;
                    }
                }
                if !run_and_publish_link_sign_jobs(sign_jobs, state, results, completion_wake) {
                    return false;
                }
            }
            job => {
                if !run_and_publish_crypto_job(
                    ScheduledCryptoJob {
                        job,
                        class,
                        work,
                        timing,
                    },
                    verifier_cache,
                    state,
                    results,
                    completion_wake,
                ) {
                    return false;
                }
            }
        }
    }
    true
}

struct ScheduledLinkSignJob {
    job: LinkSignJob,
    work: usize,
    timing: JobExecutionTimer,
}

fn run_and_publish_link_sign_jobs(
    jobs: HeaplessVec<ScheduledLinkSignJob, MAX_INTERACTIVE_CRYPTO_BATCH>,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
) -> bool {
    let mut completed = HeaplessVec::<ScheduledCryptoResult, MAX_INTERACTIVE_CRYPTO_BATCH>::new();
    for ScheduledLinkSignJob { job, work, timing } in jobs {
        if completed
            .push(ScheduledCryptoResult {
                result: run_link_sign_job(job).into(),
                class: CryptoJobClass::Latency,
                work,
                timing: timing.finish(),
            })
            .is_err()
        {
            return false;
        }
    }
    publish_crypto_results(completed, state, results, completion_wake)
}

// Results move through the worker ring. Boxing Identify would add an allocation
// and pointer chase to every completed identify solely to shrink this enum.
#[allow(clippy::large_enum_variant)]
pub(super) enum LinkSignCompleted {
    ChannelAck(ChannelAckSignCompleted),
    Receipt(LinkReceiptSignCompleted),
    Identify(IdentifySignCompleted),
}

impl From<LinkSignCompleted> for CryptoResult {
    fn from(result: LinkSignCompleted) -> Self {
        match result {
            LinkSignCompleted::ChannelAck(completed) => Self::ChannelAckSigned(completed),
            LinkSignCompleted::Receipt(completed) => Self::LinkReceiptSigned(completed),
            LinkSignCompleted::Identify(completed) => Self::IdentifySigned(completed),
        }
    }
}

pub(super) fn run_link_sign_job(job: LinkSignJob) -> LinkSignCompleted {
    match job {
        LinkSignJob::ChannelAck(owed) => {
            let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
            LinkSignCompleted::ChannelAck(ChannelAckSignCompleted {
                target: owed.target,
                link_id: owed.link_id,
                packet_hash: owed.packet_hash,
                signature,
            })
        }
        LinkSignJob::Receipt(owed) => {
            let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
            LinkSignCompleted::Receipt(LinkReceiptSignCompleted {
                target: owed.target,
                link_id: owed.link_id,
                packet_hash: owed.packet_hash,
                signature,
            })
        }
        LinkSignJob::Identify(owed) => {
            let signature = ed25519_sign(&owed.signing_secret, &owed.signed_data);
            LinkSignCompleted::Identify(IdentifySignCompleted { owed, signature })
        }
    }
}

struct ScheduledVerifyJob {
    job: SignatureVerifyJob,
    work: usize,
    timing: JobExecutionTimer,
}

fn run_and_publish_verification_jobs(
    jobs: HeaplessVec<ScheduledVerifyJob, MAX_INTERACTIVE_CRYPTO_BATCH>,
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
        let ScheduledVerifyJob { job, work, timing } = scheduled;
        let valid = if batch_valid {
            true
        } else {
            let (public, message, signature) = job.inputs();
            cached_verifier(verifier_cache, public)
                .is_some_and(|verifier| verifier.verify(message, signature).is_ok())
        };
        if !publish_crypto_result(
            job.complete(valid),
            CryptoJobClass::Verify,
            work,
            state,
            results,
            completion_wake,
            timing.finish(),
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
        let (public, _, _) = job.inputs();
        cached_verifier(verifier_cache, public)?;
    }

    let mut messages: HeaplessVec<&[u8], MAX_INTERACTIVE_CRYPTO_BATCH> = HeaplessVec::new();
    let mut signatures: HeaplessVec<Ed25519Signature, MAX_INTERACTIVE_CRYPTO_BATCH> =
        HeaplessVec::new();
    let mut verifiers: HeaplessVec<&Ed25519Verifier, MAX_INTERACTIVE_CRYPTO_BATCH> =
        HeaplessVec::new();
    for ScheduledVerifyJob { job, .. } in jobs {
        let (public, message, signature) = job.inputs();
        let verifier = verifier_cache
            .iter()
            .flatten()
            .find(|verifier| verifier.public_key() == public)?;
        if verifier.is_weak() {
            return None;
        }
        if messages.push(message).is_err()
            || signatures.push(*signature).is_err()
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
    let ScheduledCryptoJob {
        job,
        class,
        work,
        timing,
    } = scheduled;
    let timing = timing.start();
    let result = run_crypto_job(job, verifier_cache);
    publish_crypto_result(
        result,
        class,
        work,
        state,
        results,
        completion_wake,
        timing.finish(),
    )
}

fn publish_crypto_result(
    result: CryptoResult,
    class: CryptoJobClass,
    work: usize,
    state: &CryptoPoolState,
    results: &mut Producer<ScheduledCryptoResult>,
    completion_wake: &Notify,
    timing: CompletedJobTiming,
) -> bool {
    let mut pending = ScheduledCryptoResult {
        result,
        class,
        work,
        timing,
    };
    // Reserve readiness before publishing into the ring. The manifold may be draining a different
    // worker concurrently; counting first prevents it from observing an uncounted result between
    // the ring's publish and a later atomic increment.
    state.ready_results.fetch_add(1, Ordering::Release);
    loop {
        if state.shutdown.load(Ordering::Acquire) {
            state.ready_results.fetch_sub(1, Ordering::Release);
            return false;
        }
        match results.push(pending) {
            Ok(()) => {
                notify_completion_if_armed(state, completion_wake);
                return true;
            }
            Err(PushError::Full(returned)) => {
                pending = returned;
                notify_completion_if_armed(state, completion_wake);
                std::thread::yield_now();
            }
        }
    }
}

fn publish_crypto_results(
    pending: HeaplessVec<ScheduledCryptoResult, MAX_INTERACTIVE_CRYPTO_BATCH>,
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
