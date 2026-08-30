use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;

use crate::engine::CommandId;
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};
use crate::runtime::{
    RevokeRemoteControlControllerServiceError, SetRemoteControlControllerGrantServiceError,
};

pub(super) enum RemoteControlControllerGrantCommand {
    SetControllerGrant {
        id: CommandId,
        grant: RemoteControlControllerGrant,
    },
    RevokeController {
        id: CommandId,
        controller: RemoteControlControllerIdentity,
    },
}

pub(super) enum RemoteControlControllerGrantCompletion {
    ControllerGrantSet(
        Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantServiceError>,
    ),
    ControllerRevoked(
        Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerServiceError>,
    ),
}

enum RemoteControlControllerGrantExchangeState {
    Available,
    Submitted(RemoteControlControllerGrantCommand),
    Applying(CommandId),
    Settled {
        id: CommandId,
        completion: RemoteControlControllerGrantCompletion,
    },
    Completing(CommandId),
}

pub(super) struct RemoteControlControllerGrantExchange<M: RawMutex> {
    state: BlockingMutex<M, RefCell<RemoteControlControllerGrantExchangeState>>,
    command_ready: Signal<M, ()>,
    completion_ready: Signal<M, ()>,
}

impl RemoteControlControllerGrantCommand {
    const fn id(&self) -> CommandId {
        match self {
            Self::SetControllerGrant { id, .. } | Self::RevokeController { id, .. } => *id,
        }
    }
}

impl RemoteControlControllerGrantExchangeState {
    fn belongs_to(&self, id: CommandId) -> bool {
        match self {
            Self::Available => false,
            Self::Submitted(command) => command.id() == id,
            Self::Applying(applying)
            | Self::Settled { id: applying, .. }
            | Self::Completing(applying) => *applying == id,
        }
    }
}

impl<M: RawMutex> RemoteControlControllerGrantExchange<M> {
    pub(super) const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(
                RemoteControlControllerGrantExchangeState::Available,
            )),
            command_ready: Signal::new(),
            completion_ready: Signal::new(),
        }
    }

    pub(super) fn submit(&self, command: RemoteControlControllerGrantCommand) -> bool {
        let submitted = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlControllerGrantExchangeState::Available) {
                return false;
            }
            self.command_ready.reset();
            self.completion_ready.reset();
            *state = RemoteControlControllerGrantExchangeState::Submitted(command);
            true
        });
        if submitted {
            self.command_ready.signal(());
        }
        submitted
    }

    pub(super) async fn next_command(&self) -> RemoteControlControllerGrantCommand {
        loop {
            let command = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                let RemoteControlControllerGrantExchangeState::Submitted(command) = &*state else {
                    return None;
                };
                let id = command.id();
                match core::mem::replace(
                    &mut *state,
                    RemoteControlControllerGrantExchangeState::Applying(id),
                ) {
                    RemoteControlControllerGrantExchangeState::Submitted(command) => Some(command),
                    _ => unreachable!(),
                }
            });
            if let Some(command) = command {
                return command;
            }
            self.command_ready.wait().await;
        }
    }

    pub(super) fn settle(
        &self,
        id: CommandId,
        completion: RemoteControlControllerGrantCompletion,
    ) -> bool {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlControllerGrantExchangeState::Applying(applying) if applying == id)
            {
                return false;
            }
            *state = RemoteControlControllerGrantExchangeState::Settled { id, completion };
            true
        });
        if settled {
            self.completion_ready.signal(());
        }
        settled
    }

    pub(super) async fn completion(&self, id: CommandId) -> RemoteControlControllerGrantCompletion {
        loop {
            let completion = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if !matches!(&*state, RemoteControlControllerGrantExchangeState::Settled { id: settled, .. } if *settled == id)
                {
                    return None;
                }
                match core::mem::replace(
                    &mut *state,
                    RemoteControlControllerGrantExchangeState::Completing(id),
                ) {
                    RemoteControlControllerGrantExchangeState::Settled { completion, .. } => {
                        Some(completion)
                    }
                    _ => unreachable!(),
                }
            });
            if let Some(completion) = completion {
                return completion;
            }
            self.completion_ready.wait().await;
        }
    }

    pub(super) fn release(&self, id: CommandId) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.belongs_to(id) {
                *state = RemoteControlControllerGrantExchangeState::Available;
                self.completion_ready.reset();
            }
        });
    }
}
