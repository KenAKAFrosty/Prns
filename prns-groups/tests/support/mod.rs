#![allow(dead_code, clippy::panic)]
use prns_groups::{Client, Clock, Error, Job, StoreSession, Submit};
use prns_groups_service::{
    ClockInterval, ClockReading, Endpoint, EndpointId, EndpointIncarnation, Timestamp,
};
use std::{
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
};

#[cfg(feature = "network")]
#[path = "../../transport/reticulum/tests/support/wire.rs"]
pub mod wire;

pub const NOW: u64 = 1_792_000_000;
pub fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
pub fn clock() -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap())
}
pub struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> ClockReading {
        clock()
    }
}

enum Work {
    Job(Box<Job>),
    Stop,
}
struct Lane {
    budget: prns_groups::InputBudget,
    sender: mpsc::SyncSender<Work>,
    busy_admissions: Arc<std::sync::atomic::AtomicUsize>,
}
impl Submit for Lane {
    fn input_budget(&self) -> &prns_groups::InputBudget {
        &self.budget
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        if {
            let mut previous = self
                .busy_admissions
                .load(std::sync::atomic::Ordering::SeqCst);
            loop {
                let Some(next) = previous.checked_sub(1) else {
                    break Err(previous);
                };
                match self.busy_admissions.compare_exchange_weak(
                    previous,
                    next,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                ) {
                    Ok(previous) => break Ok(previous),
                    Err(current) => previous = current,
                }
            }
        }
        .is_ok()
        {
            return Err(Error::Busy);
        }
        self.sender
            .try_send(Work::Job(Box::new(job)))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => Error::Busy,
                mpsc::TrySendError::Disconnected(_) => Error::Unavailable,
            })
    }
}
pub struct Worker {
    pub client: Client,
    pub busy_admissions: Arc<std::sync::atomic::AtomicUsize>,
    sender: mpsc::SyncSender<Work>,
    thread: thread::JoinHandle<()>,
}
impl Worker {
    pub fn open(path: PathBuf, fixture: Vec<u8>, identity: prns_host::IdentityHash) -> Self {
        let (sender, receiver) = mpsc::sync_channel(8);
        let busy_admissions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client = Client::new(Arc::new(Lane {
            budget: Default::default(),
            sender: sender.clone(),
            busy_admissions: busy_admissions.clone(),
        }));
        let (ready, readiness) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let db = redb::Database::create(path).unwrap();
            let cfg = prns_groups::fixture::load_and_install(&db, &fixture, identity).unwrap();
            let mut session = StoreSession::open(&db, cfg).unwrap();
            ready.send(()).unwrap();
            while let Ok(work) = receiver.recv() {
                match work {
                    Work::Job(job) => job.execute(&mut session),
                    Work::Stop => break,
                }
            }
        });
        readiness.recv().unwrap();
        Self {
            client,
            busy_admissions,
            sender,
            thread,
        }
    }
    pub fn stop(self) {
        self.sender.send(Work::Stop).unwrap();
        self.thread.join().unwrap();
    }
}

/// Test inputs retain canonical immutable requests separately from fresh clocks.
pub type Prepared = (prns_groups::Request, prns_groups::ExecutionContext);
pub fn text_request(
    group: prns_groups_core::GroupId,
    scope: prns_groups::OriginalScope,
    id: [u8; 32],
    text: String,
    clock: ClockReading,
) -> Prepared {
    use prns_groups::{text_publication, ExecutionContext, Intent, Request, RequestId};
    (
        Request {
            id: RequestId(id),
            intent: Intent::Publish(text_publication(group, text).unwrap()),
            original_scope: Some(scope),
        },
        ExecutionContext { clock },
    )
}
pub fn publication_ref(request: &Prepared) -> &prns_groups::PublicationIntent {
    let prns_groups::Intent::Publish(intent) = &request.0.intent else {
        panic!("publication expected")
    };
    intent
}
pub fn publication(request: &mut Prepared) -> &mut prns_groups::PublicationIntent {
    let prns_groups::Intent::Publish(intent) = &mut request.0.intent else {
        panic!("publication expected")
    };
    intent
}
pub fn explicit_targets(request: &mut Prepared) -> &mut Vec<Endpoint> {
    let prns_groups::Targets::Explicit(targets) = &mut publication(request).targets else {
        panic!("explicit targets expected")
    };
    targets
}
pub trait PublicationClient {
    fn admit_request(
        &self,
        request: Prepared,
    ) -> Result<
        impl std::future::Future<
                Output = Result<prns_groups_core::RecordId, prns_groups_core::ApiError>,
            > + Send
            + 'static,
        prns_groups_core::ApiError,
    >;
    fn publish_request(
        &self,
        request: Prepared,
    ) -> impl std::future::Future<
        Output = Result<prns_groups_core::RecordId, prns_groups_core::ApiError>,
    > + Send;
}
impl PublicationClient for Client {
    fn admit_request(
        &self,
        (request, context): Prepared,
    ) -> Result<
        impl std::future::Future<
                Output = Result<prns_groups_core::RecordId, prns_groups_core::ApiError>,
            > + Send
            + 'static,
        prns_groups_core::ApiError,
    > {
        let outcome = self.admit_execute(request, context)?;
        Ok(async move {
            match outcome.await? {
                prns_groups::Outcome::Published(id) => Ok(id),
                _ => Err(prns_groups_core::ApiError::Corrupt),
            }
        })
    }
    async fn publish_request(
        &self,
        request: Prepared,
    ) -> Result<prns_groups_core::RecordId, prns_groups_core::ApiError> {
        self.admit_request(request)?.await
    }
}
