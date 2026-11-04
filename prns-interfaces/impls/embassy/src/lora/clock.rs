use embassy_time::{Duration, Instant, Timer};

pub(super) trait RadioClock {
    fn now(&self) -> Instant;
    async fn wait(&self, duration: Duration);

    /// Relative scheduler accounting must not panic before the absolute queue
    /// timestamp checks can discard work after a faulty clock rolls backward.
    fn elapsed_millis(&self, started: Instant) -> u64 {
        self.now().as_millis().saturating_sub(started.as_millis())
    }
}

pub(super) struct EmbassyClock;

impl RadioClock for EmbassyClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn wait(&self, duration: Duration) {
        Timer::after(duration).await;
    }
}
