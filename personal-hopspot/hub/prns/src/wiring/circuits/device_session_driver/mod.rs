use crate::{
    DeviceSessionEvent, DeviceSessionInput, DeviceSessionMessage, DeviceSessionRoute,
    DeviceSessionRoutingError, DeviceSessionSnapshot, DeviceSessionSwitchboard, MioSessionPoll,
    PrnsDeviceIn, PrnsDeviceWorker, PrnsInventoryTransport,
};
use alloc::{boxed::Box, sync::Arc};
use core::{future::Future, sync::atomic::AtomicBool};
use hopspot_hub_core::{DeviceId, DeviceRegistry};
use pipecircuit::{
    Circuit, CircuitReactionOutcome, ConductionFailure, Pipecircuit, Reactor, Switchboard,
};
use std::sync::mpsc::sync_channel;
use tokio::task::JoinError;

#[cfg(test)]
mod behavior;
mod dispatch;
mod handle;
mod reactor;
#[cfg(test)]
mod tests;

pub use handle::{DeviceSessionHandle, DeviceSessionSubmission};
pub use reactor::{DeviceDriverReaction, DeviceDriverReactor};

const INTENT_CAPACITY: usize = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceSessionIntent {
    Connect,
    Refresh,
    Disconnect,
    Inspect,
}

impl DeviceSessionIntent {
    fn into_message(self) -> DeviceSessionMessage {
        match self {
            Self::Connect => DeviceSessionMessage::Connect,
            Self::Refresh => DeviceSessionMessage::Refresh,
            Self::Disconnect => DeviceSessionMessage::Disconnect,
            Self::Inspect => DeviceSessionMessage::Inspect,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceSessionUpdate<const CAPACITY: usize> {
    pub event: DeviceSessionEvent,
    pub snapshot: DeviceSessionSnapshot<CAPACITY>,
}

#[derive(Debug)]
pub enum DeviceDriverFailure {
    Routing(DeviceSessionRoutingError),
    Worker(crate::PrnsDeviceWorkerError),
    CommandCapacity { rejected: PrnsDeviceIn },
}

pub struct DeviceSessionExit {
    pub registry: DeviceRegistry,
    pub settlement: Result<(), ConductionFailure<std::io::Error, DeviceDriverFailure>>,
}

pub struct DeviceSessionRuntime<Run> {
    pub handle: DeviceSessionHandle,
    pub run: Run,
}

#[expect(clippy::large_enum_variant)]
pub enum DeviceDriverTurn<const CAPACITY: usize> {
    Updated(DeviceSessionUpdate<CAPACITY>),
    Stopped,
}

pub struct DeviceSessionDriver<Physical = MioSessionPoll> {
    registry: DeviceRegistry,
    worker: PrnsDeviceWorker,
    reactor: DeviceDriverReactor<Physical>,
    queued: [Option<PrnsDeviceIn>; 2],
}

pub fn prepare_device_session<const CAPACITY: usize>(
    registry: DeviceRegistry,
    device: DeviceId,
    backend: impl PrnsInventoryTransport,
    mut on_update: impl FnMut(DeviceSessionUpdate<CAPACITY>) + Send + 'static,
) -> Result<
    DeviceSessionRuntime<impl Future<Output = Result<DeviceSessionExit, JoinError>>>,
    std::io::Error,
> {
    MioSessionPoll::try_open().map(|(physical, wake)| {
        let signal = Arc::new(handle::SessionSignal {
            stopping: AtomicBool::new(false),
            wake,
        });
        let (intents, inbox) = sync_channel(INTENT_CAPACITY);
        let handle = DeviceSessionHandle {
            intents,
            signal: handle::WakeOnDrop(Arc::clone(&signal)),
        };
        let stop = handle::StopOnDrop(Arc::clone(&signal));
        let (worker, work) = PrnsDeviceWorker::with_single_slot(device, backend);
        let driver = DeviceSessionDriver {
            registry,
            worker,
            reactor: DeviceDriverReactor {
                physical,
                inbox,
                signal,
                pending: None,
                active: None,
                completed: None,
                stopping: false,
                admit_intents: true,
            },
            queued: [None, None],
        };
        DeviceSessionRuntime {
            handle,
            run: async move {
                let stop = stop;
                let conductor = tokio::task::spawn_blocking(move || {
                    let mut conductor =
                        Pipecircuit::new(DeviceSessionSwitchboard::<CAPACITY>::new(device), driver);
                    let settlement = loop {
                        match conductor.conduct() {
                            Ok(DeviceDriverTurn::Updated(update)) => on_update(update),
                            Ok(DeviceDriverTurn::Stopped) => break Ok(()),
                            Err(failure) => break Err(failure),
                        }
                    };
                    let (_, driver) = conductor.into_parts();
                    DeviceSessionExit {
                        registry: driver.registry,
                        settlement,
                    }
                });
                let ((), result) = tokio::join!(work, conductor);
                drop(stop);
                result
            },
        }
    })
}

impl<const CAPACITY: usize, Physical: Reactor<Failure = std::io::Error>>
    Circuit<DeviceSessionSwitchboard<CAPACITY>> for DeviceSessionDriver<Physical>
{
    type Reactor = DeviceDriverReactor<Physical>;
    type Completion = DeviceDriverTurn<CAPACITY>;
    type Failure = DeviceDriverFailure;

    fn reactor(&mut self) -> &mut Self::Reactor {
        &mut self.reactor
    }

    fn react(
        &mut self,
        board: &mut DeviceSessionSwitchboard<CAPACITY>,
        reaction: &DeviceDriverReaction,
    ) -> Result<CircuitReactionOutcome<Self::Completion>, Self::Failure> {
        let message = match reaction {
            DeviceDriverReaction::IntentReady => self
                .reactor
                .pending
                .take()
                .map(DeviceSessionIntent::into_message),
            DeviceDriverReaction::WorkReady => self
                .reactor
                .completed
                .take()
                .transpose()
                .map_err(DeviceDriverFailure::Worker)?
                .map(DeviceSessionMessage::Prns),
            DeviceDriverReaction::Drained => {
                return Ok(CircuitReactionOutcome::Completed {
                    completion: DeviceDriverTurn::Stopped,
                });
            }
            DeviceDriverReaction::Shutdown => {
                self.queued = [None, None];
                self.reactor.pending = None;
                Some(DeviceSessionMessage::Disconnect)
            }
        };
        let Some(message) = message else {
            return Ok(CircuitReactionOutcome::AwaitingReaction);
        };
        let route = board
            .route(DeviceSessionInput {
                registry: &mut self.registry,
                message,
            })
            .map_err(DeviceDriverFailure::Routing)?;
        self.dispatch(&route)?;
        Ok(CircuitReactionOutcome::Completed {
            completion: DeviceDriverTurn::Updated(DeviceSessionUpdate {
                event: route.event,
                snapshot: route.snapshot,
            }),
        })
    }
}
