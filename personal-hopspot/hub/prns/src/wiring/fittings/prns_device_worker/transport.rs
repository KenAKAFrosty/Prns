use super::*;
use pipecircuit::{ReceiveFromOutcome, TransportFrom};

pub struct PrnsDeviceWorkerIncoming;

pub struct PrnsDeviceWorkerOutgoing<'fitting> {
    commands: &'fitting mpsc::Sender<Job>,
}

impl DuplexFitting<PrnsDevice> for PrnsDeviceWorker {
    type Incoming<'fitting> = PrnsDeviceWorkerIncoming;
    type Outgoing<'fitting> = PrnsDeviceWorkerOutgoing<'fitting>;

    fn split(&mut self) -> (Self::Incoming<'_>, Self::Outgoing<'_>) {
        (
            PrnsDeviceWorkerIncoming,
            PrnsDeviceWorkerOutgoing {
                commands: &self.commands,
            },
        )
    }
}

impl TransportTo<PrnsDevice> for PrnsDeviceWorkerOutgoing<'_> {
    type Outcome<'transport, 'message>
        = PrnsDeviceSubmission
    where
        Self: 'transport;

    fn send<'transport, 'message>(
        &'transport mut self,
        input: PrnsDeviceIn,
    ) -> Self::Outcome<'transport, 'message> {
        let (result, completed) = oneshot::channel();
        let (accepted, acceptance) = oneshot::channel();
        match self.commands.try_send(Job {
            input,
            result,
            accepted: acceptance,
        }) {
            Ok(()) => PrnsDeviceSubmission::Submitted {
                completion: PrnsDeviceCompletion {
                    result: completed,
                    accepted,
                },
            },
            Err(mpsc::error::TrySendError::Full(job)) => {
                PrnsDeviceSubmission::Busy { input: job.input }
            }
            Err(mpsc::error::TrySendError::Closed(job)) => {
                PrnsDeviceSubmission::Stopped { input: job.input }
            }
        }
    }
}

impl TransportFrom<PrnsDevice> for PrnsDeviceWorkerIncoming {
    type IncomingMessage<'message> = Result<PrnsDeviceOut, PrnsDeviceWorkerError>;
    type Failure = PrnsDeviceWorkerError;

    fn receive_from(
        &mut self,
        completion: Self::IncomingMessage<'_>,
    ) -> ReceiveFromOutcome<PrnsDeviceOut, Self::IncomingMessage<'_>, Self::Failure> {
        match completion {
            Ok(output) => ReceiveFromOutcome::Received { output },
            Err(failure) => ReceiveFromOutcome::Failed { failure },
        }
    }
}
