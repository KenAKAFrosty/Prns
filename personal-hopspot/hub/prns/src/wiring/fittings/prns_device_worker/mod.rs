use super::{PrnsDeviceFitting, PrnsDeviceTransport, PrnsFittingError};
use crate::{PrnsDevice, PrnsDeviceIn, PrnsDeviceOut};
use core::{future::Future, num::NonZeroUsize};
use hopspot_hub_core::DeviceId;
use pipecircuit::{DuplexFitting, TransportTo};
use tokio::sync::{mpsc, oneshot};

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;
mod transport;

pub use transport::{PrnsDeviceWorkerIncoming, PrnsDeviceWorkerOutgoing};

pub struct PrnsDeviceWorker {
    commands: mpsc::Sender<Job>,
}

#[must_use]
pub enum PrnsDeviceSubmission {
    Submitted { completion: PrnsDeviceCompletion },
    Busy { input: PrnsDeviceIn },
    Stopped { input: PrnsDeviceIn },
}

#[derive(Debug)]
pub enum PrnsDeviceWorkerError {
    Fitting(PrnsFittingError),
    Stopped(oneshot::error::RecvError),
}

#[derive(Debug, PartialEq, Eq)]
pub struct PrnsDeviceQueueCapacityError {
    pub requested: NonZeroUsize,
    pub maximum: usize,
}

#[must_use]
pub struct PrnsDeviceCompletion {
    result: oneshot::Receiver<Result<PrnsDeviceOut, PrnsFittingError>>,
    accepted: oneshot::Sender<()>,
}

struct Job {
    input: PrnsDeviceIn,
    result: oneshot::Sender<Result<PrnsDeviceOut, PrnsFittingError>>,
    accepted: oneshot::Receiver<()>,
}

impl PrnsDeviceWorker {
    pub fn try_new(
        device: DeviceId,
        backend: impl PrnsDeviceTransport,
        queue_capacity: NonZeroUsize,
    ) -> Result<(Self, impl Future<Output = ()>), PrnsDeviceQueueCapacityError> {
        if queue_capacity.get() > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(PrnsDeviceQueueCapacityError {
                requested: queue_capacity,
                maximum: tokio::sync::Semaphore::MAX_PERMITS,
            });
        }
        Ok(Self::with_capacity(device, backend, queue_capacity))
    }
}

impl PrnsDeviceWorker {
    pub(crate) fn with_single_slot(
        device: DeviceId,
        backend: impl PrnsDeviceTransport,
    ) -> (Self, impl Future<Output = ()>) {
        Self::with_capacity(device, backend, NonZeroUsize::MIN)
    }

    fn with_capacity(
        device: DeviceId,
        backend: impl PrnsDeviceTransport,
        queue_capacity: NonZeroUsize,
    ) -> (Self, impl Future<Output = ()>) {
        let (commands, jobs) = mpsc::channel(queue_capacity.get());
        let fitting = PrnsDeviceFitting::new(device, backend);
        (Self { commands }, run(fitting, jobs))
    }

    pub(crate) fn execute(
        &self,
        input: PrnsDeviceIn,
    ) -> impl Future<Output = Result<PrnsDeviceOut, PrnsDeviceWorkerError>> + Send + 'static {
        let commands = self.commands.clone();
        let (job, completion) = Job::new(input);
        async move {
            drop(commands.send(job).await);
            drop(commands);
            completion.complete().await
        }
    }
}

impl Job {
    fn new(input: PrnsDeviceIn) -> (Self, PrnsDeviceCompletion) {
        let (result, completed) = oneshot::channel();
        let (accepted, acceptance) = oneshot::channel();
        (
            Self {
                input,
                result,
                accepted: acceptance,
            },
            PrnsDeviceCompletion {
                result: completed,
                accepted,
            },
        )
    }
}

impl PrnsDeviceCompletion {
    pub async fn complete(self) -> Result<PrnsDeviceOut, PrnsDeviceWorkerError> {
        let result = self.result.await.map_err(PrnsDeviceWorkerError::Stopped)?;
        let _accepted = self.accepted.send(());
        result.map_err(PrnsDeviceWorkerError::Fitting)
    }
}

async fn run(
    mut fitting: PrnsDeviceFitting<impl PrnsDeviceTransport>,
    mut jobs: mpsc::Receiver<Job>,
) {
    while let Some(Job {
        input,
        mut result,
        accepted,
    }) = jobs.recv().await
    {
        if result.is_closed() && !matches!(input, PrnsDeviceIn::Close { .. }) {
            continue;
        }
        let cancellable = matches!(input, PrnsDeviceIn::Inventory { .. });
        let completion = {
            let (_, mut outgoing) = <_ as DuplexFitting<PrnsDevice>>::split(&mut fitting);
            let work = outgoing.send(input);
            tokio::select! {
                biased;
                () = result.closed(), if cancellable => continue,
                completion = work.complete() => completion,
            }
        };
        let newly_connected = matches!(completion, Ok(PrnsDeviceOut::Connected { .. }));
        let _delivered = result.send(completion);
        if newly_connected && accepted.await.is_err() {
            fitting.release_unaccepted_connection();
        }
    }
}
