use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;

use crate::engine::CommandId;
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerError, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
};

pub(super) enum RemoteControlAccessCommand {
    SetControllerGrant {
        id: CommandId,
        grant: RemoteControlControllerGrant,
    },
    RevokeController {
        id: CommandId,
        controller: RemoteControlControllerIdentity,
    },
}

pub(super) enum RemoteControlAccessCompletion {
    ControllerGrantSet(
        Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError>,
    ),
    ControllerRevoked(
        Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerError>,
    ),
}

enum RemoteControlAccessExchangeState {
    Available,
    Submitted(RemoteControlAccessCommand),
    Applying(CommandId),
    Settled {
        id: CommandId,
        completion: RemoteControlAccessCompletion,
    },
    Completing(CommandId),
}

pub(super) struct RemoteControlAccessExchange<M: RawMutex> {
    state: BlockingMutex<M, RefCell<RemoteControlAccessExchangeState>>,
    command_ready: Signal<M, ()>,
    completion_ready: Signal<M, ()>,
}

impl RemoteControlAccessCommand {
    const fn id(&self) -> CommandId {
        match self {
            Self::SetControllerGrant { id, .. } | Self::RevokeController { id, .. } => *id,
        }
    }
}

impl RemoteControlAccessExchangeState {
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

impl<M: RawMutex> RemoteControlAccessExchange<M> {
    pub(super) const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(RemoteControlAccessExchangeState::Available)),
            command_ready: Signal::new(),
            completion_ready: Signal::new(),
        }
    }

    pub(super) fn submit(&self, command: RemoteControlAccessCommand) -> bool {
        let submitted = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlAccessExchangeState::Available) {
                return false;
            }
            self.command_ready.reset();
            self.completion_ready.reset();
            *state = RemoteControlAccessExchangeState::Submitted(command);
            true
        });
        if submitted {
            self.command_ready.signal(());
        }
        submitted
    }

    pub(super) async fn next_command(&self) -> RemoteControlAccessCommand {
        loop {
            let command = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                let RemoteControlAccessExchangeState::Submitted(command) = &*state else {
                    return None;
                };
                let id = command.id();
                match core::mem::replace(
                    &mut *state,
                    RemoteControlAccessExchangeState::Applying(id),
                ) {
                    RemoteControlAccessExchangeState::Submitted(command) => Some(command),
                    _ => unreachable!(),
                }
            });
            if let Some(command) = command {
                return command;
            }
            self.command_ready.wait().await;
        }
    }

    pub(super) fn settle(&self, id: CommandId, completion: RemoteControlAccessCompletion) -> bool {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlAccessExchangeState::Applying(applying) if applying == id)
            {
                return false;
            }
            *state = RemoteControlAccessExchangeState::Settled { id, completion };
            true
        });
        if settled {
            self.completion_ready.signal(());
        }
        settled
    }

    pub(super) async fn completion(&self, id: CommandId) -> RemoteControlAccessCompletion {
        loop {
            let completion = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if !matches!(&*state, RemoteControlAccessExchangeState::Settled { id: settled, .. } if *settled == id)
                {
                    return None;
                }
                match core::mem::replace(
                    &mut *state,
                    RemoteControlAccessExchangeState::Completing(id),
                ) {
                    RemoteControlAccessExchangeState::Settled { completion, .. } => {
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
                *state = RemoteControlAccessExchangeState::Available;
                self.completion_ready.reset();
            }
        });
    }
}
