use super::{admission, reply, Error, Guard, Job, Profile, ProfileId};
use crate::attachment_scope::AttachmentScope;
use prns_host::InterfaceId;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::Waker,
};
use tokio::sync::oneshot;

pub(super) type CloseFuture =
    Pin<Box<dyn Future<Output = Result<(), crate::attachment_scope::AttachmentError>> + Send>>;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
const MAX_CLOSE_WAITERS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingPhase {
    Active,
    Fenced,
    Closing,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentState {
    Attaching,
    Attached,
    Retiring,
    Failed,
}
#[derive(Clone, Debug)]
pub struct LiveProfile {
    pub profile: Profile,
    pub interface: Option<InterfaceId>,
    pub state: AttachmentState,
    pub failure: Option<Error>,
}
#[derive(Clone, Debug)]
pub struct BindingSnapshot {
    pub generation: u64,
    pub saved: Guard,
    pub applied_revision: Option<u64>,
    pub phase: BindingPhase,
    /// Includes removed profiles until their actual retirement completes.
    pub attachments: Vec<LiveProfile>,
}
#[derive(Clone, Debug)]
pub struct BindingStatus {
    pub generation: u64,
    pub phase: BindingPhase,
    /// Latest cleanup failure; a failed attempt does not assert resource release.
    pub cleanup_failure: Option<Error>,
}
struct CompletionState {
    finished: bool,
    failure: Option<(u64, Error)>,
    waiters: Vec<(u64, oneshot::Sender<Result<(), Error>>)>,
}
pub(super) struct Control {
    pub generation: u64,
    pub scope: Arc<AttachmentScope>,
    pub closing: AtomicBool,
    pub fenced: AtomicBool,
    pub close_attempt: AtomicU64,
    pub progress: Waker,
    completion: Mutex<CompletionState>,
    pending_close: Mutex<Option<(u64, CloseFuture)>>,
    // Linearizes a retry's actual bounded insertion with the close fence.
    admission: Mutex<bool>,
}
impl Control {
    pub fn new(scope: Arc<AttachmentScope>, progress: Waker) -> Result<Self, Error> {
        let generation = NEXT_GENERATION
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
            .map_err(|_| Error::Busy)?;
        Ok(Self {
            generation,
            scope,
            closing: AtomicBool::new(false),
            fenced: AtomicBool::new(false),
            close_attempt: AtomicU64::new(0),
            progress,
            completion: Mutex::new(CompletionState {
                finished: false,
                failure: None,
                waiters: Vec::new(),
            }),
            admission: Mutex::new(false),
            pending_close: Mutex::new(None),
        })
    }
    pub fn is_finished(&self) -> bool {
        crate::lock(&self.completion).finished
    }
    pub fn fence(&self) {
        *crate::lock(&self.admission) = true;
        self.fenced.store(true, Ordering::Release);
        self.scope.fence();
        self.progress.wake_by_ref();
    }
    pub fn request_close(&self) {
        self.fence();
        self.closing.store(true, Ordering::Release);
        let attempt = self.close_attempt.fetch_add(1, Ordering::AcqRel) + 1;
        // close() fences synchronously and returns this exact retained attempt.
        // Never call it again merely to observe: that could retry a fast failure.
        let future: CloseFuture = Box::pin(self.scope.close());
        let discarded = {
            let mut pending = crate::lock(&self.pending_close);
            if attempt == self.close_attempt.load(Ordering::Acquire) {
                pending.replace((attempt, future))
            } else {
                Some((attempt, future))
            }
        };
        drop(discarded);
        self.progress.wake_by_ref();
    }
    pub fn take_close(&self) -> Option<(u64, CloseFuture)> {
        crate::lock(&self.pending_close).take()
    }
    pub fn status(&self) -> BindingStatus {
        let completion = crate::lock(&self.completion);
        BindingStatus {
            generation: self.generation,
            phase: if completion.finished {
                BindingPhase::Closed
            } else if self.closing.load(Ordering::Acquire) {
                BindingPhase::Closing
            } else if self.fenced.load(Ordering::Acquire) {
                BindingPhase::Fenced
            } else {
                BindingPhase::Active
            },
            cleanup_failure: completion.failure.as_ref().map(|(_, e)| e.clone()),
        }
    }
    pub fn failed(&self, attempt: u64, error: Error) {
        let waiters = {
            let mut state = crate::lock(&self.completion);
            if state
                .failure
                .as_ref()
                .is_none_or(|(previous, _)| *previous <= attempt)
            {
                state.failure = Some((attempt, error.clone()));
            }
            // An earlier attempt cannot fail a waiter registered for a later
            // explicit retry, even if publication races that new request.
            let (ready, later): (Vec<_>, Vec<_>) = std::mem::take(&mut state.waiters)
                .into_iter()
                .partition(|(waiting, _)| *waiting <= attempt);
            state.waiters = later;
            ready
        };
        for (_, waiter) in waiters {
            reply(waiter, Err(error.clone()));
        }
    }
    pub fn finish(&self) {
        let waiters = {
            let mut s = crate::lock(&self.completion);
            s.finished = true;
            std::mem::take(&mut s.waiters)
        };
        for (_, waiter) in waiters {
            reply(waiter, Ok(()));
        }
    }
    async fn wait(self: Arc<Self>) -> Result<(), Error> {
        let (send, receive) = oneshot::channel();
        let removed;
        {
            let mut s = crate::lock(&self.completion);
            if s.finished {
                return Ok(());
            }
            if let Some((attempt, error)) = &s.failure {
                if *attempt == self.close_attempt.load(Ordering::Acquire) {
                    return Err(error.clone());
                }
            }
            // Reclaim cancelled observers, but drop senders outside the mutex.
            let (live, closed): (Vec<_>, Vec<_>) = std::mem::take(&mut s.waiters)
                .into_iter()
                .partition(|(_, waiter)| !waiter.is_closed());
            s.waiters = live;
            removed = closed;
            if s.waiters.len() >= MAX_CLOSE_WAITERS {
                return Err(Error::Busy);
            }
            s.waiters
                .push((self.close_attempt.load(Ordering::Acquire), send));
        }
        drop(removed);
        receive.await.map_err(|_| Error::WorkerFailed)?
    }
}

/// Owns one host generation's reconciliation. Dropping this owner fences and
/// retains cleanup; dropping an async operation never cancels accepted work.
pub struct HostBinding {
    pub(super) control: Arc<Control>,
    lane: prns_storage::Client<Job>,
}
impl HostBinding {
    pub(super) fn new(control: Arc<Control>, lane: prns_storage::Client<Job>) -> Self {
        Self { control, lane }
    }
    /// Deny new reconciliation/retries without retiring existing interfaces.
    /// Use before draining protocols that still need those interfaces.
    pub fn fence(&self) {
        self.control.fence();
    }
    pub fn status(&self) -> BindingStatus {
        self.control.status()
    }
    pub fn client(&self) -> BindingClient {
        BindingClient {
            control: self.control.clone(),
            lane: self.lane.clone(),
        }
    }
    /// Fences immediately; retrying after a reported teardown failure starts a
    /// new cleanup attempt. The profile store remains available while unbound.
    pub fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
        self.control.request_close();
        Box::pin(self.control.clone().wait())
    }
    pub fn closed(&self) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
        Box::pin(self.control.clone().wait())
    }
}
impl Drop for HostBinding {
    fn drop(&mut self) {
        self.control.request_close();
    }
}

/// Borrowed observation/retry capability. Keeping a client never postpones its
/// owner's fence, cleanup or finalization. It has no close/stop authority.
#[derive(Clone)]
pub struct BindingClient {
    control: Arc<Control>,
    lane: prns_storage::Client<Job>,
}
impl BindingClient {
    pub fn status(&self) -> BindingStatus {
        self.control.status()
    }
    pub async fn snapshot(&self) -> Result<BindingSnapshot, Error> {
        let (send, receive) = oneshot::channel();
        self.lane
            .try_submit(Job::BindingSnapshot {
                generation: self.control.generation,
                reply: send,
            })
            .map_err(admission)?;
        receive.await.map_err(|_| Error::WorkerFailed)?
    }
    /// Accept one revision-checked reconciliation request. Success acknowledges
    /// admission, not connectivity. Inspect the binding after a lost reply.
    pub async fn retry(&self, expected: Guard, profile: ProfileId) -> Result<(), Error> {
        let (send, receive) = oneshot::channel();
        {
            let closed = crate::lock(&self.control.admission);
            if *closed {
                return Err(Error::Closed);
            }
            self.lane
                .try_submit(Job::Retry {
                    generation: self.control.generation,
                    expected,
                    profile,
                    reply: send,
                })
                .map_err(admission)?;
        }
        receive.await.map_err(|_| Error::WorkerFailed)?
    }
}
