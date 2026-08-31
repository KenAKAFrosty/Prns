//! Serialized access to one authoritative embedded random stream.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::RawMutex, Mutex};
use prns_core::entropy::{EntropySource, RuntimeEntropy};

/// Copyable access to one statically owned authoritative [`RuntimeEntropy`] stream.
///
/// The shared wrapper and its secret-bearing generator are not copied when this handle is copied.
pub struct EntropyHandle<M, S>
where
    M: RawMutex + 'static,
    S: EntropySource + 'static,
{
    shared: &'static SharedRuntimeEntropy<M, S>,
}

impl<M, S> Clone for EntropyHandle<M, S>
where
    M: RawMutex + 'static,
    S: EntropySource + 'static,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M, S> Copy for EntropyHandle<M, S>
where
    M: RawMutex + 'static,
    S: EntropySource + 'static,
{
}

impl<M, S> EntropyHandle<M, S>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    /// Fills `output` from the authoritative stream without exposing its generator or source.
    pub fn fill_random(self, output: &mut [u8]) {
        if output.is_empty() {
            return;
        }
        self.shared.fill_random(output);
    }
}

/// Mutex-serialized access to one authoritative [`RuntimeEntropy`].
///
/// `M` determines the execution contexts across which access is safe. Issuing an
/// [`EntropyHandle`] additionally requires the complete wrapper to be [`Sync`], preventing a
/// single-executor-only mutex from being presented as cross-context access.
pub struct SharedRuntimeEntropy<M, S>
where
    M: RawMutex,
    S: EntropySource,
{
    entropy: Mutex<M, RefCell<RuntimeEntropy<S>>>,
}

impl<M, S> SharedRuntimeEntropy<M, S>
where
    M: RawMutex,
    S: EntropySource,
{
    /// Wraps an existing continuous stream without cloning or reseeding it.
    #[must_use]
    pub fn new(entropy: RuntimeEntropy<S>) -> Self {
        Self {
            entropy: Mutex::new(RefCell::new(entropy)),
        }
    }

    /// Performs mandatory initial seeding, then wraps the resulting continuous stream.
    pub fn try_new(source: S) -> Result<Self, S::Error> {
        RuntimeEntropy::try_new(source).map(Self::new)
    }
}

impl<M, S> SharedRuntimeEntropy<M, S>
where
    M: RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    /// Borrows this statically installed stream through an opaque, copyable handle.
    #[must_use]
    pub fn handle(&'static self) -> EntropyHandle<M, S> {
        EntropyHandle { shared: self }
    }

    fn fill_random(&self, output: &mut [u8]) {
        self.entropy
            .lock(|entropy| entropy.borrow_mut().fill_random(output));
    }
}

#[cfg(test)]
mod tests {
    use core::{convert::Infallible, fmt::Debug};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    const RESEED_INTERVAL_BYTES: usize = 64 * 1_024;

    struct ConstantSource {
        byte: u8,
        calls: Arc<AtomicUsize>,
    }

    impl ConstantSource {
        fn new(byte: u8) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    byte,
                    calls: Arc::clone(&calls),
                },
                calls,
            )
        }
    }

    impl EntropySource for ConstantSource {
        type Error = Infallible;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            output.fill(self.byte);
            Ok(())
        }
    }

    struct FailingSource;

    impl EntropySource for FailingSource {
        type Error = InitialSeedUnavailable;

        fn try_fill_entropy(&mut self, _output: &mut [u8]) -> Result<(), Self::Error> {
            Err(InitialSeedUnavailable)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct InitialSeedUnavailable;

    type TestShared<S> = SharedRuntimeEntropy<CriticalSectionRawMutex, S>;
    type TestHandle<S> = EntropyHandle<CriticalSectionRawMutex, S>;

    assert_impl_all!(TestHandle<ConstantSource>: Clone, Copy, Send, Sync);
    assert_not_impl_any!(TestShared<ConstantSource>: Clone, Debug);

    #[test]
    fn initial_source_failure_constructs_no_shared_runtime() {
        assert!(matches!(
            TestShared::try_new(FailingSource),
            Err(InitialSeedUnavailable)
        ));
    }

    #[test]
    fn copied_handles_advance_one_authoritative_stream() {
        let (source, _) = ConstantSource::new(0x42);
        let shared = Box::leak(Box::new(
            TestShared::try_new(source).expect("constant source seeds the shared runtime"),
        ));
        let first_handle = shared.handle();
        let second_handle = first_handle;
        let mut first = [0_u8; 17];
        let mut second = [0_u8; 31];

        first_handle.fill_random(&mut first);
        second_handle.fill_random(&mut second);

        let (reference_source, _) = ConstantSource::new(0x42);
        let mut reference = RuntimeEntropy::try_new(reference_source)
            .expect("constant source seeds the reference stream");
        let mut expected_first = [0_u8; 17];
        let mut expected_second = [0_u8; 31];
        reference.fill_random(&mut expected_first);
        reference.fill_random(&mut expected_second);
        assert_eq!(first, expected_first);
        assert_eq!(second, expected_second);
    }

    #[test]
    fn wrapping_an_existing_stream_preserves_its_position() {
        let (source, _) = ConstantSource::new(0x4a);
        let mut entropy = RuntimeEntropy::try_new(source)
            .expect("constant source seeds the stream before installation");
        let mut prefix = [0_u8; 19];
        entropy.fill_random(&mut prefix);
        let shared = Box::leak(Box::new(TestShared::new(entropy)));
        let mut actual = [0_u8; 37];
        shared.handle().fill_random(&mut actual);

        let (reference_source, _) = ConstantSource::new(0x4a);
        let mut reference = RuntimeEntropy::try_new(reference_source)
            .expect("constant source seeds the reference stream");
        let mut expected_prefix = [0_u8; 19];
        let mut expected = [0_u8; 37];
        reference.fill_random(&mut expected_prefix);
        reference.fill_random(&mut expected);

        assert_eq!(prefix, expected_prefix);
        assert_eq!(actual, expected);
    }

    #[test]
    fn copied_handles_share_one_periodic_reseed_budget() {
        let (source, calls) = ConstantSource::new(0x53);
        let shared = Box::leak(Box::new(
            TestShared::try_new(source).expect("constant source seeds the shared runtime"),
        ));
        let first_handle = shared.handle();
        let second_handle = first_handle;
        let mut half_window = vec![0_u8; RESEED_INTERVAL_BYTES / 2];

        first_handle.fill_random(&mut half_window);
        second_handle.fill_random(&mut half_window);
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        second_handle.fill_random(&mut [0_u8; 1]);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn concurrent_handles_are_serialized() {
        let (source, _) = ConstantSource::new(0x64);
        let shared = Box::leak(Box::new(
            TestShared::try_new(source).expect("constant source seeds the shared runtime"),
        ));
        let handle = shared.handle();

        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(move || {
                    for _ in 0..64 {
                        handle.fill_random(&mut [0_u8; 32]);
                    }
                });
            }
        });
    }
}
