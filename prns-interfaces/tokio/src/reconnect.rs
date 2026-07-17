use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectDelay(Duration);

impl ReconnectDelay {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}
