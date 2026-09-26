use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::{Mutex, MutexGuard};

use embassy_time::{Duration, Instant, MockDriver};
use prns_simulation::{
    ManualAdvance, ManualTaskId, ManualTaskPoll, ManualTaskRunner, ManualTimeDriver,
    ManualTimeError, ManualTimeSnapshot, SimulationTick,
};

static CLOCK_OWNER: Mutex<()> = Mutex::new(());
const ACTOR_CAPACITY: usize = 8;
const SETTLEMENT_POLL_BUDGET: usize = 128;

// Embassy's mock driver is process-global. Only this integration-test binary uses it;
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
        MockDriver::get().reset();
        lease
    }
}

impl Drop for ClockLease {
    fn drop(&mut self) {
        MockDriver::get().reset();
    }
}

pub(super) struct EmbassyTasks<'driver> {
    runner: ManualTaskRunner<'driver, ()>,
    _clock: ClockLease,
}

pub(super) struct CompletionBudget {
    pub deadline: SimulationTick,
    pub polls_per_tick: NonZeroUsize,
}

impl<'driver> EmbassyTasks<'driver> {
    pub(super) fn new(driver: &'driver mut ManualTimeDriver, clock: ClockLease) -> Self {
        let tasks = Self {
            runner: ManualTaskRunner::new(driver, NonZeroUsize::new(ACTOR_CAPACITY).unwrap()),
            _clock: clock,
        };
        assert_eq!(tasks.snapshot().runtime_elapsed, std::time::Duration::ZERO);
        tasks
    }

    pub(super) fn insert(&mut self, future: impl Future<Output = ()> + 'static) -> ManualTaskId {
        self.runner.insert(future).unwrap()
    }

    #[track_caller]
    pub(super) fn complete_ready<T: 'static>(
        &mut self,
        future: impl Future<Output = T> + 'static,
    ) -> T {
        self.complete_with_budget(
            CompletionBudget {
                deadline: self.snapshot().tick,
                polls_per_tick: NonZeroUsize::new(SETTLEMENT_POLL_BUDGET).unwrap(),
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
        let before = self.snapshot();
        let ticks = budget
            .deadline
            .get()
            .checked_sub(before.tick.get())
            .unwrap();
        let poll_budget = usize::try_from(ticks.checked_add(1).unwrap())
            .unwrap()
            .checked_mul(budget.polls_per_tick.get())
            .unwrap();
        let (send, mut result) = tokio::sync::oneshot::channel();
        let operation = self.insert(async move {
            assert!(send.send(future.await).is_ok());
        });
        let mut polls_at_tick = 0;
        for _ in 0..poll_budget {
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
                    let now = self.snapshot().tick.get();
                    assert!(
                        now < budget.deadline.get(),
                        "operation stalled before completion"
                    );
                    self.advance(SimulationTick::from_ticks(now + 1)).unwrap();
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
        MockDriver::get().advance(Duration::from_micros(
            elapsed.as_micros().try_into().unwrap(),
        ));
        let _ = self.snapshot();
        Ok(report)
    }

    pub(super) fn settle(&mut self) -> usize {
        let _ = self.snapshot();
        for polls in 0..SETTLEMENT_POLL_BUDGET {
            match self.runner.poll_next().unwrap() {
                ManualTaskPoll::Idle => return polls,
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { .. } => unreachable!("supervisors must remain live"),
            }
            let _ = self.snapshot();
        }
        unreachable!("supervisors exceeded the explicit settlement poll budget")
    }
}
