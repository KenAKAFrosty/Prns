use std::{cell::RefCell, time::Duration};

use chacha20::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};
use tokio::time::Instant;
use zeroize::Zeroize;

use crate::engine::InstantMillis;
use crate::manifold::Host;

const MAX_TIMER_ARM_MILLIS: u64 = 24 * 60 * 60 * 1_000;

pub(super) fn bounded_timer_deadline(
    now: Instant,
    logical_now: InstantMillis,
    at: InstantMillis,
) -> Instant {
    let delay = at.0.saturating_sub(logical_now.0).min(MAX_TIMER_ARM_MILLIS);
    now.checked_add(Duration::from_millis(delay)).unwrap_or(now)
}

pub struct TokioHost {
    base: Instant,
    logical_start: InstantMillis,
    entropy: Option<ReseedingCsprng>,
}

const CSPRNG_SEED_LEN: usize = 32;
// Periodic OS reseeding bounds the amount of output exposed by any one state.
const CSPRNG_RESEED_BYTES: usize = 64 * 1_024;

/// Random stream seeded from the operating system CSPRNG.
///
/// The manifold keeps one stream on its host; cloneable runtime handles use one
/// per thread. Neither route shares mutable generator state between threads.
/// The `chacha20` zeroize feature erases old state on reseed and drop.
struct ReseedingCsprng {
    rng: ChaCha20Rng,
    generated: usize,
    process_id: u32,
}

impl ReseedingCsprng {
    fn seeded() -> Self {
        let mut seed = [0u8; CSPRNG_SEED_LEN];
        fill_os_entropy(&mut seed);
        let rng = ChaCha20Rng::from_seed(seed);
        seed.zeroize();
        Self {
            rng,
            generated: 0,
            process_id: std::process::id(),
        }
    }

    fn fill(&mut self, mut output: &mut [u8]) {
        let process_id = std::process::id();
        while !output.is_empty() {
            // A fork must never let parent and child continue the same stream.
            if self.process_id != process_id || self.generated == CSPRNG_RESEED_BYTES {
                *self = Self::seeded();
            }
            let take = output
                .len()
                .min(CSPRNG_RESEED_BYTES.saturating_sub(self.generated));
            let (filled, remainder) = output.split_at_mut(take);
            self.rng.fill_bytes(filled);
            self.generated += take;
            output = remainder;
        }
    }
}

#[allow(clippy::expect_used)]
fn fill_os_entropy(bytes: &mut [u8]) {
    getrandom::getrandom(bytes).expect("OS CSPRNG must provide runtime entropy");
}

std::thread_local! {
    static THREAD_CSPRNG: RefCell<Option<ReseedingCsprng>> = const { RefCell::new(None) };
}

impl Clone for TokioHost {
    fn clone(&self) -> Self {
        Self {
            base: self.base,
            logical_start: self.logical_start,
            // A clock clone lazily seeds its own stream; generator state is never duplicated.
            entropy: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TokioEntropy;

impl TokioEntropy {
    pub(crate) fn fill(self, bytes: &mut [u8]) {
        if bytes.is_empty() {
            return;
        }
        THREAD_CSPRNG.with(|stream| {
            stream
                .borrow_mut()
                .get_or_insert_with(ReseedingCsprng::seeded)
                .fill(bytes);
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
        Self {
            base: Instant::now(),
            logical_start,
            entropy: None,
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

    fn fill_entropy(&mut self, bytes: &mut [u8]) {
        if bytes.is_empty() {
            return;
        }
        self.entropy
            .get_or_insert_with(ReseedingCsprng::seeded)
            .fill(bytes);
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

    #[test]
    fn cloning_a_host_never_clones_its_random_stream() {
        let mut host = TokioHost::new();
        host.fill_entropy(&mut [0u8; 1]);
        assert!(host.entropy.is_some());

        assert!(host.clone().entropy.is_none());
    }

    #[test]
    fn empty_entropy_requests_do_not_seed_a_host() {
        let mut host = TokioHost::new();
        host.fill_entropy(&mut []);

        assert!(host.entropy.is_none());
    }

    #[test]
    fn thread_entropy_seeds_lazily_and_ignores_empty_requests() {
        std::thread::spawn(|| {
            THREAD_CSPRNG.with(|stream| assert!(stream.borrow().is_none()));
            TokioEntropy.fill(&mut []);
            THREAD_CSPRNG.with(|stream| assert!(stream.borrow().is_none()));

            TokioEntropy.fill(&mut [0u8; 1]);
            THREAD_CSPRNG.with(|stream| assert!(stream.borrow().is_some()));
        })
        .join()
        .expect("entropy test thread completes");
    }

    #[test]
    fn host_random_stream_reseeds_at_its_byte_limit() {
        let mut stream = ReseedingCsprng::seeded();
        let mut first_epoch = vec![0u8; CSPRNG_RESEED_BYTES];
        stream.fill(&mut first_epoch);
        assert_eq!(stream.generated, CSPRNG_RESEED_BYTES);

        stream.fill(&mut [0u8; 17]);
        assert_eq!(stream.generated, 17);
        assert_eq!(stream.process_id, std::process::id());
    }

    #[test]
    fn host_random_stream_reseeds_after_a_process_change() {
        let mut stream = ReseedingCsprng::seeded();
        stream.process_id = stream.process_id.wrapping_add(1);

        stream.fill(&mut [0u8; 17]);
        assert_eq!(stream.generated, 17);
        assert_eq!(stream.process_id, std::process::id());
    }
}
