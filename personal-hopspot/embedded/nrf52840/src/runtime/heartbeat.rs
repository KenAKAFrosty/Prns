use embassy_time::Duration;

const CYCLE_MILLIS: u64 = 15_000;

pub(crate) const NORMAL: HeartbeatTiming = HeartbeatTiming::with_illuminated_millis(100);

pub(crate) struct HeartbeatTiming {
    illuminated_millis: u64,
    dark_millis: u64,
}

impl HeartbeatTiming {
    pub(crate) const fn with_illuminated_millis(illuminated_millis: u64) -> Self {
        assert!(illuminated_millis > 0);
        assert!(illuminated_millis < CYCLE_MILLIS);
        Self {
            illuminated_millis,
            dark_millis: CYCLE_MILLIS - illuminated_millis,
        }
    }

    pub(crate) const fn illuminated(&self) -> Duration {
        Duration::from_millis(self.illuminated_millis)
    }

    pub(crate) const fn dark(&self) -> Duration {
        Duration::from_millis(self.dark_millis)
    }
}
