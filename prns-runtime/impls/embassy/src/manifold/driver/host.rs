use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Duration, Timer};
use prns_core::entropy::EntropySource;

use crate::engine::InstantMillis;
use crate::manifold::timebase::EmbassyTimebase;
use crate::manifold::Host;
use crate::runtime::EntropyHandle;

/// Embassy clock and authoritative runtime-randomness handle.
///
/// Arbitrary callbacks cannot be installed as the host's random service.
///
/// ```compile_fail
/// use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
/// use prns_runtime_embassy::manifold::driver::EmbassyHost;
///
/// let callback = |output: &mut [u8]| output.fill(0x42);
/// let _ = EmbassyHost::<CriticalSectionRawMutex, _>::new(callback);
/// ```
pub struct EmbassyHost<M, S>
where
    M: RawMutex + 'static,
    S: EntropySource + 'static,
{
    timebase: EmbassyTimebase,
    entropy: EntropyHandle<M, S>,
}

pub trait ResumableHost: Host {
    fn resume_at(&mut self, logical_start: InstantMillis);
}

impl<M, S> EmbassyHost<M, S>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    pub fn new(entropy: EntropyHandle<M, S>) -> Self {
        Self::new_with_timebase(EmbassyTimebase::capture_now(), entropy)
    }

    pub fn new_with_timebase(timebase: EmbassyTimebase, entropy: EntropyHandle<M, S>) -> Self {
        Self { timebase, entropy }
    }
}

impl<M, S> Host for EmbassyHost<M, S>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    fn now(&self) -> InstantMillis {
        self.timebase.now()
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        let remaining = deadline.0.saturating_sub(self.timebase.now().0);
        Timer::after(Duration::from_millis(remaining)).await;
    }

    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.entropy.fill_random(bytes);
    }
}

impl<M, S> ResumableHost for EmbassyHost<M, S>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    fn resume_at(&mut self, logical_start: InstantMillis) {
        self.timebase = EmbassyTimebase::start_at(logical_start);
    }
}
