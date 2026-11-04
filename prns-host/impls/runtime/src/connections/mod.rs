//! Persistent connection intent and one independently drainable host binding.
//!
//! Saved profiles are installation-wide and editable without a running host.
//! Accepted saves outlive their waiters. Save success confirms durable intent;
//! `applied_revision` separately reports actual attachment reconciliation. A
//! binding owns only its claimed interfaces, never the supplied host itself.
mod binding;
mod model;
mod runtime;
mod store;

pub use binding::{
    AttachmentState, BindingClient, BindingPhase, BindingSnapshot, BindingStatus, HostBinding,
    LiveProfile,
};
pub use model::{
    tcp_client_config, tcp_client_config_from_target, Error, Guard, InputError, Limits, Options,
    Profile, ProfileId, SchemaError, Snapshot,
};

use crate::owner::OwnedSession;
use prns_storage::{Completion, Inbox, OwnerEvent, Worker};
use std::{
    path::Path,
    sync::{Arc, Mutex, Weak},
    task::Waker,
};
use tokio::sync::oneshot;

fn reply<T>(sender: oneshot::Sender<T>, value: T) {
    // A foreign executor's waker is outside this owner's trust boundary.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sender.send(value)));
}
fn admission(error: prns_storage::SubmitError<Job>) -> Error {
    match error {
        prns_storage::SubmitError::Full(_) => Error::Busy,
        prns_storage::SubmitError::Closed(_) => Error::Closed,
        prns_storage::SubmitError::Reentrant(_) => Error::Reentrant,
    }
}
#[allow(clippy::large_enum_variant)]
enum Job {
    Snapshot(oneshot::Sender<Result<Snapshot, Error>>),
    Replace {
        expected: Guard,
        profiles: Vec<Profile>,
        reply: oneshot::Sender<Result<Snapshot, Error>>,
    },
    Bind {
        control: Arc<binding::Control>,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    BindingSnapshot {
        generation: u64,
        reply: oneshot::Sender<Result<BindingSnapshot, Error>>,
    },
    Retry {
        generation: u64,
        expected: Guard,
        profile: ProfileId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    #[cfg(test)]
    Fault(store::Fault, oneshot::Sender<()>),
    #[cfg(test)]
    Barrier(
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    ),
}

/// Ordinary borrowed storage client. Dropping it neither stops networking nor
/// closes the physical store. Async methods admit on their first poll.
#[derive(Clone)]
pub struct Client {
    lane: prns_storage::Client<Job>,
    limits: Limits,
}
impl Client {
    pub async fn snapshot(&self) -> Result<Snapshot, Error> {
        let (send, receive) = oneshot::channel();
        self.lane
            .try_submit(Job::Snapshot(send))
            .map_err(admission)?;
        receive.await.map_err(|_| Error::WorkerFailed)?
    }
    pub async fn replace(
        &self,
        expected: Guard,
        profiles: Vec<Profile>,
    ) -> Result<Snapshot, Error> {
        let profiles = store::checked(profiles, self.limits)?;
        let (send, receive) = oneshot::channel();
        self.lane
            .try_submit(Job::Replace {
                expected,
                profiles,
                reply: send,
            })
            .map_err(admission)?;
        receive.await.map_err(|_| Error::WorkerFailed)?
    }
}

/// Owns one private profile database and bounded worker, independent of any
/// host generation. No preset is inserted by `open_existing` or by a read.
pub struct ProfileOwner {
    worker: Worker<Job, Error>,
    limits: Limits,
    progress: Waker,
    binding: Mutex<Option<Weak<binding::Control>>>,
}
impl ProfileOwner {
    pub fn create_new(
        path: impl AsRef<Path>,
        profiles: Vec<Profile>,
        options: Options,
    ) -> Result<Self, Error> {
        let options = options.validate()?;
        let profiles = store::checked(profiles, options.limits)?;
        Self::start(path.as_ref(), Some(profiles), options)
    }
    pub fn open_existing(path: impl AsRef<Path>, options: Options) -> Result<Self, Error> {
        Self::start(path.as_ref(), None, options.validate()?)
    }
    fn start(path: &Path, initial: Option<Vec<Profile>>, options: Options) -> Result<Self, Error> {
        let path =
            prns_storage::database::private_path(path).map_err(|e| Error::File(Arc::new(e)))?;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker = Worker::start(
            "prns-connection-profiles",
            options.queue_capacity,
            move |inbox: Inbox<Job>| {
                let store = store::Store::open(&path, initial, options)?;
                let saved = store.read()?;
                let progress = inbox.progress_waker();
                let _ = send.send(progress.clone());
                let mut state = runtime::State::new(store, saved, progress);
                inbox.run_owned(|event, context| {
                    match event {
                        OwnerEvent::Job(job) => state.job(job),
                        OwnerEvent::Closing => state.close(),
                        OwnerEvent::Progress => (),
                    }
                    state.poll(context)
                });
                Ok(())
            },
        )
        .map_err(|e| match e {
            prns_storage::StartError::Initialization(e) => e,
            prns_storage::StartError::Spawn(e) => Error::Spawn(Arc::new(e)),
            _ => Error::WorkerFailed,
        })?;
        let progress = receive.recv().map_err(|_| Error::WorkerFailed)?;
        Ok(Self {
            worker,
            limits: options.limits,
            progress,
            binding: Mutex::new(None),
        })
    }
    pub fn client(&self) -> Client {
        Client {
            lane: self.worker.client(),
            limits: self.limits,
        }
    }

    /// Bind one running host. Cleanup uses the owner's unrestricted attachment
    /// capability; a revoked public HostClient cannot strand this binding.
    /// Cancelling this constructor closes the accepted binding, never leaks it.
    pub async fn bind(&self, host: &OwnedSession) -> Result<HostBinding, Error> {
        let scope = Arc::new(host.attachment_scope().map_err(Error::Host)?);
        let control = Arc::new(binding::Control::new(scope, self.progress.clone())?);
        let binding = HostBinding::new(control.clone(), self.worker.client());
        {
            let mut current = crate::lock(&self.binding);
            if current
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|c| !c.is_finished())
            {
                return Err(Error::AlreadyBound);
            }
            *current = Some(Arc::downgrade(&control));
        }
        let (send, receive) = oneshot::channel();
        self.worker
            .client()
            .try_submit(Job::Bind {
                control,
                reply: send,
            })
            .map_err(admission)?;
        receive.await.map_err(|_| Error::WorkerFailed)??;
        Ok(binding)
    }
    /// Synchronously fence storage admission and the current attachment claim.
    /// The returned completion retains real cleanup, independent of its waiters.
    pub fn close(&self) -> Completion<Error> {
        self.worker.request_close();
        let binding = crate::lock(&self.binding).as_ref().and_then(Weak::upgrade);
        if let Some(binding) = binding {
            binding.request_close();
        }
        self.worker.closed()
    }
    pub fn closed(&self) -> Completion<Error> {
        self.worker.closed()
    }
}
impl Drop for ProfileOwner {
    fn drop(&mut self) {
        drop(self.close());
    }
}

#[cfg(test)]
mod tests;
