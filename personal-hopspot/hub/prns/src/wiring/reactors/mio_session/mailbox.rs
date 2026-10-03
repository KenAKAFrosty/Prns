use crate::DeviceSessionMessage;
use alloc::sync::Arc;
use mio::Waker as MioWaker;
use std::sync::mpsc::{SyncSender, TrySendError};

#[derive(Clone)]
struct WakeOnDrop(Arc<MioWaker>);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        let _wake = self.0.wake();
    }
}

#[derive(Clone)]
pub struct MioSessionSender {
    sender: SyncSender<DeviceSessionMessage>,
    wake: WakeOnDrop,
}

#[must_use]
pub enum MioSessionSubmission {
    Submitted,
    Busy { message: DeviceSessionMessage },
    Closed { message: DeviceSessionMessage },
}

#[derive(Debug)]
pub struct MioSessionWakeFailure(pub std::io::Error);

impl MioSessionSender {
    pub(super) fn new(sender: SyncSender<DeviceSessionMessage>, wake: Arc<MioWaker>) -> Self {
        Self {
            sender,
            wake: WakeOnDrop(wake),
        }
    }

    pub fn submit(
        &self,
        message: DeviceSessionMessage,
    ) -> Result<MioSessionSubmission, MioSessionWakeFailure> {
        match self.sender.try_send(message) {
            Ok(()) => self
                .wake
                .0
                .wake()
                .map(|()| MioSessionSubmission::Submitted)
                .map_err(MioSessionWakeFailure),
            Err(TrySendError::Full(message)) => Ok(MioSessionSubmission::Busy { message }),
            Err(TrySendError::Disconnected(message)) => {
                Ok(MioSessionSubmission::Closed { message })
            }
        }
    }
}
