use std::future::{poll_fn, Future};
use std::num::NonZeroU64;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use tokio::runtime::{Builder, Handle, Runtime};
use tokio::time::Instant;

use crate::SimulationTick;

mod error;
mod medium;
mod tasks;
pub use error::ManualTimeError;
pub use medium::{ManualAdvance, ManualMedium};
pub use tasks::{
    ManualTaskAdmissionError, ManualTaskCancellation, ManualTaskId, ManualTaskPoll,
    ManualTaskRunner, ManualTaskScheduling, SEEDED_TASK_SCHEDULING_ALGORITHM_VERSION,
};

#[derive(Debug, PartialEq, Eq)]
pub struct ManualTimeSnapshot {
    pub tick: SimulationTick,
    pub runtime_elapsed: Duration,
}

/// A private paused runtime for explicitly polled futures and coordinated media.
/// The caller supplies known runtime deadlines as advancement boundaries and owns poll/work budgets.
/// Futures must not spawn tasks, perform blocking work, export runtime handles, or alter Tokio time.
/// Medium handles must not be mutated concurrently. This is not a general-purpose Tokio executor.
/// Create, drive, and drop this owner outside Tokio; construct timer state inside the polled futures.
pub struct ManualTimeDriver {
    runtime: Runtime,
    medium: ManualMedium,
    tick_millis: NonZeroU64,
    origin_tick: SimulationTick,
    origin_instant: Instant,
    tick: SimulationTick,
}

impl ManualTimeDriver {
    /// Binds one medium's current tick to a fresh paused runtime. A tick must be a nonzero whole
    /// number of milliseconds; the existing medium origin need not be zero.
    pub fn new(medium: ManualMedium, tick_duration: Duration) -> Result<Self, ManualTimeError> {
        outside_runtime()?;
        let tick_millis = u64::try_from(tick_duration.as_millis())
            .ok()
            .and_then(NonZeroU64::new)
            .filter(|millis| Duration::from_millis(millis.get()) == tick_duration)
            .ok_or(ManualTimeError::InvalidTickDuration {
                requested: tick_duration,
            })?;
        let runtime = Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .map_err(ManualTimeError::RuntimeBuild)?;
        let origin_instant = {
            let _entered = runtime.enter();
            Instant::now()
        };
        let origin_tick = medium.now()?;
        Ok(Self {
            runtime,
            medium,
            tick_millis,
            origin_tick,
            origin_instant,
            tick: origin_tick,
        })
    }

    /// Reports coordinated time, refusing external clock changes or live spawned async tasks.
    pub fn snapshot(&self) -> Result<ManualTimeSnapshot, ManualTimeError> {
        self.validate()?;
        Ok(ManualTimeSnapshot {
            tick: self.tick,
            runtime_elapsed: self.instant_at(self.tick)? - self.origin_instant,
        })
    }

    /// Polls exactly once without idling the runtime or implicitly advancing time.
    /// An error after polling does not undo the future's effects; discard a drifted driver.
    pub fn poll<F: Future>(
        &mut self,
        mut future: Pin<&mut F>,
    ) -> Result<Poll<F::Output>, ManualTimeError> {
        self.validate()?;
        let result = self.runtime.block_on(poll_fn(|context| {
            Poll::Ready(future.as_mut().poll(context))
        }));
        self.validate()?;
        Ok(result)
    }

    /// Settles the next medium event no later than the caller's runtime deadline, then advances Tokio.
    /// All same-tick medium effects precede the caller's next explicit future poll. This does not
    /// discover Tokio deadlines, poll expired timers to completion, or establish task quiescence.
    pub fn advance_to_next_event(
        &mut self,
        not_after: SimulationTick,
    ) -> Result<ManualAdvance, ManualTimeError> {
        self.validate()?;
        if not_after < self.tick {
            return Err(ManualTimeError::BeforeCurrent {
                current: self.tick,
                requested: not_after,
            });
        }
        let target = self.medium.schedule()?.target_not_after(not_after);
        let _ = self.instant_at(target)?;
        // The upper bound also keeps a changed schedule inside the prevalidated clock range.
        let report = self.medium.advance(target)?;
        let (from, to) = report.bounds();
        if from != self.tick {
            return Err(ManualTimeError::MediumDrift {
                expected: self.tick,
                observed: from,
            });
        }
        let duration = self.instant_at(to)? - self.instant_at(self.tick)?;
        self.runtime.block_on(tokio::time::advance(duration));
        self.tick = to;
        self.validate()?;
        Ok(report)
    }

    fn instant_at(&self, tick: SimulationTick) -> Result<Instant, ManualTimeError> {
        let millis = tick
            .get()
            .checked_sub(self.origin_tick.get())
            .and_then(|ticks| ticks.checked_mul(self.tick_millis.get()))
            .ok_or(ManualTimeError::ClockRange { tick })?;
        self.origin_instant
            .checked_add(Duration::from_millis(millis))
            .ok_or(ManualTimeError::ClockRange { tick })
    }

    fn validate(&self) -> Result<(), ManualTimeError> {
        outside_runtime()?;
        let count = self.runtime.metrics().num_alive_tasks();
        if count != 0 {
            return Err(ManualTimeError::SpawnedTasks { count });
        }
        let observed = {
            let _entered = self.runtime.enter();
            Instant::now()
        };
        let expected = self.instant_at(self.tick)?;
        if observed != expected {
            return Err(ManualTimeError::ClockDrift {
                expected: expected.saturating_duration_since(self.origin_instant),
                observed: observed.saturating_duration_since(self.origin_instant),
            });
        }
        let observed = self.medium.now()?;
        if observed != self.tick {
            return Err(ManualTimeError::MediumDrift {
                expected: self.tick,
                observed,
            });
        }
        Ok(())
    }
}

fn outside_runtime() -> Result<(), ManualTimeError> {
    if Handle::try_current().is_ok() {
        return Err(ManualTimeError::InsideRuntime);
    }
    Ok(())
}

#[cfg(test)]
mod mixed_tests;
#[cfg(test)]
mod tests;
