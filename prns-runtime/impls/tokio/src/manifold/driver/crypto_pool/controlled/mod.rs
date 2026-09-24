use super::*;
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlledWorkerId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlledJobId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledWorkKind {
    VerifySignature,
    BuildResource,
    HashResourcePart,
    DecompressResource,
    SealStaged,
    OpenSpan,
    SealScalars,
    SignProof,
    Decrypt,
    DecryptWithRatchets,
    VerifyLinkProof,
    SignLinkProof,
    SignLink,
    SignTunnel,
    EstablishLink,
    SignAnnounce,
    VerifyAnnounce,
    VerifyPairing,
    #[cfg(test)]
    Probe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoWorkBoundary {
    Execution,
    Publication,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlledCryptoEvent {
    Queued {
        job: ControlledJobId,
        worker: ControlledWorkerId,
        kind: ControlledWorkKind,
    },
    Executed {
        job: ControlledJobId,
    },
    Published {
        job: ControlledJobId,
    },
    Consumed {
        job: ControlledJobId,
    },
    Retired {
        occupancy: ControlledCryptoSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlledCryptoSnapshot {
    pub queued: usize,
    pub computed: usize,
    pub published: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ControlledCryptoStep {
    Idle,
    Held,
    Backpressured,
    Executed(ControlledJobId),
    Published(ControlledJobId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledCryptoError {
    NotAttached,
    AlreadyAttached,
    Retired,
    WorkerOutOfRange,
    TraceCapacity,
    JobIdsExhausted,
    OwnershipMismatch,
    Poisoned,
}

impl std::fmt::Display for ControlledCryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "controlled crypto: {self:?}")
    }
}
impl std::error::Error for ControlledCryptoError {}

#[derive(Clone)]
pub struct ControlledCrypto(Arc<Mutex<Control>>);

impl std::fmt::Debug for ControlledCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlledCrypto").finish_non_exhaustive()
    }
}

struct JobIdentity {
    id: ControlledJobId,
    kind: ControlledWorkKind,
}
struct PendingResult {
    identity: JobIdentity,
    result: CryptoResult,
    class: CryptoJobClass,
    work: usize,
}
struct Worker {
    interactive: Consumer<ScheduledCryptoJob>,
    bulk: Consumer<ScheduledCryptoJob>,
    results: Producer<ScheduledCryptoResult>,
    interactive_ids: VecDeque<JobIdentity>,
    bulk_ids: VecDeque<JobIdentity>,
    pending: VecDeque<PendingResult>,
    published: VecDeque<ControlledJobId>,
    cache: WorkerVerifierCache,
}
struct Attached {
    state: Arc<CryptoPoolState>,
    wake: Arc<Notify>,
    workers: Vec<Worker>,
}
enum Attachment {
    Unattached,
    Attached(Attached),
    Retired,
}
struct Control {
    workers: NonZeroUsize,
    trace_capacity: NonZeroUsize,
    trace: Vec<ControlledCryptoEvent>,
    next_id: u64,
    next_worker: usize,
    holds: Vec<(ControlledWorkKind, CryptoWorkBoundary)>,
    attachment: Attachment,
}
impl Control {
    fn require_trace_slot(&self) -> Result<(), ControlledCryptoError> {
        if self.trace.len() == self.trace_capacity.get() {
            return Err(ControlledCryptoError::TraceCapacity);
        }
        Ok(())
    }
    fn record(&mut self, event: ControlledCryptoEvent) -> Result<(), ControlledCryptoError> {
        self.require_trace_slot()?;
        self.trace.push(event);
        Ok(())
    }
    fn attached(&mut self) -> Result<&mut Attached, ControlledCryptoError> {
        match &mut self.attachment {
            Attachment::Unattached => Err(ControlledCryptoError::NotAttached),
            Attachment::Attached(attached) => Ok(attached),
            Attachment::Retired => Err(ControlledCryptoError::Retired),
        }
    }
}

impl ControlledCrypto {
    pub fn new(workers: NonZeroUsize, trace_capacity: NonZeroUsize) -> Self {
        Self(Arc::new(Mutex::new(Control {
            workers,
            trace_capacity,
            trace: Vec::new(),
            next_id: 0,
            next_worker: 0,
            holds: Vec::new(),
            attachment: Attachment::Unattached,
        })))
    }
    fn lock(&self) -> Result<MutexGuard<'_, Control>, ControlledCryptoError> {
        self.0.lock().map_err(|_| ControlledCryptoError::Poisoned)
    }
    pub fn hold(
        &self,
        kind: ControlledWorkKind,
        boundary: CryptoWorkBoundary,
    ) -> Result<(), ControlledCryptoError> {
        let mut control = self.lock()?;
        if !control.holds.contains(&(kind, boundary)) {
            control.holds.push((kind, boundary));
        }
        Ok(())
    }
    pub fn release(
        &self,
        kind: ControlledWorkKind,
        boundary: CryptoWorkBoundary,
    ) -> Result<(), ControlledCryptoError> {
        self.lock()?
            .holds
            .retain(|entry| *entry != (kind, boundary));
        Ok(())
    }
    pub fn trace(&self) -> Result<Vec<ControlledCryptoEvent>, ControlledCryptoError> {
        Ok(self.lock()?.trace.clone())
    }
    pub fn snapshot(&self) -> Result<ControlledCryptoSnapshot, ControlledCryptoError> {
        let mut control = self.lock()?;
        Ok(occupancy(control.attached()?))
    }
    pub fn execute(
        &self,
        worker: ControlledWorkerId,
    ) -> Result<ControlledCryptoStep, ControlledCryptoError> {
        let mut control = self.lock()?;
        control.require_trace_slot()?;
        let holds = control.holds.clone();
        let attached = control.attached()?;
        let slot = attached
            .workers
            .get_mut(worker.0)
            .ok_or(ControlledCryptoError::WorkerOutOfRange)?;
        if slot.pending.len() == CRYPTO_WORKER_RESULT_RING_DEPTH {
            return Ok(ControlledCryptoStep::Backpressured);
        }
        let (jobs, identities) = if slot.interactive.slots() != 0 {
            (&mut slot.interactive, &mut slot.interactive_ids)
        } else {
            (&mut slot.bulk, &mut slot.bulk_ids)
        };
        let Some(identity) = identities.front() else {
            return Ok(ControlledCryptoStep::Idle);
        };
        if holds.contains(&(identity.kind, CryptoWorkBoundary::Execution)) {
            return Ok(ControlledCryptoStep::Held);
        }
        let scheduled = jobs
            .pop()
            .map_err(|_| ControlledCryptoError::OwnershipMismatch)?;
        let identity = identities
            .pop_front()
            .ok_or(ControlledCryptoError::OwnershipMismatch)?;
        if identity.kind != work_kind(&scheduled.job) {
            return Err(ControlledCryptoError::OwnershipMismatch);
        }
        attached.state.queued_jobs.fetch_sub(1, Ordering::Release);
        let result = run_crypto_job(scheduled.job, &mut slot.cache);
        let id = identity.id;
        slot.pending.push_back(PendingResult {
            identity,
            result,
            class: scheduled.class,
            work: scheduled.work,
        });
        control.record(ControlledCryptoEvent::Executed { job: id })?;
        Ok(ControlledCryptoStep::Executed(id))
    }
    pub fn publish(
        &self,
        worker: ControlledWorkerId,
    ) -> Result<ControlledCryptoStep, ControlledCryptoError> {
        let mut control = self.lock()?;
        control.require_trace_slot()?;
        let holds = control.holds.clone();
        let attached = control.attached()?;
        let slot = attached
            .workers
            .get_mut(worker.0)
            .ok_or(ControlledCryptoError::WorkerOutOfRange)?;
        let Some(pending) = slot.pending.front() else {
            return Ok(ControlledCryptoStep::Idle);
        };
        if holds.contains(&(pending.identity.kind, CryptoWorkBoundary::Publication)) {
            return Ok(ControlledCryptoStep::Held);
        }
        if slot.results.slots() == 0 {
            return Ok(ControlledCryptoStep::Backpressured);
        }
        let pending = slot
            .pending
            .pop_front()
            .ok_or(ControlledCryptoError::OwnershipMismatch)?;
        let id = pending.identity.id;
        if !publish_crypto_result(
            pending.result,
            pending.class,
            pending.work,
            &attached.state,
            &mut slot.results,
            &attached.wake,
            CompletedJobTiming::unmeasured(),
        ) {
            return Err(ControlledCryptoError::Retired);
        }
        slot.published.push_back(id);
        control.record(ControlledCryptoEvent::Published { job: id })?;
        Ok(ControlledCryptoStep::Published(id))
    }
    /// One eligible worker transition. Idle or held work never advances the scenario clock.
    pub fn step(&self) -> Result<ControlledCryptoStep, ControlledCryptoError> {
        let (first, count) = {
            let mut control = self.lock()?;
            control.attached()?;
            let first = control.next_worker;
            control.next_worker = (first + 1) % control.workers.get();
            (first, control.workers.get())
        };
        let mut blocked = ControlledCryptoStep::Idle;
        for offset in 0..count {
            let worker = ControlledWorkerId((first + offset) % count);
            let published = self.publish(worker)?;
            if matches!(published, ControlledCryptoStep::Published(_)) {
                return Ok(published);
            }
            let executed = self.execute(worker)?;
            if matches!(executed, ControlledCryptoStep::Executed(_)) {
                return Ok(executed);
            }
            for step in [published, executed] {
                match step {
                    ControlledCryptoStep::Held => blocked = ControlledCryptoStep::Held,
                    ControlledCryptoStep::Backpressured
                        if blocked == ControlledCryptoStep::Idle =>
                    {
                        blocked = ControlledCryptoStep::Backpressured
                    }
                    ControlledCryptoStep::Idle | ControlledCryptoStep::Backpressured => {}
                    ControlledCryptoStep::Executed(_) | ControlledCryptoStep::Published(_) => {
                        unreachable!("progress returns immediately")
                    }
                }
            }
        }
        Ok(blocked)
    }
    #[expect(
        clippy::panic,
        reason = "invalid validation assembly must fail rather than run inline"
    )]
    pub(in crate::manifold::driver) fn spawn(&self, wake: Arc<Notify>) -> CryptoPool {
        self.attach(wake).unwrap_or_else(|error| panic!("{error}"))
    }
    fn attach(&self, wake: Arc<Notify>) -> Result<CryptoPool, ControlledCryptoError> {
        let mut control = self.lock()?;
        if !matches!(control.attachment, Attachment::Unattached) {
            return Err(ControlledCryptoError::AlreadyAttached);
        }
        let count = control.workers.get();
        let state = Arc::new(CryptoPoolState {
            queued_jobs: AtomicUsize::new(0),
            completion_readiness: CompletionReadiness::new(),
            warm_worker: AtomicUsize::new(NO_WARM_WORKER),
            backpressure_depth: crypto_backpressure_depth(count),
            shutdown: AtomicBool::new(false),
        });
        let layout = CryptoWorkerLayout::resolve(CryptoWorkerPlacement::SchedulerManaged, count);
        let mut workers = Vec::with_capacity(count);
        let mut slots = Vec::with_capacity(count);
        for worker in 0..count {
            let (interactive_producer, interactive) = RingBuffer::new(CRYPTO_WORKER_JOB_RING_DEPTH);
            let (bulk_producer, bulk) = RingBuffer::new(CRYPTO_WORKER_JOB_RING_DEPTH);
            let (results, result_consumer) = RingBuffer::new(CRYPTO_WORKER_RESULT_RING_DEPTH);
            workers.push(Worker {
                interactive,
                bulk,
                results,
                interactive_ids: VecDeque::new(),
                bulk_ids: VecDeque::new(),
                pending: VecDeque::new(),
                published: VecDeque::new(),
                cache: core::array::from_fn(|_| None),
            });
            slots.push(CryptoWorker {
                interactive_job_producer: RefCell::new(Some(interactive_producer)),
                bulk_job_producer: RefCell::new(Some(bulk_producer)),
                result_consumer: RefCell::new(Some(result_consumer)),
                wake_on_submit: Arc::new(AtomicBool::new(false)),
                handle: None,
                role: layout.role(worker),
                outstanding_jobs: Cell::new(0),
                outstanding_work: Cell::new(0),
                tail_class: Cell::new(None),
                tail_run: Cell::new(0),
            });
        }
        control.attachment = Attachment::Attached(Attached {
            state: state.clone(),
            wake,
            workers,
        });
        Ok(CryptoPool {
            control: Some(self.clone()),
            state,
            workers: slots,
            verify_batch_target: verify_batch_target(count, count),
            maximum_outstanding_work: crypto_backpressure_work(count),
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
    #[expect(
        clippy::panic,
        reason = "trace and identity budget failures are qualification failures"
    )]
    pub(super) fn queued(&self, worker: usize, kind: ControlledWorkKind, class: CryptoJobClass) {
        self.queue_identity(worker, kind, class)
            .unwrap_or_else(|error| panic!("{error}"));
    }
    fn queue_identity(
        &self,
        worker: usize,
        kind: ControlledWorkKind,
        class: CryptoJobClass,
    ) -> Result<(), ControlledCryptoError> {
        let mut control = self.lock()?;
        let id = ControlledJobId(control.next_id);
        control.next_id = control
            .next_id
            .checked_add(1)
            .ok_or(ControlledCryptoError::JobIdsExhausted)?;
        let slot = &mut control.attached()?.workers[worker];
        let ids = match class {
            CryptoJobClass::Bulk => &mut slot.bulk_ids,
            CryptoJobClass::Latency | CryptoJobClass::Verify => &mut slot.interactive_ids,
        };
        ids.push_back(JobIdentity { id, kind });
        control.record(ControlledCryptoEvent::Queued {
            job: id,
            worker: ControlledWorkerId(worker),
            kind,
        })
    }
    #[expect(
        clippy::panic,
        reason = "completion ownership must match its actual worker ring"
    )]
    pub(super) fn consumed(&self, worker: usize) {
        let result = (|| {
            let mut control = self.lock()?;
            let id = control.attached()?.workers[worker]
                .published
                .pop_front()
                .ok_or(ControlledCryptoError::OwnershipMismatch)?;
            control.record(ControlledCryptoEvent::Consumed { job: id })
        })();
        result.unwrap_or_else(|error| panic!("{error}"));
    }
    #[expect(clippy::panic, reason = "retirement evidence cannot silently overflow")]
    pub(super) fn retire(&self) {
        let result = (|| {
            let mut control = self.lock()?;
            let snapshot = occupancy(control.attached()?);
            control.attachment = Attachment::Retired;
            control.record(ControlledCryptoEvent::Retired {
                occupancy: snapshot,
            })
        })();
        result.unwrap_or_else(|error| panic!("{error}"));
    }
}

fn occupancy(attached: &Attached) -> ControlledCryptoSnapshot {
    ControlledCryptoSnapshot {
        queued: attached.state.queued_jobs.load(Ordering::Acquire),
        computed: attached
            .workers
            .iter()
            .map(|worker| worker.pending.len())
            .sum(),
        published: attached
            .workers
            .iter()
            .map(|worker| worker.published.len())
            .sum(),
    }
}

pub(super) fn work_kind(job: &CryptoJob) -> ControlledWorkKind {
    match job {
        CryptoJob::VerifySignature(_) => ControlledWorkKind::VerifySignature,
        CryptoJob::BuildResource(_) => ControlledWorkKind::BuildResource,
        CryptoJob::HashResourcePart(_) => ControlledWorkKind::HashResourcePart,
        CryptoJob::DecompressResource(_) => ControlledWorkKind::DecompressResource,
        CryptoJob::SealStaged(_) => ControlledWorkKind::SealStaged,
        CryptoJob::OpenSpan(_) => ControlledWorkKind::OpenSpan,
        CryptoJob::SealScalars(_) => ControlledWorkKind::SealScalars,
        CryptoJob::SignProof(_) => ControlledWorkKind::SignProof,
        CryptoJob::Decrypt(_) => ControlledWorkKind::Decrypt,
        CryptoJob::DecryptWithRatchets(_) => ControlledWorkKind::DecryptWithRatchets,
        CryptoJob::VerifyLinkProof(_) => ControlledWorkKind::VerifyLinkProof,
        CryptoJob::SignLinkProof(_) => ControlledWorkKind::SignLinkProof,
        CryptoJob::SignLink(_) => ControlledWorkKind::SignLink,
        CryptoJob::SignTunnelSynthesize(_) => ControlledWorkKind::SignTunnel,
        CryptoJob::EstablishLink(_) => ControlledWorkKind::EstablishLink,
        CryptoJob::SignAnnounce(_) => ControlledWorkKind::SignAnnounce,
        CryptoJob::VerifyAnnounce(_) => ControlledWorkKind::VerifyAnnounce,
        CryptoJob::VerifyRemoteControlPairingAvailability(_) => ControlledWorkKind::VerifyPairing,
        #[cfg(test)]
        CryptoJob::ScheduledTest(_) => ControlledWorkKind::Probe,
    }
}
