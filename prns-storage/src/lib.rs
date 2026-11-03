//! A bounded blocking lane with one explicit owner and borrowed submission scopes.
//!
//! The initializer creates resources on the worker thread, then calls
//! [`Inbox::run`] while any borrowing managers remain in lexical scope. Jobs own
//! their result channels and transactions; this crate never opens a transaction.
//! Accepted jobs run even when their caller drops its result receiver. Bound each
//! job's work: FIFO scheduling cannot preempt a blocking or unbounded handler.
//!
//! [`Worker::close`] fences admission outside the ordinary queue and joins after
//! accepted work drains. Dropping the owner initiates the same fence, but does not
//! wait or claim successful shutdown. Dropping a client does neither. Scopes fence
//! and drain their own jobs without stopping other users of the physical owner.

#![forbid(unsafe_code)]

mod completion;
#[cfg(feature = "redb")]
pub mod database;
pub mod files;
pub use completion::Completion;

use std::{
    collections::VecDeque,
    fmt,
    num::NonZeroUsize,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex, MutexGuard, Weak,
    },
    task::{Context, Wake, Waker},
    thread::{self, JoinHandle, ThreadId},
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Startup did not produce an initialized, owned worker.
#[derive(Debug)]
pub enum StartError<E> {
    Spawn(std::io::Error),
    Initialization(E),
    /// The initializer returned without entering [`Inbox::run`].
    NotReady,
    Panicked,
}
impl<E: fmt::Debug> fmt::Display for StartError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "storage worker startup: {self:?}")
    }
}
impl<E: fmt::Debug> std::error::Error for StartError<E> {}

/// Shutdown preserves the distinction between handler failure and thread panic.
#[derive(Debug)]
pub enum CloseError<E> {
    Failed(Arc<E>),
    Panicked,
    /// Joining the current worker would deadlock.
    Reentrant,
}
impl<E> Clone for CloseError<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Failed(error) => Self::Failed(Arc::clone(error)),
            Self::Panicked => Self::Panicked,
            Self::Reentrant => Self::Reentrant,
        }
    }
}
impl<E: fmt::Debug> fmt::Display for CloseError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "storage worker shutdown: {self:?}")
    }
}
impl<E: fmt::Debug> std::error::Error for CloseError<E> {}

/// Failed admission returns the unaccepted job to its caller.
#[derive(Debug)]
pub enum SubmitError<J> {
    Full(J),
    Closed(J),
    /// Handlers cannot enqueue work onto their own lane and then wait for it.
    Reentrant(J),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainError {
    Reentrant,
    WorkerFailed,
}

#[derive(Default)]
struct ScopeState {
    closed: bool,
    pending: usize,
    failed: bool,
}
#[derive(Default)]
struct Scope {
    state: Mutex<ScopeState>,
    drained: Condvar,
}
struct Queued<J> {
    job: J,
    scope: Arc<Scope>,
}
struct Queue<J> {
    jobs: VecDeque<Queued<J>>,
    control: Option<Queued<J>>,
    closed: bool,
    progress: bool,
    thread: Option<ThreadId>,
}
/// Read-only observation of the physical worker's irreversible close fence.
/// It retains neither the worker nor its resources and carries no authority to
/// submit work. Closure is visible synchronously before request_close/Drop
/// returns, including while an ordinary handler is blocked.
#[derive(Clone)]
pub struct CloseSignal(Arc<AtomicBool>);
impl CloseSignal {
    pub fn is_closed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

struct Shared<J> {
    queue: Mutex<Queue<J>>,
    available: Condvar,
    capacity: NonZeroUsize,
    close: CloseSignal,
}
impl<J> Shared<J> {
    fn request_close(&self) {
        {
            let mut queue = lock(&self.queue);
            queue.closed = true;
            self.close.0.store(true, Ordering::Release);
        }
        self.available.notify_all();
    }
    fn is_current_thread(&self) -> bool {
        lock(&self.queue).thread == Some(thread::current().id())
    }
}

/// A submission capability. Its lifetime never controls worker shutdown.
pub struct Client<J> {
    shared: Weak<Shared<J>>,
    scope: Arc<Scope>,
}
impl<J> Clone for Client<J> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            scope: Arc::clone(&self.scope),
        }
    }
}
impl<J> Client<J> {
    /// Capacity bounds ordinary queued jobs; one additional job may execute.
    pub fn try_submit(&self, job: J) -> Result<(), SubmitError<J>> {
        self.submit(job, false)
    }
    fn submit(&self, job: J, control: bool) -> Result<(), SubmitError<J>> {
        let Some(shared) = self.shared.upgrade() else {
            return Err(SubmitError::Closed(job));
        };
        let mut queue = lock(&shared.queue);
        if queue.thread == Some(thread::current().id()) {
            return Err(SubmitError::Reentrant(job));
        }
        let mut scope = lock(&self.scope.state);
        if queue.closed || scope.closed {
            return Err(SubmitError::Closed(job));
        }
        if if control {
            queue.control.is_some()
        } else {
            queue.jobs.len() >= shared.capacity.get()
        } {
            return Err(SubmitError::Full(job));
        }
        scope.pending += 1;
        let queued = Queued {
            job,
            scope: Arc::clone(&self.scope),
        };
        if control {
            queue.control = Some(queued);
        } else {
            queue.jobs.push_back(queued);
        }
        shared.available.notify_one();
        Ok(())
    }
}

/// Admission to the worker's single reserved lifecycle-control slot.
///
/// Keep this capability with the trusted owner that classifies lifecycle work;
/// ordinary clients cannot promote their jobs. It shares scope fencing and
/// drainage with its ordinary client. Full ordinary queues cannot occupy this
/// slot, but another pending control can. Controls do not preempt an executing
/// handler, and bounded fairness still services ordinary work and progress.
pub struct ControlClient<J>(Client<J>);
impl<J> Clone for ControlClient<J> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<J> ControlClient<J> {
    pub fn try_submit(&self, job: J) -> Result<(), SubmitError<J>> {
        self.0.submit(job, true)
    }
}

/// Owns admission for one attached user, without owning the physical worker.
pub struct ScopeOwner<J> {
    client: Client<J>,
}
impl<J> ScopeOwner<J> {
    pub fn client(&self) -> Client<J> {
        self.client.clone()
    }
    /// Borrow this scope's admission to the physical worker's reserved slot.
    pub fn control_client(&self) -> ControlClient<J> {
        ControlClient(self.client())
    }
    /// Fence this scope without waiting; already accepted work remains owned.
    pub fn request_close(&self) {
        lock(&self.client.scope.state).closed = true;
    }
    /// Fence this scope and wait for its accepted jobs. Other scopes remain open.
    /// The worker handler must not call this method on its own scope.
    pub fn close(&mut self) -> Result<(), DrainError> {
        if self
            .client
            .shared
            .upgrade()
            .is_some_and(|shared| shared.is_current_thread())
        {
            return Err(DrainError::Reentrant);
        }
        let scope = &self.client.scope;
        let mut state = lock(&scope.state);
        state.closed = true;
        while state.pending != 0 {
            state = scope
                .drained
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.failed {
            Err(DrainError::WorkerFailed)
        } else {
            Ok(())
        }
    }
}
impl<J> Drop for ScopeOwner<J> {
    fn drop(&mut self) {
        // Fencing does not cancel pending jobs or block a finalizer.
        lock(&self.client.scope.state).closed = true;
    }
}

/// Owns the worker thread. Keep this owner until explicit shutdown is inspected.
pub struct Worker<J, E> {
    shared: Arc<Shared<J>>,
    root: Arc<Scope>,
    thread: Option<JoinHandle<()>>,
    completion: Completion<E>,
}
impl<J: Send + 'static, E: Send + Sync + 'static> Worker<J, E> {
    /// Wait for explicit readiness. Initialization failures and panics are joined
    /// before returning, so a failed start cannot acquire resources later.
    pub fn start(
        name: impl Into<String>,
        capacity: NonZeroUsize,
        initialize: impl FnOnce(Inbox<J>) -> Result<(), E> + Send + 'static,
    ) -> Result<Self, StartError<E>> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                jobs: VecDeque::new(),
                control: None,
                closed: false,
                progress: false,
                thread: None,
            }),
            available: Condvar::new(),
            capacity,
            close: CloseSignal(Arc::new(AtomicBool::new(false))),
        });
        let (ready, readiness) = mpsc::sync_channel(1);
        let worker_shared = Arc::clone(&shared);
        let completion = Completion::new();
        let completed = completion.clone();
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                lock(&worker_shared.queue).thread = Some(thread::current().id());
                completed.set_thread(thread::current().id());
                // The initializer, borrowing managers, resources and abandoned
                // jobs all drop before observers can see terminal completion.
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let _exit = ExitGuard(Arc::clone(&worker_shared));
                    initialize(Inbox {
                        shared: worker_shared,
                        ready,
                    })
                }));
                let result = match outcome {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(CloseError::Failed(Arc::new(error))),
                    Err(_) => Err(CloseError::Panicked),
                };
                completed.finish(result);
            })
            .map_err(StartError::Spawn)?;
        if readiness.recv().is_err() {
            if thread.join().is_err() {
                return Err(StartError::Panicked);
            }
            // No observer has been exposed and the publisher has joined, so
            // startup still returns the original owned initialization error.
            return match completion.take_startup_result() {
                Some(Err(CloseError::Failed(error))) => match Arc::try_unwrap(error) {
                    Ok(error) => Err(StartError::Initialization(error)),
                    Err(_) => Err(StartError::Panicked),
                },
                Some(Ok(())) => Err(StartError::NotReady),
                _ => Err(StartError::Panicked),
            };
        }
        Ok(Self {
            shared,
            root: Arc::default(),
            thread: Some(thread),
            completion,
        })
    }
}
impl<J, E> Worker<J, E> {
    pub fn client(&self) -> Client<J> {
        Client {
            shared: Arc::downgrade(&self.shared),
            scope: Arc::clone(&self.root),
        }
    }
    /// Borrow admission to the single reserved lifecycle-control slot.
    pub fn control_client(&self) -> ControlClient<J> {
        ControlClient(self.client())
    }
    /// Attach a separately drainable scope. Closed workers return a closed scope.
    /// The capability only schedules jobs; domain identity binding is validated
    /// by the domain attachment before publishing this scope to its consumers.
    pub fn scope(&self) -> ScopeOwner<J> {
        ScopeOwner {
            client: Client {
                shared: Arc::downgrade(&self.shared),
                scope: Arc::default(),
            },
        }
    }
    /// Fence new submissions without waiting or consuming ordinary queue space.
    pub fn request_close(&self) {
        self.shared.request_close();
    }
    /// Observe the same synchronous fence used by close and finalization.
    pub fn close_signal(&self) -> CloseSignal {
        self.shared.close.clone()
    }
    /// Observe eventual resource cleanup without initiating shutdown or joining
    /// the OS thread. This future needs no runtime and survives owner/waiter drop.
    /// Call `request_close` first when the intent is to shut down.
    pub fn closed(&self) -> Completion<E> {
        self.completion.clone()
    }
    /// Drain accepted jobs, drop resources, and join. Every call observes the
    /// same terminal outcome; a failure never turns into success on retry.
    pub fn close(&mut self) -> Result<(), CloseError<E>> {
        if self.shared.is_current_thread() {
            return Err(CloseError::Reentrant);
        }
        self.request_close();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                self.completion.finish(Err(CloseError::Panicked));
            }
        }
        self.completion
            .status()
            .unwrap_or(Err(CloseError::Panicked))
    }
}
impl<J, E> Drop for Worker<J, E> {
    fn drop(&mut self) {
        // The thread still owns its resources and accepted queue until it drains.
        // Detaching is not a successful close; explicit close observes the result.
        self.shared.request_close();
    }
}

/// Passed only to the initializer, allowing managers to borrow local resources.
pub struct Inbox<J> {
    shared: Arc<Shared<J>>,
    ready: mpsc::SyncSender<()>,
}
/// One bounded lane event. Closing is delivered once, before the next ordinary
/// job, even when the queue is full. Progress is a coalesced wake hint; results
/// must remain in the owner's bounded native slots rather than in this signal.
pub enum OwnerEvent<J> {
    Job(J),
    Progress,
    Closing,
}

/// Whether the handler still owns work beyond its ordinary queue. Reporting
/// Drained never closes admission; it permits exit only after close and after
/// every accepted lane job has finished. Keep Pending while cleanup is unresolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerProgress {
    Pending,
    Drained,
}

struct ProgressWake<J>(Weak<Shared<J>>);
impl<J: Send + 'static> Wake for ProgressWake<J> {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(shared) = self.0.upgrade() {
            lock(&shared.queue).progress = true;
            shared.available.notify_one();
        }
    }
}

enum Next<J> {
    Job(Queued<J>, bool),
    Progress,
    Closing,
}

impl<J: Send + 'static> Inbox<J> {
    /// Construct ordinary weak admission while assembling native resources.
    /// It is the same bounded physical queue, with no control capability. Using
    /// it on this worker still returns Reentrant; construction alone dispatches
    /// nothing and never authorizes a resource before readiness.
    pub fn client(&self) -> Client<J> {
        Client {
            shared: Arc::downgrade(&self.shared),
            scope: Arc::default(),
        }
    }

    /// Share the physical close fence with already-owned external effects.
    pub fn close_signal(&self) -> CloseSignal {
        self.shared.close.clone()
    }

    /// Coalesced progress notification for resources constructed before readiness.
    /// This weak handle does not keep the worker alive and carries no result.
    /// Retain actual effects in the owner's bounded slots and use run_owned to
    /// poll them; waking after cleanup is harmless.
    pub fn progress_waker(&self) -> Waker {
        Waker::from(Arc::new(ProgressWake(Arc::downgrade(&self.shared))))
    }

    /// Publish readiness, execute ordinary jobs in FIFO order, and return after
    /// admission closes and its queue drains. Handlers run outside queue locks.
    pub fn run(self, mut execute: impl FnMut(J)) {
        self.run_owned(|event, _| {
            if let OwnerEvent::Job(job) = event {
                execute(job);
            }
            OwnerProgress::Drained
        });
    }

    /// Drive a native owner that retains asynchronous effects between lane jobs.
    /// The initializer and all borrowing state remain on this same worker.
    ///
    /// Every handler call must be bounded and nonblocking. Poll retained effects
    /// with the supplied Context; their waker coalesces progress outside ordinary
    /// queue capacity. Repeated progress wakes alternate with queued jobs, so a
    /// self-waking effect cannot starve already accepted work. Reserved controls
    /// run before backlog, alternating with ordinary work or progress when either
    /// is pending; they do not reset the ordinary/progress alternation. No executor or
    /// completion-callback thread is created here.
    ///
    /// Closing fences new admission, but accepted jobs still reach the handler.
    /// Return Pending until all additional native effects and cleanup settle;
    /// caller/owner loss does not cancel them. Scoped queue drainage only covers
    /// handler return, so a domain with retained effects must also drain its own
    /// exact scopes before releasing its attachment. A panic abandons remaining
    /// jobs and fails worker completion, as for run.
    pub fn run_owned(
        self,
        mut handle: impl FnMut(OwnerEvent<J>, &mut Context<'_>) -> OwnerProgress,
    ) {
        let waker = self.progress_waker();
        let mut context = Context::from_waker(&waker);
        // Poll initial retained effects once even if nobody has submitted work.
        lock(&self.shared.queue).progress = true;
        if self.ready.send(()).is_err() {
            self.shared.request_close();
        }
        let mut close_delivered = false;
        let mut last_progress = false;
        let mut last_control = false;
        let mut progress = OwnerProgress::Pending;
        loop {
            let next = {
                let mut queue = lock(&self.shared.queue);
                loop {
                    if queue.closed && !close_delivered {
                        close_delivered = true;
                        break Some(Next::Closing);
                    }
                    if queue.closed
                        && queue.jobs.is_empty()
                        && queue.control.is_none()
                        && progress == OwnerProgress::Drained
                    {
                        break None;
                    }
                    if !last_control || (queue.jobs.is_empty() && !queue.progress) {
                        if let Some(job) = queue.control.take() {
                            break Some(Next::Job(job, true));
                        }
                    }
                    if queue.progress && (!last_progress || queue.jobs.is_empty()) {
                        queue.progress = false;
                        break Some(Next::Progress);
                    }
                    if let Some(job) = queue.jobs.pop_front() {
                        break Some(Next::Job(job, false));
                    }
                    queue = self
                        .shared
                        .available
                        .wait(queue)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            match next {
                Some(Next::Job(Queued { job, scope }, control)) => {
                    let mut completion = JobCompletion {
                        scope,
                        finished: false,
                    };
                    progress = handle(OwnerEvent::Job(job), &mut context);
                    completion.finished = true;
                    last_control = control;
                    if !control {
                        last_progress = false;
                    }
                }
                Some(Next::Progress) => {
                    progress = handle(OwnerEvent::Progress, &mut context);
                    last_progress = true;
                    last_control = false;
                }
                Some(Next::Closing) => {
                    progress = handle(OwnerEvent::Closing, &mut context);
                    last_progress = false;
                    last_control = false;
                }
                None => return,
            }
        }
    }
}

struct JobCompletion {
    scope: Arc<Scope>,
    finished: bool,
}
impl Drop for JobCompletion {
    fn drop(&mut self) {
        let mut state = lock(&self.scope.state);
        state.pending -= 1;
        state.failed |= !self.finished;
        self.scope.drained.notify_all();
    }
}
struct ExitGuard<J>(Arc<Shared<J>>);
impl<J> Drop for ExitGuard<J> {
    fn drop(&mut self) {
        let abandoned = {
            let mut queue = lock(&self.0.queue);
            queue.closed = true;
            self.0.close.0.store(true, Ordering::Release);
            (std::mem::take(&mut queue.jobs), queue.control.take())
        };
        // Dropping arbitrary jobs/result senders also stays outside the queue lock.
        for Queued { job, scope } in abandoned.0.into_iter().chain(abandoned.1) {
            let _completion = JobCompletion {
                scope,
                finished: false,
            };
            drop(job);
        }
        self.0.available.notify_all();
    }
}

#[cfg(test)]
mod tests;
