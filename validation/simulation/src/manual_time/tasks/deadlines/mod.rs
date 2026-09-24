use super::*;
use std::future::poll_fn;
use std::time::Duration;

impl<T> ManualTaskRunner<'_, T> {
    /// Uses the paused Tokio timer queue to stop at the first actor wake, medium
    /// event or caller horizon. Actors remain unpolled until medium effects settle.
    /// Requires millisecond ticks and the driver's no-external-work contract.
    /// This discovers Tokio timers, not Embassy timers or external worker deadlines.
    /// Discard the runner after a post-advance error; clock movement cannot be undone.
    pub fn advance_to_next_wake(
        &mut self,
        not_after: SimulationTick,
    ) -> Result<ManualAdvance, ManualTimeError> {
        self.driver.validate()?;
        if self.driver.tick_millis.get() != 1 {
            return Err(ManualTimeError::WakeSteppingRequiresMillisecondTicks);
        }
        let count = ReadyTasks::lock(&self.ready).ready_count();
        if count != 0 {
            return Err(ManualTimeError::ReadyTasks { count });
        }
        if not_after < self.driver.tick {
            return Err(ManualTimeError::BeforeCurrent {
                current: self.driver.tick,
                requested: not_after,
            });
        }
        let target = self.driver.medium.schedule()?.target_not_after(not_after);
        self.driver.medium.check_advance(target)?;
        let deadline = self.driver.instant_at(target)?;
        if target == self.driver.tick {
            return self.driver.advance_to_next_event(target);
        }
        let ready = self.ready.clone();
        self.driver.runtime.block_on(async {
            let wake = poll_fn(|context| {
                let mut ready = ReadyTasks::lock(&ready);
                if ready.ready_count() != 0 {
                    return Poll::Ready(());
                }
                ready.waiter = Some(context.waker().clone());
                Poll::Pending
            });
            tokio::select! {
                biased;
                _ = wake => {},
                _ = tokio::time::sleep_until(deadline) => {},
            }
        });
        ReadyTasks::lock(&self.ready).waiter = None;
        let observed = {
            let _entered = self.driver.runtime.enter();
            tokio::time::Instant::now()
        };
        let elapsed = observed - self.driver.origin_instant;
        let millis = u64::try_from(elapsed.as_millis())
            .map_err(|_| ManualTimeError::ClockRange { tick: target })?;
        if elapsed != Duration::from_millis(millis) || observed > deadline {
            return Err(ManualTimeError::ClockDrift {
                expected: deadline - self.driver.origin_instant,
                observed: elapsed,
            });
        }
        let tick = self
            .driver
            .origin_tick
            .get()
            .checked_add(millis)
            .map(SimulationTick::from_ticks)
            .ok_or(ManualTimeError::ClockRange { tick: target })?;
        let report = self.driver.medium.advance(tick)?;
        self.driver.tick = tick;
        self.driver.validate()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
