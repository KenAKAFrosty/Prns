//! Caller-supplied time for durable requests and network scheduling.
use prns_groups_service::{ClockInterval, ClockReading, Timestamp};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now(&self) -> ClockReading;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> ClockReading {
        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return ClockReading::Unknown;
        };
        match ClockInterval::new(Timestamp(now.as_secs()), Timestamp(now.as_secs())) {
            Ok(interval) => ClockReading::Trusted(interval),
            Err(_) => ClockReading::Unknown,
        }
    }
}
