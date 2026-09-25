use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use super::{ManualAdvance, ManualTimeDriver, ManualTimeError, ManualTimeSnapshot};
use crate::SimulationTick;

mod ready;
use ready::{ReadyTasks, TaskWake};

/// An admission ordinal scoped to one runner. Ordinals are never reused within that runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ManualTaskId(u64);

#[derive(Debug, PartialEq, Eq)]
pub enum ManualTaskAdmissionError {
    Capacity { maximum: NonZeroUsize },
    IdsExhausted,
}

impl fmt::Display for ManualTaskAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity { maximum } => {
                write!(formatter, "manual task capacity {maximum} reached")
            }
            Self::IdsExhausted => formatter.write_str("manual task ordinals exhausted"),
        }
    }
}

impl std::error::Error for ManualTaskAdmissionError {}

#[derive(Debug, PartialEq, Eq)]
pub enum ManualTaskPoll<T> {
    /// No registered task was ready at inspection. This does not imply global quiescence.
    Idle,
    Pending {
        task: ManualTaskId,
    },
    Completed {
        task: ManualTaskId,
        output: T,
    },
}

struct Task<T> {
    future: Pin<Box<dyn Future<Output = T>>>,
    waker: Waker,
}

/// Bounded, wake-driven futures borrowing one manual clock driver for their entire lifetime.
/// Each call polls at most one ready future, in cyclic admission order. Futures may be non-Send,
/// but must cooperate by returning from each poll and obey the manual driver's restrictions.
/// The caller owns the overall poll budget and supplies known runtime deadline boundaries.
pub struct ManualTaskRunner<'driver, T> {
    driver: &'driver mut ManualTimeDriver,
    maximum: NonZeroUsize,
    next_id: Option<u64>,
    tasks: BTreeMap<ManualTaskId, Task<T>>,
    ready: Arc<Mutex<ReadyTasks>>,
}

impl<'driver, T> ManualTaskRunner<'driver, T> {
    pub fn new(driver: &'driver mut ManualTimeDriver, maximum: NonZeroUsize) -> Self {
        Self {
            driver,
            maximum,
            next_id: Some(0),
            tasks: BTreeMap::new(),
            ready: Arc::new(Mutex::new(ReadyTasks::new())),
        }
    }

    /// Admission queues the first poll. Refusal drops the unpolled future without consuming an ID.
    pub fn insert<F: Future<Output = T> + 'static>(
        &mut self,
        future: F,
    ) -> Result<ManualTaskId, ManualTaskAdmissionError> {
        if self.tasks.len() == self.maximum.get() {
            return Err(ManualTaskAdmissionError::Capacity {
                maximum: self.maximum,
            });
        }
        let ordinal = self.next_id.ok_or(ManualTaskAdmissionError::IdsExhausted)?;
        let id = ManualTaskId(ordinal);
        self.next_id = ordinal.checked_add(1);
        let waker = Waker::from(Arc::new(TaskWake::new(id, Arc::downgrade(&self.ready))));
        self.tasks.insert(
            id,
            Task {
                future: Box::pin(future),
                waker,
            },
        );
        ReadyTasks::lock(&self.ready).insert(id);
        Ok(id)
    }

    /// Completed outputs leave the runner immediately; there is no retained completion queue.
    /// An error after polling does not undo task effects or recover its output; discard the runner.
    pub fn poll_next(&mut self) -> Result<ManualTaskPoll<T>, ManualTimeError> {
        let _ = self.driver.validate()?;
        let Some(id) = ReadyTasks::lock(&self.ready).pop_next() else {
            return Ok(ManualTaskPoll::Idle);
        };
        let report = {
            // Enter timer context without a Tokio scheduler turn: this runner owns the waker.
            // A short-lived block_on would discard Tokio's deferred cooperative-yield wakes.
            let _entered = self.driver.runtime.enter();
            let task = self
                .tasks
                .get_mut(&id)
                .unwrap_or_else(|| unreachable!("only registered tasks can be ready"));
            let mut context = Context::from_waker(&task.waker);
            match task.future.as_mut().poll(&mut context) {
                Poll::Pending => ManualTaskPoll::Pending { task: id },
                Poll::Ready(output) => {
                    ReadyTasks::lock(&self.ready).retire(id);
                    self.tasks.remove(&id);
                    ManualTaskPoll::Completed { task: id, output }
                }
            }
        };
        let _ = self.driver.validate()?;
        Ok(report)
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn snapshot(&self) -> Result<ManualTimeSnapshot, ManualTimeError> {
        self.driver.snapshot()
    }

    /// Refuses advancement while registered actors are ready. Does not infer timer deadlines.
    pub fn advance_to_next_event(
        &mut self,
        not_after: SimulationTick,
    ) -> Result<ManualAdvance, ManualTimeError> {
        let _ = self.driver.validate()?;
        let count = ReadyTasks::lock(&self.ready).ready_count();
        if count != 0 {
            return Err(ManualTimeError::ReadyTasks { count });
        }
        self.driver.advance_to_next_event(not_after)
    }
}

#[cfg(test)]
mod tests;
