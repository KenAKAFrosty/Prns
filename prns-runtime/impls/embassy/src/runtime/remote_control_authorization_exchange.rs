use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;

use crate::engine::CommandId;

pub(super) trait RemoteControlAuthorizationCommand {
    fn id(&self) -> CommandId;
}

enum RemoteControlAuthorizationExchangeState<Command, Completion> {
    Available,
    Submitted(Command),
    Applying(CommandId),
    Settled {
        id: CommandId,
        completion: Completion,
    },
    Completing(CommandId),
}

impl<Command, Completion> RemoteControlAuthorizationExchangeState<Command, Completion>
where
    Command: RemoteControlAuthorizationCommand,
{
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

pub(super) struct RemoteControlAuthorizationExchange<M, Command, Completion>
where
    M: RawMutex,
    Command: RemoteControlAuthorizationCommand,
{
    state: BlockingMutex<M, RefCell<RemoteControlAuthorizationExchangeState<Command, Completion>>>,
    command_ready: Signal<M, ()>,
    completion_ready: Signal<M, ()>,
}

impl<M, Command, Completion> RemoteControlAuthorizationExchange<M, Command, Completion>
where
    M: RawMutex,
    Command: RemoteControlAuthorizationCommand,
{
    pub(super) const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(
                RemoteControlAuthorizationExchangeState::Available,
            )),
            command_ready: Signal::new(),
            completion_ready: Signal::new(),
        }
    }

    pub(super) fn submit(&self, command: Command) -> bool {
        let submitted = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlAuthorizationExchangeState::Available) {
                return false;
            }
            self.command_ready.reset();
            self.completion_ready.reset();
            *state = RemoteControlAuthorizationExchangeState::Submitted(command);
            true
        });
        if submitted {
            self.command_ready.signal(());
        }
        submitted
    }

    pub(super) async fn next_command(&self) -> Command {
        loop {
            let command = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                let RemoteControlAuthorizationExchangeState::Submitted(command) = &*state else {
                    return None;
                };
                let id = command.id();
                match core::mem::replace(
                    &mut *state,
                    RemoteControlAuthorizationExchangeState::Applying(id),
                ) {
                    RemoteControlAuthorizationExchangeState::Submitted(command) => Some(command),
                    RemoteControlAuthorizationExchangeState::Available
                    | RemoteControlAuthorizationExchangeState::Applying(_)
                    | RemoteControlAuthorizationExchangeState::Settled { .. }
                    | RemoteControlAuthorizationExchangeState::Completing(_) => unreachable!(),
                }
            });
            if let Some(command) = command {
                return command;
            }
            self.command_ready.wait().await;
        }
    }

    pub(super) fn settle(&self, id: CommandId, completion: Completion) -> bool {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlAuthorizationExchangeState::Applying(applying) if applying == id)
            {
                return false;
            }
            *state = RemoteControlAuthorizationExchangeState::Settled { id, completion };
            true
        });
        if settled {
            self.completion_ready.signal(());
        }
        settled
    }

    pub(super) async fn completion(&self, id: CommandId) -> Completion {
        loop {
            let completion = self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if !matches!(&*state, RemoteControlAuthorizationExchangeState::Settled { id: settled, .. } if *settled == id)
                {
                    return None;
                }
                match core::mem::replace(
                    &mut *state,
                    RemoteControlAuthorizationExchangeState::Completing(id),
                ) {
                    RemoteControlAuthorizationExchangeState::Settled { completion, .. } => {
                        Some(completion)
                    }
                    RemoteControlAuthorizationExchangeState::Available
                    | RemoteControlAuthorizationExchangeState::Submitted(_)
                    | RemoteControlAuthorizationExchangeState::Applying(_)
                    | RemoteControlAuthorizationExchangeState::Completing(_) => unreachable!(),
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
                *state = RemoteControlAuthorizationExchangeState::Available;
                self.completion_ready.reset();
            }
        });
    }
}
