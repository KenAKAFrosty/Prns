//! Core-owned cryptographically secure runtime randomness.
//!
//! A platform supplies genuine seed material at this module's single host boundary. After
//! construction, consumers draw fast random bytes from [`RuntimeEntropy::fill_random`] instead of
//! calling the hardware or operating-system source directly.

use chacha20::ChaCha20Rng;
use hkdf::Hkdf;
use rand_core::{Rng, SeedableRng};
use sha2::Sha256;
use zeroize::Zeroizing;

const SEED_LEN: usize = 32;
const RESEED_INTERVAL_BYTES: usize = 64 * 1_024;
const RESEED_INFO: &[u8] = b"personal-rns/csprng/reseed/v1";

/// A platform source of genuine cryptographic entropy.
///
/// An implementation must either overwrite the complete `output` slice with independently
/// sourced, cryptographically secure bytes or return an error. This is the key place where every platform
/// must do the careful, deliberate work of ensuring the quality of randomness provided.
/// The core cannot measure or prove the physical quality of those bytes.
///
/// Ordinary protocol and application code should not retain or call an entropy source directly.
/// It should construct [`RuntimeEntropy`] and use [`RuntimeEntropy::fill_random`] instead.
pub trait EntropySource {
    type Error;

    /// Completely fills `output` with fresh cryptographic entropy.
    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error>;
}

impl<F, E> EntropySource for F
where
    F: FnMut(&mut [u8]) -> Result<(), E>,
{
    type Error = E;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        self(output)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReseedHealth {
    Healthy,
    /// A reseed source failed; the previously secure CSPRNG stream remains in service.
    Deferred,
}

/// A continuous, core-owned cryptographically secure random stream.
///
/// The generator can only be created by successfully filling its initial seed through an
/// [`EntropySource`]. It owns that source for explicit and periodic reseeding. The secret-bearing
/// generator deliberately implements neither `Clone` nor `Debug` and does not expose its state.
pub struct RuntimeEntropy<S> {
    csprng: ChaCha20Rng,
    source: S,
    bytes_until_reseed_attempt: usize,
    reseed_health: ReseedHealth,
}

impl<S: EntropySource> RuntimeEntropy<S> {
    /// Seeds a new runtime generator from the supplied platform source.
    ///
    /// An all-zero seed is not specially rejected: construction trusts the source's success
    /// contract and does not pretend to perform statistical entropy validation.
    pub fn try_new(mut source: S) -> Result<Self, S::Error> {
        let mut seed = Zeroizing::new([0_u8; SEED_LEN]);
        source.try_fill_entropy(&mut seed[..])?;

        Ok(Self {
            csprng: ChaCha20Rng::from_seed(*seed),
            source,
            bytes_until_reseed_attempt: RESEED_INTERVAL_BYTES,
            reseed_health: ReseedHealth::Healthy,
        })
    }

    pub fn fill_random(&mut self, output: &mut [u8]) {
        let mut filled_len = 0;

        while filled_len < output.len() {
            if self.bytes_until_reseed_attempt == 0 && self.try_reseed().is_err() {
                // The stream remains secure from its prior seed. Open another full output window
                // before retrying so a failed hardware source is not hammered on every call.
                self.bytes_until_reseed_attempt = RESEED_INTERVAL_BYTES;
            }

            let remaining_len = output.len() - filled_len;
            let chunk_len = remaining_len.min(self.bytes_until_reseed_attempt);
            let chunk_end = filled_len + chunk_len;
            self.csprng.fill_bytes(&mut output[filled_len..chunk_end]);
            self.bytes_until_reseed_attempt -= chunk_len;
            filled_len = chunk_end;
        }
    }

    pub fn try_reseed(&mut self) -> Result<(), S::Error> {
        let mut fresh = Zeroizing::new([0_u8; SEED_LEN]);
        if let Err(error) = self.source.try_fill_entropy(&mut fresh[..]) {
            self.reseed_health = ReseedHealth::Deferred;
            return Err(error);
        }

        let mut continuity = Zeroizing::new([0_u8; SEED_LEN]);
        self.csprng.fill_bytes(&mut continuity[..]);
        let new_seed = derive_reseed_seed(&fresh, &continuity);

        self.csprng = ChaCha20Rng::from_seed(*new_seed);
        self.bytes_until_reseed_attempt = RESEED_INTERVAL_BYTES;
        self.reseed_health = ReseedHealth::Healthy;
        Ok(())
    }

    #[must_use]
    pub const fn reseed_health(&self) -> ReseedHealth {
        self.reseed_health
    }

    /// Moves this continuous stream to a different platform entropy source.
    ///
    /// This consumes the old generator so its secret state cannot be cloned during a boot-time
    /// transition, while preserving its stream position and reseed health. Installing a source
    /// does not itself claim that source is live; call [`Self::try_reseed`] when the transition
    /// requires fresh entropy immediately.
    #[must_use]
    pub fn with_source<T: EntropySource>(self, source: T) -> RuntimeEntropy<T> {
        let Self {
            csprng,
            source: old_source,
            bytes_until_reseed_attempt,
            reseed_health,
        } = self;
        drop(old_source);

        RuntimeEntropy {
            csprng,
            source,
            bytes_until_reseed_attempt,
            reseed_health,
        }
    }
}

#[allow(clippy::expect_used)]
fn derive_reseed_seed(
    fresh: &[u8; SEED_LEN],
    continuity: &[u8; SEED_LEN],
) -> Zeroizing<[u8; SEED_LEN]> {
    // RFC 5869 extract: salt = fresh platform bytes, IKM = hidden stream continuity bytes.
    let (prk, hkdf) = Hkdf::<Sha256>::extract(Some(fresh), continuity);
    let _prk = Zeroizing::new(prk);

    let mut new_seed = Zeroizing::new([0_u8; SEED_LEN]);
    hkdf.expand(RESEED_INFO, &mut new_seed[..])
        .expect("a 32-byte HKDF-SHA256 expansion is always valid");
    new_seed
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use core::fmt::Debug;

    use rand_core::{Rng, SeedableRng};
    use serde::{Deserialize, Serialize};
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestSourceError {
        Unavailable,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct ScriptedSource {
        outputs: [[u8; SEED_LEN]; 3],
        calls: usize,
        fail_on_call: Option<usize>,
    }

    impl ScriptedSource {
        fn new(outputs: [[u8; SEED_LEN]; 3]) -> Self {
            Self {
                outputs,
                calls: 0,
                fail_on_call: None,
            }
        }

        fn failing_on(mut self, call: usize) -> Self {
            self.fail_on_call = Some(call);
            self
        }
    }

    impl EntropySource for ScriptedSource {
        type Error = TestSourceError;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            self.calls += 1;
            if self.fail_on_call == Some(self.calls) {
                return Err(TestSourceError::Unavailable);
            }

            let scripted = self.outputs[(self.calls - 1).min(self.outputs.len() - 1)];
            for (index, byte) in output.iter_mut().enumerate() {
                *byte = scripted[index % scripted.len()];
            }
            Ok(())
        }
    }

    assert_not_impl_any!(
        RuntimeEntropy<ScriptedSource>:
            Clone,
            Debug,
            Serialize,
            serde::de::DeserializeOwned
    );

    #[test]
    fn fallible_host_fill_function_is_accepted_only_at_construction() {
        let mut calls = 0;
        let source = |output: &mut [u8]| {
            calls += 1;
            output.fill(0x0f);
            Ok::<(), core::convert::Infallible>(())
        };
        let mut entropy = match RuntimeEntropy::try_new(source) {
            Ok(entropy) => entropy,
            Err(never) => match never {},
        };

        entropy.fill_random(&mut [0_u8; 17]);
        drop(entropy);

        assert_eq!(calls, 1);
    }

    #[test]
    fn initial_source_failure_constructs_no_generator() {
        let source = ScriptedSource::new([[0x11; SEED_LEN]; 3]).failing_on(1);

        assert!(matches!(
            RuntimeEntropy::try_new(source),
            Err(TestSourceError::Unavailable)
        ));
    }

    #[test]
    fn all_zero_initial_seed_is_accepted_under_the_source_contract() {
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([[0; SEED_LEN]; 3]))
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));

        entropy.fill_random(&mut [0_u8; 1]);

        assert_eq!(entropy.source.calls, 1);
        assert_eq!(entropy.reseed_health(), ReseedHealth::Healthy);
    }

    #[test]
    fn initial_seed_produces_the_expected_chacha20_stream() {
        let seed = [0x21; SEED_LEN];
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([seed; 3]))
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut actual = [0_u8; 96];
        entropy.fill_random(&mut actual);

        let mut reference = ChaCha20Rng::from_seed(seed);
        let mut expected = [0_u8; 96];
        reference.fill_bytes(&mut expected);

        assert_eq!(actual, expected);
        assert_eq!(entropy.source.calls, 1);
        assert_eq!(entropy.reseed_health(), ReseedHealth::Healthy);
    }

    #[test]
    fn empty_fills_neither_advance_nor_reseed() {
        let seed = [0x31; SEED_LEN];
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([seed; 3]))
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));

        entropy.fill_random(&mut []);
        assert_eq!(entropy.source.calls, 1);

        let mut actual = [0_u8; 16];
        entropy.fill_random(&mut actual);
        let mut reference = ChaCha20Rng::from_seed(seed);
        let mut expected = [0_u8; 16];
        reference.fill_bytes(&mut expected);
        assert_eq!(actual, expected);
    }

    #[test]
    fn exact_boundary_reseeds_only_before_more_output() {
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([
            [0x41; SEED_LEN],
            [0x42; SEED_LEN],
            [0x43; SEED_LEN],
        ]))
        .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut first_window = [0_u8; RESEED_INTERVAL_BYTES];

        entropy.fill_random(&mut first_window);
        assert_eq!(entropy.source.calls, 1);

        entropy.fill_random(&mut [0_u8; 1]);
        assert_eq!(entropy.source.calls, 2);
        assert_eq!(
            entropy.bytes_until_reseed_attempt,
            RESEED_INTERVAL_BYTES - 1
        );
    }

    #[test]
    fn one_large_fill_reseeds_at_each_crossed_window() {
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([
            [0x51; SEED_LEN],
            [0x52; SEED_LEN],
            [0x53; SEED_LEN],
        ]))
        .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut output = [0_u8; RESEED_INTERVAL_BYTES * 2 + 17];

        entropy.fill_random(&mut output);

        assert_eq!(entropy.source.calls, 3);
        assert_eq!(
            entropy.bytes_until_reseed_attempt,
            RESEED_INTERVAL_BYTES - 17
        );
        assert_eq!(entropy.reseed_health(), ReseedHealth::Healthy);
    }

    #[test]
    fn failed_scheduled_reseed_preserves_the_continuous_stream() {
        let seed = [0x61; SEED_LEN];
        let source = ScriptedSource::new([seed; 3]).failing_on(2);
        let mut entropy = RuntimeEntropy::try_new(source)
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut actual = [0_u8; RESEED_INTERVAL_BYTES + 32];

        entropy.fill_random(&mut actual);

        let mut reference = ChaCha20Rng::from_seed(seed);
        let mut expected = [0_u8; RESEED_INTERVAL_BYTES + 32];
        reference.fill_bytes(&mut expected);
        assert_eq!(actual, expected);
        assert_eq!(entropy.source.calls, 2);
        assert_eq!(
            entropy.bytes_until_reseed_attempt,
            RESEED_INTERVAL_BYTES - 32
        );
        assert_eq!(entropy.reseed_health(), ReseedHealth::Deferred);
    }

    #[test]
    fn scheduled_failure_waits_another_full_window_before_retrying() {
        let source = ScriptedSource::new([[0x71; SEED_LEN]; 3]).failing_on(2);
        let mut entropy = RuntimeEntropy::try_new(source)
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut first_retry_window = [0_u8; RESEED_INTERVAL_BYTES * 2];

        entropy.fill_random(&mut first_retry_window);
        assert_eq!(entropy.source.calls, 2);
        assert_eq!(entropy.reseed_health(), ReseedHealth::Deferred);

        entropy.fill_random(&mut [0_u8; 1]);
        assert_eq!(entropy.source.calls, 3);
        assert_eq!(entropy.reseed_health(), ReseedHealth::Healthy);
    }

    #[test]
    fn mandatory_reseed_failure_does_not_advance_the_stream_or_retry_budget() {
        let seed = [0x81; SEED_LEN];
        let source = ScriptedSource::new([seed; 3]).failing_on(2);
        let mut entropy = RuntimeEntropy::try_new(source)
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut prefix = [0_u8; 19];
        entropy.fill_random(&mut prefix);

        assert_eq!(entropy.try_reseed(), Err(TestSourceError::Unavailable));
        assert_eq!(
            entropy.bytes_until_reseed_attempt,
            RESEED_INTERVAL_BYTES - prefix.len()
        );
        assert_eq!(entropy.reseed_health(), ReseedHealth::Deferred);

        let mut actual_suffix = [0_u8; 32];
        entropy.fill_random(&mut actual_suffix);
        let mut reference = ChaCha20Rng::from_seed(seed);
        reference.fill_bytes(&mut [0_u8; 19]);
        let mut expected_suffix = [0_u8; 32];
        reference.fill_bytes(&mut expected_suffix);
        assert_eq!(actual_suffix, expected_suffix);
    }

    #[test]
    fn zero_fresh_bytes_preserve_distinct_prior_streams() {
        let mut left = RuntimeEntropy::try_new(ScriptedSource::new([
            [0x91; SEED_LEN],
            [0; SEED_LEN],
            [0; SEED_LEN],
        ]))
        .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut right = RuntimeEntropy::try_new(ScriptedSource::new([
            [0x92; SEED_LEN],
            [0; SEED_LEN],
            [0; SEED_LEN],
        ]))
        .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));

        left.try_reseed()
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        right
            .try_reseed()
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        let mut left_output = [0_u8; 64];
        let mut right_output = [0_u8; 64];
        left.fill_random(&mut left_output);
        right.fill_random(&mut right_output);

        assert_ne!(left_output, right_output);
    }

    #[test]
    fn with_source_moves_the_stream_without_cloning_or_reseeding_it() {
        let seed = [0xa1; SEED_LEN];
        let mut entropy = RuntimeEntropy::try_new(ScriptedSource::new([seed; 3]))
            .unwrap_or_else(|error| panic!("unexpected source error: {error:?}"));
        entropy.fill_random(&mut [0_u8; 23]);

        let mut entropy = entropy.with_source(ScriptedSource::new([[0xa2; SEED_LEN]; 3]));
        let mut actual = [0_u8; 32];
        entropy.fill_random(&mut actual);

        let mut reference = ChaCha20Rng::from_seed(seed);
        reference.fill_bytes(&mut [0_u8; 23]);
        let mut expected = [0_u8; 32];
        reference.fill_bytes(&mut expected);
        assert_eq!(actual, expected);
        assert_eq!(entropy.source.calls, 0);
        assert_eq!(
            entropy.bytes_until_reseed_attempt,
            RESEED_INTERVAL_BYTES - (23 + 32)
        );
    }

    #[test]
    fn reseed_transcript_and_next_output_match_the_v1_golden_vector() {
        let fresh = [0xb1; SEED_LEN];
        let continuity = [0xb2; SEED_LEN];
        let seed = derive_reseed_seed(&fresh, &continuity);
        let expected_seed = [
            0x08, 0x93, 0xeb, 0x62, 0x53, 0x3a, 0x69, 0xe4, 0x4b, 0x64, 0x79, 0x7e, 0x64, 0xbb,
            0xe3, 0x8c, 0x90, 0x83, 0x7b, 0x38, 0xe3, 0x28, 0x05, 0x12, 0x39, 0x92, 0x0c, 0x94,
            0x9e, 0xd8, 0x20, 0xd9,
        ];
        assert_eq!(*seed, expected_seed);

        let mut rng = ChaCha20Rng::from_seed(*seed);
        let mut output = [0_u8; 64];
        rng.fill_bytes(&mut output);
        let expected_output = [
            0x55, 0xf5, 0xcc, 0x36, 0x4d, 0xd1, 0xe7, 0xbf, 0x08, 0xda, 0x38, 0x03, 0xf5, 0xdd,
            0x1e, 0x58, 0x2f, 0xaa, 0xf6, 0x81, 0x38, 0xb7, 0xeb, 0x8b, 0xa0, 0xa2, 0xa8, 0x0d,
            0x8a, 0xb1, 0x2c, 0x5c, 0xc6, 0x09, 0x21, 0x95, 0x51, 0x72, 0xd5, 0x71, 0x5f, 0x9b,
            0x7e, 0xc4, 0x5a, 0xe7, 0xdc, 0x4a, 0x8c, 0xa2, 0x7a, 0x66, 0xd6, 0xda, 0xb5, 0x07,
            0x70, 0x24, 0x8c, 0xe1, 0x12, 0x75, 0x6f, 0x83,
        ];
        assert_eq!(output, expected_output);
    }
}
