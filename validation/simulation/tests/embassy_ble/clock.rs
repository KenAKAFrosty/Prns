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
        let before = self.snapshot();
        let (send, mut result) = tokio::sync::oneshot::channel();
        let operation = self.insert(async move {
            assert!(send.send(future.await).is_ok());
        });
        for _ in 0..SETTLEMENT_POLL_BUDGET {
            match self.runner.poll_next().unwrap() {
                ManualTaskPoll::Pending { .. } => {}
                ManualTaskPoll::Completed { task, output: () } => {
                    assert_eq!(task, operation, "only the operation may complete");
                    let _ = self.settle();
                    assert_eq!(self.snapshot(), before, "operation must not advance time");
                    return result.try_recv().unwrap();
                }
                ManualTaskPoll::Idle => unreachable!("operation stalled before completion"),
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
