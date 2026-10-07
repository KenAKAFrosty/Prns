use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::{Mutex, MutexGuard};

use embassy_time::Instant;
use prns_runtime_tokio::runtime::{ControlledCrypto, ControlledCryptoError, ControlledCryptoStep};
use prns_simulation::{
    ManualAdvance, ManualTaskId, ManualTaskPoll, ManualTaskRunner, ManualTimeDriver,
    ManualTimeError, ManualTimeSnapshot, SimulationTick,
};

static CLOCK_OWNER: Mutex<()> = Mutex::new(());
mod driver;
pub(super) use driver::QueueStats;
mod tests;
const ACTOR_CAPACITY: usize = 8;
const SETTLEMENT_POLL_BUDGET: usize = 128;
const COMPLETION_POLL_BUDGET: usize = 512 * 1024;

// Embassy's time driver is process-global. Only this integration-test binary uses it;
// the lease serializes scenarios, and outlives every actor that may hold a timer.
pub(super) struct ClockLease {
    _owner: MutexGuard<'static, ()>,
}

impl ClockLease {
    pub(super) fn acquire() -> Self {
        let lease = Self {
            _owner: CLOCK_OWNER
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        };
        driver::reset();
        super::static_storage::reset_accounting();
        lease
    }
}

impl Drop for ClockLease {
    fn drop(&mut self) {
        driver::reset();
    }
}

pub(super) struct EmbassyTasks<'driver> {
    runner: ManualTaskRunner<'driver, ()>,
    _clock: ClockLease,
    settlement_poll_budget: NonZeroUsize,
    crypto: Vec<ControlledCrypto>,
}

enum CryptoProgress {
    Idle,
    Advanced,
}
const CRYPTO_GENERATION_CAPACITY: usize = 256;

pub(super) struct CompletionBudget {
    pub deadline: SimulationTick,
    pub polls_per_tick: NonZeroUsize,
}

impl<'driver> EmbassyTasks<'driver> {
    pub(super) fn new(driver: &'driver mut ManualTimeDriver, clock: ClockLease) -> Self {
        Self::with_limits(
            driver,
            clock,
            NonZeroUsize::new(ACTOR_CAPACITY).unwrap(),
            NonZeroUsize::new(SETTLEMENT_POLL_BUDGET).unwrap(),
            prns_simulation::ManualTaskScheduling::Cyclic,
        )
    }

    pub(super) fn with_limits(
        driver: &'driver mut ManualTimeDriver,
        clock: ClockLease,
        actors: NonZeroUsize,
        settlement_poll_budget: NonZeroUsize,
        scheduling: prns_simulation::ManualTaskScheduling,
    ) -> Self {
        let tasks = Self {
            runner: ManualTaskRunner::new_with_scheduling(driver, actors, scheduling),
            _clock: clock,
            settlement_poll_budget,
            crypto: Vec::new(),
        };
        assert_eq!(tasks.snapshot().runtime_elapsed, std::time::Duration::ZERO);
        tasks
    }

    pub(super) fn insert(&mut self, future: impl Future<Output = ()> + 'static) -> ManualTaskId {
        self.runner.insert(future).unwrap()
    }

    pub(super) fn register_crypto(&mut self, control: ControlledCrypto) {
        assert!(
            self.crypto.len() < CRYPTO_GENERATION_CAPACITY,
            "bounded crypto generations"
        );
        self.crypto.push(control);
    }

    fn step_crypto(&self) -> CryptoProgress {
        let mut progress = CryptoProgress::Idle;
        for control in &self.crypto {
            match control.step() {
                Ok(ControlledCryptoStep::Executed(_) | ControlledCryptoStep::Published(_)) => {
                    progress = CryptoProgress::Advanced;
                }
                Ok(
                    ControlledCryptoStep::Idle
                    | ControlledCryptoStep::Held
                    | ControlledCryptoStep::Backpressured,
                )
                | Err(ControlledCryptoError::NotAttached | ControlledCryptoError::Retired) => {}
                Err(error) => unreachable!("controlled worker ownership: {error}"),
            }
        }
        progress
    }

    pub(super) fn cancel(&mut self, task: ManualTaskId) -> prns_simulation::ManualTaskCancellation {
        let before = self.snapshot();
        let result = self.runner.cancel(task).unwrap();
        assert_eq!(self.snapshot(), before);
        result
    }

    pub(super) fn timer_stats(&self) -> QueueStats {
        driver::stats()
    }

    #[track_caller]
    pub(super) fn complete_ready<T: 'static>(
        &mut self,
        future: impl Future<Output = T> + 'static,
    ) -> T {
        self.complete_with_budget(
            CompletionBudget {
                deadline: self.snapshot().tick,
                polls_per_tick: self.settlement_poll_budget,
            },
            future,
        )
    }

    #[track_caller]
    pub(super) fn complete_with_budget<T: 'static>(
        &mut self,
        budget: CompletionBudget,
        future: impl Future<Output = T> + 'static,
    ) -> T {
        self.complete_with_budget_before_advance(budget, future, || {})
    }

    #[track_caller]
    fn complete_with_budget_before_advance<T: 'static>(
        &mut self,
        budget: CompletionBudget,
        future: impl Future<Output = T> + 'static,
        mut before_advance: impl FnMut(),
    ) -> T {
        let before = self.snapshot();
        assert!(budget.deadline >= before.tick);
        let (send, mut result) = tokio::sync::oneshot::channel();
        let operation = self.insert(async move {
            assert!(send.send(future.await).is_ok());
        });
        let mut polls_at_tick = 0;
        for _ in 0..COMPLETION_POLL_BUDGET {
            let _ = self.step_crypto();
            polls_at_tick += 1;
            assert!(
                polls_at_tick <= budget.polls_per_tick.get(),
                "operation failed to yield within its per-tick poll budget"
            );
            match self.runner.poll_next().unwrap() {
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { task, output: () } => {
                    assert_eq!(task, operation, "only the operation may complete");
                    let _ = self.settle();
                    assert!(self.snapshot().tick <= budget.deadline);
                    return result.try_recv().unwrap();
                }
                ManualTaskPoll::Idle => {
                    if matches!(self.step_crypto(), CryptoProgress::Advanced) {
                        continue;
                    }
                    let now = self.snapshot().tick.get();
                    assert!(
                        now < budget.deadline.get(),
                        "operation stalled before completion"
                    );
                    before_advance();
                    let advancement = self.advance_to_next_wake(budget.deadline);
                    // Idle is an observation; an external wake can arrive before
                    // the clock guard. Poll it at this tick within the same budget.
                    if matches!(
                        advancement,
                        Err(ClockAdvanceError::Manual(
                            ManualTimeError::ReadyTasks { .. }
                        ))
                    ) {
                        continue;
                    }
                    advancement.unwrap();
                    if self.snapshot().tick.get() != now {
                        polls_at_tick = 0;
                    }
                }
            }
            let _ = self.snapshot();
        }
        unreachable!("operation exceeded the explicit settlement poll budget")
    }

    pub(super) fn snapshot(&self) -> ManualTimeSnapshot {
        let snapshot = self.runner.snapshot().unwrap();
        assert_eq!(
            u128::from(Instant::now().as_micros()),
            snapshot.runtime_elapsed.as_micros(),
            "Embassy and the medium/Tokio clock must agree before any actor runs"
        );
        snapshot
    }

    pub(super) fn advance(
        &mut self,
        not_after: SimulationTick,
    ) -> Result<ManualAdvance, ManualTimeError> {
        let before = self.snapshot();
        let report = self.runner.advance_to_next_event(not_after)?;
        let after = self.runner.snapshot().unwrap();
        let elapsed = after
            .runtime_elapsed
            .checked_sub(before.runtime_elapsed)
            .unwrap();
        driver::advance(elapsed.as_micros().try_into().unwrap());
        let _ = self.snapshot();
        Ok(report)
    }

    pub(super) fn advance_to_next_wake(
        &mut self,
        horizon: SimulationTick,
    ) -> Result<ManualAdvance, ClockAdvanceError> {
        let before = self.snapshot();
        let boundary = match driver::next_deadline() {
            None => horizon,
            Some(micros) => {
                let remaining = micros.checked_sub(Instant::now().as_micros()).unwrap();
                if !remaining.is_multiple_of(1000) {
                    return Err(ClockAdvanceError::SubmillisecondDeadline { micros });
                }
                let tick = before
                    .tick
                    .get()
                    .checked_add(remaining / 1000)
                    .ok_or(ClockAdvanceError::ClockRange)?;
                horizon.min(SimulationTick::from_ticks(tick))
            }
        };
        let report = self
            .runner
            .advance_to_next_wake(boundary)
            .map_err(ClockAdvanceError::Manual)?;
        let after = self.runner.snapshot().unwrap();
        let elapsed = after
            .runtime_elapsed
            .checked_sub(before.runtime_elapsed)
            .unwrap();
        driver::advance(elapsed.as_micros().try_into().unwrap());
        let _ = self.snapshot();
        Ok(report)
    }

    pub(super) fn settle(&mut self) -> usize {
        let _ = self.snapshot();
        for polls in 0..self.settlement_poll_budget.get() {
            let _ = self.step_crypto();
            match self.runner.poll_next().unwrap() {
                ManualTaskPoll::Idle => match self.step_crypto() {
                    CryptoProgress::Idle => return polls,
                    CryptoProgress::Advanced => {}
                },
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { .. } => unreachable!("supervisors must remain live"),
            }
            let _ = self.snapshot();
        }
        unreachable!("supervisors exceeded the explicit settlement poll budget")
    }

    pub(super) fn poll_turns(&mut self, turns: NonZeroUsize) {
        let before = self.snapshot();
        for _ in 0..turns.get() {
            let _ = self.step_crypto();
            match self.runner.poll_next().unwrap() {
                ManualTaskPoll::Pending { .. } | ManualTaskPoll::Idle => {}
                ManualTaskPoll::Completed { .. } => unreachable!("tracked actors stay live"),
            }
        }
        assert_eq!(self.snapshot(), before);
    }
}

#[derive(Debug)]
pub(super) enum ClockAdvanceError {
    Manual(ManualTimeError),
    SubmillisecondDeadline { micros: u64 },
    ClockRange,
}

impl std::fmt::Display for ClockAdvanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual(error) => write!(f, "{error}"),
            Self::SubmillisecondDeadline { micros } => write!(
                f,
                "Embassy deadline {micros}us is not aligned to the millisecond simulation clock"
            ),
            Self::ClockRange => f.write_str("Embassy deadline exceeds the simulation clock range"),
        }
    }
}
