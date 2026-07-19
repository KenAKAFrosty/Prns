use std::time::Duration;

use tokio::time::Instant;

use crate::engine::InstantMillis;
use crate::reactor::Host;

const MAX_TIMER_ARM_MILLIS: u64 = 24 * 60 * 60 * 1_000;

pub(super) fn bounded_timer_deadline(
    now: Instant,
    logical_now: InstantMillis,
    at: InstantMillis,
) -> Instant {
    let delay = at.0.saturating_sub(logical_now.0).min(MAX_TIMER_ARM_MILLIS);
    now.checked_add(Duration::from_millis(delay)).unwrap_or(now)
}

#[derive(Clone)]
pub struct TokioHost {
    base: Instant,
    logical_start: InstantMillis,
}

#[derive(Clone, Copy)]
pub(crate) struct TokioEntropy;

impl TokioEntropy {
    #[allow(clippy::expect_used)]
    pub(crate) fn fill(self, bytes: &mut [u8]) {
        getrandom::getrandom(bytes).expect("OS CSPRNG must provide reactor entropy");
    }
}

impl TokioHost {
    #[must_use]
    pub fn new() -> Self {
        Self::start_at(InstantMillis(0))
    }

    /// Mirrors `EmbassyTimebase::start_at`: the logical timeline resumes from `logical_start` instead of zero, so persisted timestamps stay in this boot's past.
    #[must_use]
    pub fn start_at(logical_start: InstantMillis) -> Self {
        Self {
            base: Instant::now(),
            logical_start,
        }
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for TokioHost {
    fn now(&self) -> InstantMillis {
        let elapsed = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        InstantMillis(self.logical_start.0.saturating_add(elapsed))
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        loop {
            let remaining = deadline.0.saturating_sub(self.now().0);
            if remaining == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(remaining.min(MAX_TIMER_ARM_MILLIS))).await;
        }
    }

    #[allow(clippy::expect_used)]
    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        TokioEntropy.fill(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn logical_time_saturates_at_the_numeric_limit() {
        let host = TokioHost::start_at(InstantMillis(u64::MAX - 5));
        tokio::time::advance(Duration::from_millis(10)).await;
        assert_eq!(host.now(), InstantMillis(u64::MAX));
    }

    #[tokio::test(start_paused = true)]
    async fn a_far_future_sleep_arms_without_overflowing_the_timer() {
        let host = TokioHost::new();
        let sleeping = host.sleep_until(InstantMillis(u64::MAX));
        tokio::pin!(sleeping);
        tokio::select! {
            () = &mut sleeping => panic!("the numeric limit is not immediately due"),
            () = tokio::time::sleep(Duration::from_millis(1)) => {}
        }
    }
}
