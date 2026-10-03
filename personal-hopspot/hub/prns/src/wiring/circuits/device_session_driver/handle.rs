use super::*;
use alloc::sync::Arc;
use alloc::task::Wake;
use core::sync::atomic::{AtomicBool, Ordering};
use mio::Waker;
use std::sync::mpsc::{SyncSender, TrySendError};

pub(super) struct SessionSignal {
    pub(super) stopping: AtomicBool,
    pub(super) wake: Arc<Waker>,
}

impl Wake for SessionSignal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let _woken = self.wake.wake();
    }
}

#[derive(Clone)]
pub(super) struct WakeOnDrop(pub(super) Arc<SessionSignal>);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.wake_by_ref();
    }
}

#[derive(Clone)]
pub struct DeviceSessionHandle {
    pub(super) intents: SyncSender<DeviceSessionIntent>,
    pub(super) signal: WakeOnDrop,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum DeviceSessionSubmission {
    Submitted,
    Busy { intent: DeviceSessionIntent },
    Stopped { intent: DeviceSessionIntent },
}

impl DeviceSessionHandle {
    pub fn submit(
        &self,
        intent: DeviceSessionIntent,
    ) -> Result<DeviceSessionSubmission, std::io::Error> {
        if self.signal.0.stopping.load(Ordering::Acquire) {
            return Ok(DeviceSessionSubmission::Stopped { intent });
        }
        match self.intents.try_send(intent) {
            Ok(()) => self
                .signal
                .0
                .wake
                .wake()
                .map(|()| DeviceSessionSubmission::Submitted),
            Err(TrySendError::Full(intent)) => Ok(DeviceSessionSubmission::Busy { intent }),
            Err(TrySendError::Disconnected(intent)) => {
                Ok(DeviceSessionSubmission::Stopped { intent })
            }
        }
    }

    pub fn shutdown(&self) -> Result<(), std::io::Error> {
        self.signal.0.stopping.store(true, Ordering::Release);
        self.signal.0.wake.wake()
    }
}

pub(super) struct StopOnDrop(pub(super) Arc<SessionSignal>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stopping.store(true, Ordering::Release);
        self.0.wake_by_ref();
    }
}
