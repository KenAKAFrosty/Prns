use super::*;
use alloc::{boxed::Box, sync::Arc};
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::Ordering,
    task::{Context, Poll, Waker},
};
use std::{
    io::ErrorKind,
    sync::mpsc::{Receiver, TryRecvError},
};

pub(super) struct ActiveWork {
    pub(super) input: PrnsDeviceIn,
    pub(super) completion: Pin<
        Box<dyn Future<Output = Result<crate::PrnsDeviceOut, crate::PrnsDeviceWorkerError>> + Send>,
    >,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceDriverReaction {
    IntentReady,
    WorkReady,
    Shutdown,
    Drained,
}

pub struct DeviceDriverReactor<Physical = MioSessionPoll> {
    pub(super) physical: Physical,
    pub(super) inbox: Receiver<DeviceSessionIntent>,
    pub(super) signal: Arc<handle::SessionSignal>,
    pub(super) pending: Option<DeviceSessionIntent>,
    pub(super) active: Option<ActiveWork>,
    pub(super) completed: Option<Result<crate::PrnsDeviceOut, crate::PrnsDeviceWorkerError>>,
    pub(super) stopping: bool,
    pub(super) admit_intents: bool,
}

impl<Physical: Reactor<Failure = std::io::Error>> Reactor for DeviceDriverReactor<Physical> {
    type Reaction = DeviceDriverReaction;
    type Failure = std::io::Error;

    fn wait_for_reaction(&mut self) -> Result<Self::Reaction, Self::Failure> {
        let waker = Waker::from(Arc::clone(&self.signal));
        let mut context = Context::from_waker(&waker);
        loop {
            if !self.stopping && self.signal.stopping.load(Ordering::Acquire) {
                self.stopping = true;
                return Ok(DeviceDriverReaction::Shutdown);
            }
            if self.pending.is_some() {
                return Ok(DeviceDriverReaction::IntentReady);
            }
            if self.completed.is_some() {
                return Ok(DeviceDriverReaction::WorkReady);
            }
            if let Some(active) = &mut self.active
                && let Poll::Ready(result) = active.completion.as_mut().poll(&mut context)
            {
                self.active = None;
                self.completed = Some(result);
                return Ok(DeviceDriverReaction::WorkReady);
            }
            if !self.stopping && self.admit_intents {
                match self.inbox.try_recv() {
                    Ok(intent) => {
                        self.pending = Some(intent);
                        return Ok(DeviceDriverReaction::IntentReady);
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.signal.stopping.store(true, Ordering::Release);
                        self.stopping = true;
                        return Ok(DeviceDriverReaction::Shutdown);
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            if self.stopping && self.active.is_none() {
                return Ok(DeviceDriverReaction::Drained);
            }
            match self.physical.wait_for_reaction() {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}
