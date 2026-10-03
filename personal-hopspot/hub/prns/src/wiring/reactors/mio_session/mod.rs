use crate::DeviceSessionMessage;
use alloc::sync::Arc;
use mio::{Events, Poll, Token, Waker};
use pipecircuit::Reactor;
use std::io::ErrorKind;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

#[cfg(test)]
mod behavior;
mod mailbox;
#[cfg(test)]
mod tests;

pub use mailbox::{MioSessionSender, MioSessionSubmission, MioSessionWakeFailure};

const MAILBOX_CAPACITY: usize = 1;
const MAILBOX_TOKEN: Token = Token(0);

#[derive(Debug, PartialEq, Eq)]
pub enum MioSessionReaction {
    MessageReady,
    SendersClosed,
}

pub struct MioSessionReactor<Physical = MioSessionPoll> {
    physical: Physical,
    inbox: Receiver<DeviceSessionMessage>,
    pending: Option<DeviceSessionMessage>,
}

pub struct MioSessionPoll {
    poll: Poll,
    events: Events,
}

impl Reactor for MioSessionPoll {
    type Reaction = ();
    type Failure = std::io::Error;

    fn wait_for_reaction(&mut self) -> Result<(), Self::Failure> {
        self.poll.poll(&mut self.events, None)
    }
}

impl MioSessionReactor {
    pub fn try_open() -> Result<(Self, MioSessionSender), std::io::Error> {
        Poll::new().and_then(|poll| {
            Waker::new(poll.registry(), MAILBOX_TOKEN).map(|waker| {
                let (sender, inbox) = sync_channel(MAILBOX_CAPACITY);
                (
                    Self {
                        physical: MioSessionPoll {
                            poll,
                            events: Events::with_capacity(MAILBOX_CAPACITY),
                        },
                        inbox,
                        pending: None,
                    },
                    MioSessionSender::new(sender, Arc::new(waker)),
                )
            })
        })
    }
}

impl<Physical> MioSessionReactor<Physical> {
    pub(crate) fn take_message(&mut self) -> Option<DeviceSessionMessage> {
        self.pending.take()
    }
}

impl<Physical: Reactor<Failure = std::io::Error>> Reactor for MioSessionReactor<Physical> {
    type Reaction = MioSessionReaction;
    type Failure = Physical::Failure;

    fn wait_for_reaction(&mut self) -> Result<Self::Reaction, Self::Failure> {
        if self.pending.is_some() {
            return Ok(MioSessionReaction::MessageReady);
        }
        loop {
            match self.inbox.try_recv() {
                Ok(message) => {
                    self.pending = Some(message);
                    return Ok(MioSessionReaction::MessageReady);
                }
                Err(TryRecvError::Disconnected) => return Ok(MioSessionReaction::SendersClosed),
                Err(TryRecvError::Empty) => match self.physical.wait_for_reaction() {
                    Ok(_) => {}
                    Err(source) if source.kind() == ErrorKind::Interrupted => {}
                    Err(source) => return Err(source),
                },
            }
        }
    }
}
