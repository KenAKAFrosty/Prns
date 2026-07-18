use embassy_time::{Duration, Timer};

use crate::engine::InstantMillis;
use crate::reactor::timebase::EmbassyTimebase;
use crate::reactor::Host;

/// A [`Host`] backed by embassy's clock and a caller-supplied entropy source: an [`EmbassyTimebase`] owns the clock and `draw_entropy` is whatever the board hands it. The engine never reads either; it asks the host.
pub struct EmbassyHost<E> {
    timebase: EmbassyTimebase,
    draw_entropy: E,
}

impl<E> EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    pub fn new(draw_entropy: E) -> Self {
        Self::new_with_timebase(EmbassyTimebase::capture_now(), draw_entropy)
    }

    pub fn new_with_timebase(timebase: EmbassyTimebase, draw_entropy: E) -> Self {
        Self {
            timebase,
            draw_entropy,
        }
    }
}

impl<E> Host for EmbassyHost<E>
where
    E: FnMut(&mut [u8]),
{
    fn now(&self) -> InstantMillis {
        self.timebase.now()
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        let remaining = deadline.0.saturating_sub(self.timebase.now().0);
        Timer::after(Duration::from_millis(remaining)).await;
    }

    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        (self.draw_entropy)(bytes);
    }
}
