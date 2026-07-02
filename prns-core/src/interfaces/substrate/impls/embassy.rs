use embassy_time::Instant as EmbassyInstant;

use crate::engine::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbassyTimebase {
    raw_start: EmbassyInstant,
    logical_start: InstantMillis,
}

impl EmbassyTimebase {
    pub fn capture_now() -> Self {
        let raw_start = EmbassyInstant::now();
        Self {
            raw_start,
            logical_start: InstantMillis(raw_start.as_millis()),
        }
    }

    pub fn start_at(logical_start: InstantMillis) -> Self {
        Self {
            raw_start: EmbassyInstant::now(),
            logical_start,
        }
    }

    pub fn now(&self) -> InstantMillis {
        let elapsed = EmbassyInstant::now()
            .saturating_duration_since(self.raw_start)
            .as_millis();
        InstantMillis(self.logical_start.0.saturating_add(elapsed))
    }
}
