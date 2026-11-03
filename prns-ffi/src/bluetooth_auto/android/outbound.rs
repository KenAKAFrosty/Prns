use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;
use tokio::time::Instant;

use super::{AndroidBleError, CONTROL_BUFFER_LEN};

/// Transport-local receipt identity; never sent over Bluetooth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AndroidBleControlTicket {
    pub session: u64,
    pub operation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AndroidBleControlOutput {
    Empty,
    Closed,
    BufferTooSmall {
        required: usize,
    },
    Ready {
        len: usize,
        ticket: AndroidBleControlTicket,
    },
}

struct ControlOperation {
    ticket: AndroidBleControlTicket,
    bytes: Vec<u8>,
    deadline: Instant,
    result: Option<Result<(), AndroidBleError>>,
}

struct ControlState {
    open: bool,
    sequence: u64,
    pending: Option<ControlOperation>,
    watchdog_running: bool,
}

/// One bounded operation per exclusive control sender. State survives cancellation
/// of its waiting future: a retry rejoins the same bytes, ticket and deadline.
pub(super) struct ControlOutbox {
    session: u64,
    state: Mutex<ControlState>,
    changed: Notify,
}

impl ControlOutbox {
    pub(super) fn new(session: u64) -> Self {
        Self {
            session,
            state: Mutex::new(ControlState {
                open: true,
                sequence: 0,
                pending: None,
                watchdog_running: false,
            }),
            changed: Notify::new(),
        }
    }

    pub(super) fn begin(
        &self,
        bytes: &[u8],
        deadline: Instant,
    ) -> Result<(AndroidBleControlTicket, Instant, bool), AndroidBleError> {
        if bytes.len() > CONTROL_BUFFER_LEN {
            return Err(AndroidBleError::ControlTooLarge);
        }
        let mut state = self.state.lock().map_err(|_| AndroidBleError::Closed)?;
        if let Some(operation) = &state.pending {
            if operation.bytes != bytes {
                return Err(AndroidBleError::ControlBusy);
            }
            return Ok((operation.ticket, operation.deadline, false));
        }
        if !state.open {
            return Err(AndroidBleError::Closed);
        }
        let Some(sequence) = state.sequence.checked_add(1) else {
            state.open = false;
            return Err(AndroidBleError::Closed);
        };
        state.sequence = sequence;
        let ticket = AndroidBleControlTicket {
            session: self.session,
            operation: sequence,
        };
        state.pending = Some(ControlOperation {
            ticket,
            bytes: bytes.to_vec(),
            deadline,
            result: None,
        });
        let start_watchdog = !state.watchdog_running;
        state.watchdog_running = true;
        drop(state);
        self.changed.notify_waiters();
        Ok((ticket, deadline, start_watchdog))
    }

    pub(super) fn peek(&self, out: &mut [u8]) -> AndroidBleControlOutput {
        let Ok(state) = self.state.lock() else {
            return AndroidBleControlOutput::Closed;
        };
        if !state.open {
            return AndroidBleControlOutput::Closed;
        }
        let Some(operation) = &state.pending else {
            return AndroidBleControlOutput::Empty;
        };
        if operation.result.is_some() {
            return AndroidBleControlOutput::Empty;
        }
        if out.len() < operation.bytes.len() {
            return AndroidBleControlOutput::BufferTooSmall {
                required: operation.bytes.len(),
            };
        }
        out[..operation.bytes.len()].copy_from_slice(&operation.bytes);
        AndroidBleControlOutput::Ready {
            len: operation.bytes.len(),
            ticket: operation.ticket,
        }
    }

    pub(super) fn complete(
        &self,
        ticket: AndroidBleControlTicket,
        result: Result<(), AndroidBleError>,
    ) -> Option<Result<(), AndroidBleError>> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let operation = state.pending.as_mut()?;
        if operation.ticket != ticket || operation.result.is_some() {
            return None;
        }
        // Callback scheduling must not turn a late completion into success just
        // because the executor has not polled the watchdog yet.
        let result = if Instant::now() >= operation.deadline {
            Err(AndroidBleError::ControlTimedOut)
        } else {
            result
        };
        operation.result = Some(result);
        if result.is_err() {
            state.open = false;
        }
        drop(state);
        self.changed.notify_waiters();
        Some(result)
    }

    pub(super) async fn result(
        &self,
        ticket: AndroidBleControlTicket,
    ) -> Result<(), AndroidBleError> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.state.lock().map_err(|_| AndroidBleError::Closed)?;
                let Some(operation) = &state.pending else {
                    return Err(AndroidBleError::Closed);
                };
                if operation.ticket != ticket {
                    return Err(AndroidBleError::Closed);
                }
                if let Some(result) = operation.result {
                    state.pending = None;
                    return result;
                }
            }
            changed.await;
        }
    }

    /// The watchdog owns no physical lease, and exits on callback or shutdown,
    /// even when the sender is no longer polling its future.
    pub(super) async fn expired(&self) -> bool {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (ticket, deadline) = {
                let Ok(mut state) = self.state.lock() else {
                    return false;
                };
                match state
                    .pending
                    .as_ref()
                    .filter(|operation| operation.result.is_none())
                {
                    Some(operation) => (operation.ticket, operation.deadline),
                    None => {
                        state.watchdog_running = false;
                        return false;
                    }
                }
            };
            tokio::select! {
                () = changed => {},
                () = tokio::time::sleep_until(deadline) => {
                    if self.complete(ticket, Err(AndroidBleError::ControlTimedOut)).is_some() {
                        if let Ok(mut state) = self.state.lock() { state.watchdog_running = false; }
                        return true;
                    }
                }
            }
        }
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.open = false;
            if let Some(operation) = &mut state.pending {
                if operation.result.is_none() {
                    operation.result = Some(Err(AndroidBleError::Closed));
                }
            }
        }
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn late_callback_is_a_timeout_even_before_the_watchdog_is_polled() {
        let queue = ControlOutbox::new(1);
        let (ticket, _, _) = queue
            .begin(&[1], Instant::now() + Duration::from_secs(30))
            .unwrap();
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            queue.complete(ticket, Ok(())),
            Some(Err(AndroidBleError::ControlTimedOut))
        );
        assert_eq!(
            queue.result(ticket).await,
            Err(AndroidBleError::ControlTimedOut)
        );
        assert_eq!(queue.peek(&mut [0; 1]), AndroidBleControlOutput::Closed);
    }

    #[tokio::test]
    async fn completed_operations_reuse_one_scheduled_watchdog_instead_of_spawning_unbounded_tasks()
    {
        let queue = Arc::new(ControlOutbox::new(1));
        let mut starts = 0;
        for _ in 0..100 {
            let (ticket, _, start) = queue
                .begin(&[1], Instant::now() + Duration::from_secs(30))
                .unwrap();
            starts += usize::from(start);
            assert_eq!(queue.complete(ticket, Ok(())), Some(Ok(())));
            assert_eq!(queue.result(ticket).await, Ok(()));
        }
        assert_eq!(starts, 1);
        // Simulate the one scheduled task first being polled after the burst.
        assert!(!queue.expired().await);
        assert!(!queue.state.lock().unwrap().watchdog_running);
        let (_, _, start) = queue
            .begin(&[2], Instant::now() + Duration::from_secs(30))
            .unwrap();
        assert!(start);
        queue.close();
        assert!(!queue.expired().await);
    }

    #[tokio::test]
    async fn callback_and_close_end_watchdog_without_waiting_for_deadline() {
        for close in [false, true] {
            let queue = Arc::new(ControlOutbox::new(1));
            let (ticket, _, _) = queue
                .begin(&[1], Instant::now() + Duration::from_secs(30))
                .unwrap();
            let observer = Arc::clone(&queue);
            let watchdog = tokio::spawn(async move { observer.expired().await });
            tokio::task::yield_now().await;
            if close {
                queue.close();
            } else {
                queue.complete(ticket, Ok(()));
            }
            assert!(!tokio::time::timeout(Duration::from_secs(1), watchdog)
                .await
                .unwrap()
                .unwrap());
            assert_eq!(Arc::strong_count(&queue), 1);
        }
    }

    #[tokio::test]
    async fn sequence_exhaustion_closes_instead_of_reusing_a_ticket() {
        let queue = ControlOutbox::new(1);
        queue.state.lock().unwrap().sequence = u64::MAX;
        assert_eq!(
            queue.begin(&[1], Instant::now()),
            Err(AndroidBleError::Closed)
        );
        assert_eq!(queue.peek(&mut [0; 1]), AndroidBleControlOutput::Closed);
    }
}

#[derive(Debug)]
pub(super) enum OutboundQueueError {
    Closed,
    ItemTooLarge,
}

struct MessageQueueState {
    open: bool,
    messages: VecDeque<Vec<u8>>,
}

pub(super) struct BoundedMessageQueue {
    limit: usize,
    state: Mutex<MessageQueueState>,
    space: Notify,
}

impl BoundedMessageQueue {
    pub(super) fn with_count_limit(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(MessageQueueState {
                open: true,
                messages: VecDeque::new(),
            }),
            space: Notify::new(),
        }
    }

    pub(super) async fn push(&self, messages: Vec<Vec<u8>>) -> Result<(), OutboundQueueError> {
        let added_count = messages.len();
        if added_count > self.limit {
            return Err(OutboundQueueError::ItemTooLarge);
        }

        loop {
            let space = self.space.notified();
            {
                let mut state = self.state.lock().map_err(|_| OutboundQueueError::Closed)?;
                if !state.open {
                    return Err(OutboundQueueError::Closed);
                }
                let admitted = state.messages.len().saturating_add(added_count) <= self.limit;
                if admitted {
                    state.messages.extend(messages);
                    return Ok(());
                }
            }
            space.await;
        }
    }

    pub(super) fn peek(&self, out: &mut [u8]) -> usize {
        let Ok(state) = self.state.lock() else {
            return 0;
        };
        let Some(message) = state.messages.front() else {
            return 0;
        };
        if message.len() > out.len() {
            return 0;
        }
        out[..message.len()].copy_from_slice(message);
        message.len()
    }

    pub(super) fn commit(&self) -> bool {
        let committed = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.messages.pop_front())
            .is_some();
        if committed {
            self.space.notify_one();
        }
        committed
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.open = false;
            state.messages.clear();
        }
        self.space.notify_waiters();
    }
}

struct ByteQueueState {
    open: bool,
    bytes: VecDeque<u8>,
}

pub(super) struct BoundedByteQueue {
    limit: usize,
    state: Mutex<ByteQueueState>,
    space: Notify,
}

impl BoundedByteQueue {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(ByteQueueState {
                open: true,
                bytes: VecDeque::new(),
            }),
            space: Notify::new(),
        }
    }

    pub(super) async fn push(&self, bytes: &[u8]) -> Result<(), OutboundQueueError> {
        if bytes.len() > self.limit {
            return Err(OutboundQueueError::ItemTooLarge);
        }
        loop {
            let space = self.space.notified();
            {
                let mut state = self.state.lock().map_err(|_| OutboundQueueError::Closed)?;
                if !state.open {
                    return Err(OutboundQueueError::Closed);
                }
                if state.bytes.len().saturating_add(bytes.len()) <= self.limit {
                    state.bytes.extend(bytes.iter().copied());
                    return Ok(());
                }
            }
            space.await;
        }
    }

    pub(super) fn drain(&self, out: &mut [u8]) -> usize {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        let mut written = 0;
        for slot in out.iter_mut() {
            let Some(byte) = state.bytes.pop_front() else {
                break;
            };
            *slot = byte;
            written += 1;
        }
        drop(state);
        if written != 0 {
            self.space.notify_one();
        }
        written
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.open = false;
            state.bytes.clear();
        }
        self.space.notify_waiters();
    }
}
