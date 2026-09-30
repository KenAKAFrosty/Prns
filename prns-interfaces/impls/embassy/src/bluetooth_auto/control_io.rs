//! Bounded native control operations. These receipts describe local transport completion,
//! never peer liveness or remote application receipt.

use core::cell::RefCell;
use core::future::{poll_fn, Future};
use core::pin::{pin, Pin};
use core::task::Poll;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::waitqueue::WakerRegistration;
use embassy_time::{with_deadline, Duration, Instant};
use prns_core::interfaces::bluetooth_auto::{Control, CONTROL_MAX_LEN};

use super::connection_slots::ConnectionSlotWorkerLease;

pub mod availability;
pub mod slot_claims;

pub const CONTROL_OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlIoError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlTicket {
    sequence: u32,
    deadline: Instant,
}

impl ControlTicket {
    pub fn deadline(self) -> Instant {
        self.deadline
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Waiting,
    Submitted,
    Completed(bool),
}

struct Operation {
    ticket: ControlTicket,
    digest: [u8; 32],
    phase: Phase,
}

struct State {
    sequence: u32,
    operation: Option<Operation>,
    retired: bool,
    waiter: WakerRegistration,
}

/// One operation belongs to one leased physical slot. Reset is only valid after its previous
/// worker and all owners have exited. The sequence is deliberately not reset on slot reuse.
pub struct ControlCompletion<M: RawMutex> {
    state: BlockingMutex<M, RefCell<State>>,
}

impl<M: RawMutex> Default for ControlCompletion<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex> ControlCompletion<M> {
    pub const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(State {
                sequence: 0,
                operation: None,
                retired: false,
                waiter: WakerRegistration::new(),
            })),
        }
    }

    pub fn reset(&self) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.waiter.wake();
            state.operation = None;
            state.retired = false;
        });
    }

    /// Preserve both the original deadline and message identity before queue/pool admission.
    /// The digest checks resumption consistency, not peer authentication. Encoding and hashing
    /// happen synchronously so the scratch payload never enlarges a suspended slot future.
    pub fn start(&self, control: &Control, now: Instant) -> Result<ControlTicket, ControlIoError> {
        let digest = control_digest(control)?;
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.retired {
                return Err(ControlIoError);
            }
            if let Some(operation) = &state.operation {
                if operation.digest == digest
                    && (now < operation.ticket.deadline
                        || matches!(operation.phase, Phase::Completed(_)))
                {
                    return Ok(operation.ticket);
                }
                state.retired = true;
                state.waiter.wake();
                return Err(ControlIoError);
            }
            let Some(sequence) = state.sequence.checked_add(1) else {
                state.retired = true;
                return Err(ControlIoError);
            };
            state.sequence = sequence;
            let ticket = ControlTicket {
                sequence,
                deadline: now + CONTROL_OPERATION_TIMEOUT,
            };
            state.operation = Some(Operation {
                ticket,
                digest,
                phase: Phase::Waiting,
            });
            Ok(ticket)
        })
    }

    pub fn submitted(&self, ticket: ControlTicket) -> bool {
        self.state.lock(|state| {
            let state = state.borrow();
            state.operation.as_ref().is_some_and(|operation| {
                operation.ticket == ticket && !matches!(operation.phase, Phase::Waiting)
            })
        })
    }

    /// Called in the same synchronous turn as queue admission; never across an await.
    pub fn admit(&self, ticket: ControlTicket) -> Result<(), ControlIoError> {
        self.admit_at(ticket, Instant::now())
    }

    pub fn admit_at(&self, ticket: ControlTicket, now: Instant) -> Result<(), ControlIoError> {
        self.admit_with(ticket, now, || true).map(|_| ())
    }

    /// Publish the queue entry and its admitted state under the same critical section. A worker
    /// woken by publication cannot observe an entry without its matching operation ticket.
    pub fn admit_with(
        &self,
        ticket: ControlTicket,
        now: Instant,
        enqueue: impl FnOnce() -> bool,
    ) -> Result<bool, ControlIoError> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.retired || now >= ticket.deadline {
                return Err(ControlIoError);
            }
            let operation = state.operation.as_mut().ok_or(ControlIoError)?;
            if operation.ticket != ticket || !matches!(operation.phase, Phase::Waiting) {
                return Err(ControlIoError);
            }
            let admitted = enqueue();
            if admitted {
                operation.phase = Phase::Submitted;
            }
            Ok(admitted)
        })
    }

    pub fn pending_ticket(&self) -> Option<ControlTicket> {
        self.state.lock(|state| {
            state.borrow().operation.as_ref().and_then(|operation| {
                matches!(operation.phase, Phase::Submitted).then_some(operation.ticket)
            })
        })
    }

    pub fn complete(&self, ticket: ControlTicket, success: bool) -> bool {
        self.complete_at(ticket, success, Instant::now())
    }

    pub fn complete_at(&self, ticket: ControlTicket, success: bool, now: Instant) -> bool {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.retired {
                return false;
            }
            let Some(operation) = state.operation.as_mut() else {
                return false;
            };
            if operation.ticket != ticket || !matches!(operation.phase, Phase::Submitted) {
                return false;
            }
            operation.phase = Phase::Completed(success && now < ticket.deadline);
            state.waiter.wake();
            true
        })
    }

    pub async fn wait(&self, ticket: ControlTicket) -> Result<(), ControlIoError> {
        poll_fn(|cx| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.retired {
                    return Poll::Ready(Err(ControlIoError));
                }
                let Some(operation) = &state.operation else {
                    return Poll::Ready(Err(ControlIoError));
                };
                if operation.ticket != ticket {
                    return Poll::Ready(Err(ControlIoError));
                }
                if let Phase::Completed(success) = operation.phase {
                    return Poll::Ready(if success { Ok(()) } else { Err(ControlIoError) });
                }
                state.waiter.register(cx.waker());
                Poll::Pending
            })
        })
        .await
    }

    pub fn acknowledge(&self, ticket: ControlTicket) -> Result<(), ControlIoError> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let Some(operation) = &state.operation else {
                return Err(ControlIoError);
            };
            if state.retired
                || operation.ticket != ticket
                || !matches!(operation.phase, Phase::Completed(true))
            {
                return Err(ControlIoError);
            }
            state.operation = None;
            Ok(())
        })
    }

    pub fn retire(&self) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.retired = true;
            state.waiter.wake();
        });
    }
}

fn control_digest(control: &Control) -> Result<[u8; 32], ControlIoError> {
    let mut bytes = [0; CONTROL_MAX_LEN];
    let length = control.encode(&mut bytes).ok_or(ControlIoError)?;
    Ok(prns_core::crypto::sha256(&bytes[..length]))
}

/// The ATT response lane has no operation IDs. Any canceled/failed admitted transaction poisons
/// the physical connection *before* releasing the gate, so another write cannot steal its reply.
/// The arbiter borrows one exact worker owner for its whole lifetime; that owner's sticky close
/// latch is the poison state, and its lease prevents slot reuse while the arbiter can be used.
pub struct GattWriteArbiter<'a, M: RawMutex + 'static> {
    gate: Mutex<M, ()>,
    worker: &'a ConnectionSlotWorkerLease<M>,
}

impl<'a, M: RawMutex> GattWriteArbiter<'a, M> {
    pub const fn new(worker: &'a ConnectionSlotWorkerLease<M>) -> Self {
        Self {
            gate: Mutex::new(()),
            worker,
        }
    }

    pub async fn run<T>(
        &self,
        deadline: Instant,
        operation: impl Future<Output = Result<T, ControlIoError>>,
    ) -> Result<T, ControlIoError> {
        // Only the native operation varies between callers. Erase it after
        // pinning in-place so the gate, deadline and retirement poll machinery
        // has one implementation per result type, without a heap allocation.
        let operation = pin!(operation);
        self.run_pinned(deadline, operation).await
    }

    /// Run an operation pinned by the caller. Embedded workers can use this directly to
    /// avoid storing a second copy of the operation in the generic async wrapper's state.
    /// The operation stays in caller-owned storage; no heap allocation is introduced.
    pub async fn run_pinned<T>(
        &self,
        deadline: Instant,
        operation: Pin<&mut dyn Future<Output = Result<T, ControlIoError>>>,
    ) -> Result<T, ControlIoError> {
        let guard = match with_deadline(deadline, self.gate.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                self.worker.request_close();
                return Err(ControlIoError);
            }
        };
        if self.worker.is_closed() || Instant::now() >= deadline {
            self.worker.request_close();
            return Err(ControlIoError);
        }
        let mut retirement = RetireWriteOnDrop {
            worker: self.worker,
            armed: true,
        };
        let result = with_deadline(deadline, operation)
            .await
            .map_err(|_| ControlIoError)
            .and_then(core::convert::identity)
            .and_then(|value| {
                if Instant::now() < deadline {
                    Ok(value)
                } else {
                    Err(ControlIoError)
                }
            });
        if result.is_ok() {
            retirement.armed = false;
        }
        drop(retirement);
        drop(guard);
        result
    }
}

struct RetireWriteOnDrop<'a, M: RawMutex + 'static> {
    worker: &'a ConnectionSlotWorkerLease<M>,
    armed: bool,
}

impl<M: RawMutex> Drop for RetireWriteOnDrop<'_, M> {
    fn drop(&mut self) {
        if self.armed {
            self.worker.request_close();
        }
    }
}

/// One connection-local notification queue entry, shared by every characteristic. Registration
/// precedes native submission. Surplus counts never become credit for a future notification.
pub struct NotificationCompletion<M: RawMutex> {
    state: BlockingMutex<M, RefCell<NotificationState>>,
}

struct NotificationState {
    pending: bool,
    complete: bool,
    waiter: WakerRegistration,
}

impl<M: RawMutex> Default for NotificationCompletion<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex> NotificationCompletion<M> {
    pub const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(NotificationState {
                pending: false,
                complete: false,
                waiter: WakerRegistration::new(),
            })),
        }
    }

    pub fn begin(&self) -> Result<(), ControlIoError> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.pending {
                return Err(ControlIoError);
            }
            state.pending = true;
            state.complete = false;
            Ok(())
        })
    }

    pub fn completed(&self, count: u8) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if count != 0 && state.pending {
                state.complete = true;
                state.waiter.wake();
            }
        });
    }

    pub async fn wait(&self) {
        poll_fn(|cx| {
            self.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.pending && state.complete {
                    state.pending = false;
                    state.complete = false;
                    Poll::Ready(())
                } else {
                    state.waiter.register(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests;
