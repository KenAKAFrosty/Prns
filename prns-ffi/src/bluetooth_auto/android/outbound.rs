use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;

#[derive(Debug)]
pub(super) enum OutboundQueueError {
    Closed,
    ItemTooLarge,
}

enum MessageLimit {
    Bytes(usize),
    Count(usize),
}

struct MessageQueueState {
    open: bool,
    messages: VecDeque<Vec<u8>>,
}

pub(super) struct BoundedMessageQueue {
    limit: MessageLimit,
    state: Mutex<MessageQueueState>,
    space: Notify,
}

impl BoundedMessageQueue {
    pub(super) fn with_byte_limit(limit: usize) -> Self {
        Self {
            limit: MessageLimit::Bytes(limit),
            state: Mutex::new(MessageQueueState {
                open: true,
                messages: VecDeque::new(),
            }),
            space: Notify::new(),
        }
    }

    pub(super) fn with_count_limit(limit: usize) -> Self {
        Self {
            limit: MessageLimit::Count(limit),
            state: Mutex::new(MessageQueueState {
                open: true,
                messages: VecDeque::new(),
            }),
            space: Notify::new(),
        }
    }

    pub(super) async fn push(&self, messages: Vec<Vec<u8>>) -> Result<(), OutboundQueueError> {
        let added_count = messages.len();
        let added_bytes = messages.iter().map(Vec::len).sum::<usize>();
        let item_fits = match self.limit {
            MessageLimit::Bytes(limit) => added_bytes <= limit,
            MessageLimit::Count(limit) => added_count <= limit,
        };
        if !item_fits {
            return Err(OutboundQueueError::ItemTooLarge);
        }

        loop {
            let space = self.space.notified();
            {
                let mut state = self.state.lock().map_err(|_| OutboundQueueError::Closed)?;
                if !state.open {
                    return Err(OutboundQueueError::Closed);
                }
                let admitted = match self.limit {
                    MessageLimit::Bytes(limit) => {
                        state
                            .messages
                            .iter()
                            .map(Vec::len)
                            .sum::<usize>()
                            .saturating_add(added_bytes)
                            <= limit
                    }
                    MessageLimit::Count(limit) => {
                        state.messages.len().saturating_add(added_count) <= limit
                    }
                };
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
