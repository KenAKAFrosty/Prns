//! Coalesced owner invalidation, separate from durable record/change cursors.
use crate::ApiError;
use prns_groups_store_redb::ChangeWitness;
use std::{
    future::poll_fn,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    task::{Poll, Waker},
};

/// Fixed per-owner registration bound. Each live subscription retains one hint
/// and at most one waiting task; there is no event backlog or delivery receipt.
pub const MAX_OBSERVATIONS: usize = 32;

#[derive(Default)]
struct State {
    pending: bool,
    closed: bool,
    waiter: Option<Waker>,
}

/// Subscribe before reading a snapshot, then wait and refresh using the ordinary
/// queries/durable cursors. Hints coalesce and never enumerate committed events.
/// A late subscriber starts with no historical hint and must read a snapshot.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct GroupObservation {
    state: Arc<Mutex<State>>,
    waiting: AtomicBool,
}

#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl GroupObservation {
    /// Observe possible change. Only one next() may be pending per subscription;
    /// another returns Busy. Dropping a pending wait preserves its unconsumed
    /// hint and releases the waiter. Unavailable means the owner has closed;
    /// reopen and subscribe again, never infer a domain mutation was rejected.
    pub async fn next(&self) -> Result<(), ApiError> {
        if self.waiting.swap(true, Ordering::AcqRel) {
            return Err(ApiError::Busy);
        }
        let _waiting = Waiting(self);
        poll_fn(|context| {
            // Waker clone/drop/wake are user callbacks. None run under our lock.
            let replacement = context.waker().clone();
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return Poll::Ready(Err(ApiError::Unavailable)),
            };
            let result = if state.closed {
                Poll::Ready(Err(ApiError::Unavailable))
            } else if state.pending {
                state.pending = false;
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            };
            let previous = if result.is_pending() {
                state.waiter.replace(replacement)
            } else {
                drop(state);
                drop(replacement);
                return result;
            };
            drop(state);
            drop(previous);
            result
        })
        .await
    }
}
struct Waiting<'a>(&'a GroupObservation);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let previous = match self.0.state.lock() {
            Ok(mut state) => state.waiter.take(),
            Err(poisoned) => poisoned.into_inner().waiter.take(),
        };
        // Release the ownership flag before dropping a user-provided waker.
        self.0.waiting.store(false, Ordering::Release);
        drop(previous);
    }
}

#[derive(Default)]
pub(crate) struct Source {
    witness: ChangeWitness,
    subscribers: Vec<Weak<Mutex<State>>>,
}
impl Source {
    pub(crate) fn witness(&self) -> ChangeWitness {
        self.witness.clone()
    }

    pub(crate) fn subscribe(&mut self) -> Result<Arc<GroupObservation>, ApiError> {
        self.subscribers.retain(|slot| slot.strong_count() != 0);
        if self.subscribers.len() == MAX_OBSERVATIONS {
            return Err(ApiError::Busy);
        }
        self.subscribers
            .try_reserve_exact(1)
            .map_err(|_| ApiError::Unavailable)?;
        let state = Arc::new(Mutex::new(State::default()));
        self.subscribers.push(Arc::downgrade(&state));
        Ok(Arc::new(GroupObservation {
            state,
            waiting: AtomicBool::new(false),
        }))
    }

    pub(crate) fn publish(&mut self) {
        if !self.witness.take_possible_change() {
            return;
        }
        self.subscribers.retain(|slot| {
            if let Some(state) = slot.upgrade() {
                signal(&state, false);
                true
            } else {
                false
            }
        });
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        for state in self.subscribers.iter().filter_map(Weak::upgrade) {
            signal(&state, true);
        }
    }
}
fn signal(state: &Mutex<State>, closed: bool) {
    let waiter = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending = true;
        state.closed |= closed;
        state.waiter.take()
    };
    if let Some(waiter) = waiter {
        // Tokio watch/Notify unlinks remaining waiters without waking them when
        // a custom waker panics. Isolate each wake here so one Rust consumer
        // cannot strand other subscriptions or unwind the storage owner.
        let _ = catch_unwind(AssertUnwindSafe(|| waiter.wake()));
    }
}

/// A standalone manager owns its source; a lexical manager borrows the
/// persistent attachment's source without closing it between jobs.
pub(crate) enum SourceLease<'a> {
    Owned(Source),
    Borrowed(&'a mut Source),
}
impl core::ops::Deref for SourceLease<'_> {
    type Target = Source;
    fn deref(&self) -> &Source {
        match self {
            Self::Owned(source) => source,
            Self::Borrowed(source) => source,
        }
    }
}
impl core::ops::DerefMut for SourceLease<'_> {
    fn deref_mut(&mut self) -> &mut Source {
        match self {
            Self::Owned(source) => source,
            Self::Borrowed(source) => source,
        }
    }
}

#[cfg(test)]
#[path = "observation/tests.rs"]
mod tests;
