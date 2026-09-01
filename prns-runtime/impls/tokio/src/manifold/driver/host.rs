use std::{cell::RefCell, time::Duration};

use tokio::time::Instant;

use crate::engine::InstantMillis;
use crate::manifold::Host;
use crate::runtime::OsRuntimeEntropy;

const MAX_TIMER_ARM_MILLIS: u64 = 24 * 60 * 60 * 1_000;

/// A manifold-local millisecond clock carried through one complete scheduling step.
///
/// The selected branch samples authoritative monotonic time once and every operation in its batch
/// shares that value. The following scheduling decision reuses it too, eliminating redundant
/// top-of-loop reads without adding a ticker, wake, shared state, or competing select branch.
pub(super) struct ManifoldClock {
    logical_now: InstantMillis,
}

impl ManifoldClock {
    pub(super) fn new<H: Host>(host: &H) -> Self {
        Self {
            logical_now: host.now(),
        }
    }

    pub(super) fn now(&self) -> InstantMillis {
        self.logical_now
    }

    pub(super) fn immediate_deadline(&self) -> Instant {
        Instant::now()
    }

    pub(super) fn timer_deadline(&self, at: InstantMillis) -> Instant {
        let delay =
            at.0.saturating_sub(self.logical_now.0)
                .min(MAX_TIMER_ARM_MILLIS);
        let raw_now = Instant::now();
        raw_now
            .checked_add(Duration::from_millis(delay))
            .unwrap_or(raw_now)
    }

    pub(super) fn observe_step<H: Host>(&mut self, host: &H) -> InstantMillis {
        self.reconcile(host);
        self.logical_now
    }

    fn reconcile<H: Host>(&mut self, host: &H) {
        self.logical_now = self.logical_now.max(host.now());
    }
}

/// Cloneable Tokio-backed logical time without access to runtime randomness.
#[derive(Clone)]
pub struct TokioClock {
    base: Instant,
    logical_start: InstantMillis,
}

impl TokioClock {
    #[must_use]
    pub fn new() -> Self {
        Self::start_at(InstantMillis(0))
    }

    #[must_use]
    pub fn start_at(logical_start: InstantMillis) -> Self {
        Self {
            base: Instant::now(),
            logical_start,
        }
    }

    #[must_use]
    pub fn now(&self) -> InstantMillis {
        let elapsed = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        InstantMillis(self.logical_start.0.saturating_add(elapsed))
    }

    pub async fn sleep_until(&self, deadline: InstantMillis) {
        loop {
            let remaining = deadline.0.saturating_sub(self.now().0);
            if remaining == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(remaining.min(MAX_TIMER_ARM_MILLIS))).await;
        }
    }

    fn with_entropy(self, entropy: OsRuntimeEntropy) -> TokioHost {
        TokioHost {
            clock: self,
            entropy,
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokio-backed host services for one manifold.
///
/// A child created by a raw Unix `fork` must execute a fresh program image before using Prns.
/// Continuing within the inherited process is unsupported because it can duplicate cryptographic
/// random-generator state. Ordinary spawn-and-execute process creation remains supported.
///
/// The entropy-owning host deliberately cannot be cloned. Use [`Self::clock`] when another task
/// needs a cloneable logical-time view.
///
/// ```compile_fail
/// use prns_runtime_tokio::manifold::driver::TokioHost;
///
/// let host = TokioHost::new();
/// let duplicated = host.clone();
/// ```
pub struct TokioHost {
    clock: TokioClock,
    entropy: OsRuntimeEntropy,
}

#[expect(
    clippy::expect_used,
    reason = "a host without a functioning OS CSPRNG must not emit runtime randomness"
)]
fn seeded_runtime_entropy() -> OsRuntimeEntropy {
    OsRuntimeEntropy::try_new().expect("OS CSPRNG must provide the initial runtime seed")
}

std::thread_local! {
    static THREAD_ENTROPY: RefCell<Option<OsRuntimeEntropy>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub(crate) struct TokioEntropy;

impl TokioEntropy {
    pub(crate) fn fill(self, bytes: &mut [u8]) {
        if bytes.is_empty() {
            return;
        }
        THREAD_ENTROPY.with(|stream| {
            stream
                .borrow_mut()
                .get_or_insert_with(seeded_runtime_entropy)
                .fill_random(bytes);
        });
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
        TokioClock::start_at(logical_start).with_entropy(seeded_runtime_entropy())
    }

    #[must_use]
    pub fn clock(&self) -> TokioClock {
        self.clock.clone()
    }

    pub(crate) fn set_timeline_origin(&mut self, logical_start: InstantMillis) {
        self.clock = TokioClock::start_at(logical_start);
    }
}

impl Default for TokioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for TokioHost {
    fn now(&self) -> InstantMillis {
        self.clock.now()
    }

    async fn sleep_until(&self, deadline: InstantMillis) {
        self.clock.sleep_until(deadline).await;
    }

    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.entropy.fill_random(bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{Arc, Barrier},
    };

    use prns_core::entropy::{EntropySource, RuntimeEntropy};

    use super::*;

    const CORE_RESEED_INTERVAL_BYTES: usize = 64 * 1_024;
    struct TestEntropySource {
        calls: Rc<Cell<usize>>,
    }

    impl EntropySource for TestEntropySource {
        type Error = core::convert::Infallible;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            output.fill(u8::try_from(call).unwrap_or(u8::MAX));
            Ok(())
        }
    }

    fn test_runtime_entropy() -> (RuntimeEntropy<TestEntropySource>, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        let source = TestEntropySource {
            calls: Rc::clone(&calls),
        };
        let entropy =
            RuntimeEntropy::try_new(source).expect("the scripted initial entropy fill succeeds");
        (entropy, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn manifold_clock_carries_time_until_the_next_step() {
        let host = TokioHost::new();
        let mut clock = ManifoldClock::new(&host);

        let carried = clock.now();
        tokio::time::advance(Duration::from_millis(7)).await;
        assert_eq!(clock.now(), carried);

        clock.observe_step(&host);
        assert_eq!(clock.now(), InstantMillis(carried.0 + 7));
    }

    #[tokio::test(start_paused = true)]
    async fn manifold_clock_maps_absolute_deadlines_and_bounds_far_future_arms() {
        let host = TokioHost::start_at(InstantMillis(100));
        let clock = ManifoldClock::new(&host);
        let raw_now = Instant::now();

        assert_eq!(
            clock.timer_deadline(InstantMillis(105)),
            raw_now + Duration::from_millis(5)
        );
        assert_eq!(
            clock.timer_deadline(InstantMillis(u64::MAX)),
            raw_now + Duration::from_millis(MAX_TIMER_ARM_MILLIS)
        );
    }

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

    #[test]
    fn clock_views_clone_without_cloning_the_host() {
        let host = TokioHost::start_at(InstantMillis(17));
        let clock = host.clock();
        let cloned = clock.clone();

        assert_eq!(clock.logical_start, cloned.logical_start);
        assert_eq!(clock.base, cloned.base);
    }

    #[test]
    fn a_host_always_owns_healthy_runtime_entropy() {
        let mut host = TokioHost::new();
        host.fill_random(&mut []);

        assert_eq!(
            host.entropy.reseed_health(),
            prns_core::entropy::ReseedHealth::Healthy
        );
    }

    #[test]
    fn thread_entropy_is_isolated_seeds_lazily_and_ignores_empty_requests() {
        let first_seeded = Arc::new(Barrier::new(2));
        let release_first = Arc::new(Barrier::new(2));
        let first_thread = std::thread::spawn({
            let first_seeded = Arc::clone(&first_seeded);
            let release_first = Arc::clone(&release_first);
            move || {
                THREAD_ENTROPY.with(|stream| assert!(stream.borrow().is_none()));
                TokioEntropy.fill(&mut [0u8; 1]);
                THREAD_ENTROPY.with(|stream| assert!(stream.borrow().is_some()));
                first_seeded.wait();
                release_first.wait();
            }
        });

        first_seeded.wait();
        std::thread::spawn(|| {
            THREAD_ENTROPY.with(|stream| assert!(stream.borrow().is_none()));
            TokioEntropy.fill(&mut []);
            THREAD_ENTROPY.with(|stream| assert!(stream.borrow().is_none()));

            TokioEntropy.fill(&mut [0u8; 1]);
            THREAD_ENTROPY.with(|stream| assert!(stream.borrow().is_some()));
        })
        .join()
        .expect("second entropy test thread completes");
        release_first.wait();
        first_thread
            .join()
            .expect("first entropy test thread completes");
    }

    #[test]
    fn core_runtime_entropy_reseeds_at_its_byte_limit() {
        let (mut entropy, calls) = test_runtime_entropy();
        let mut first_window = vec![0u8; CORE_RESEED_INTERVAL_BYTES];

        entropy.fill_random(&mut first_window);
        assert_eq!(calls.get(), 1);

        entropy.fill_random(&mut [0u8; 17]);
        assert_eq!(calls.get(), 2);
        assert_eq!(
            entropy.reseed_health(),
            prns_core::entropy::ReseedHealth::Healthy
        );
    }
}
