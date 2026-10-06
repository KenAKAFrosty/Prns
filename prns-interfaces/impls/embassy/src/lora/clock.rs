use embassy_time::{Duration, Instant, Timer};

pub(super) trait RadioClock {
    fn now(&self) -> Instant;
    async fn wait(&self, duration: Duration);
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
