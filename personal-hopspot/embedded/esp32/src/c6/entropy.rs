use esp_hal::rng::{Trng, TrngError, TrngSource};
use prns_core::entropy::{EntropySource, RuntimeEntropy};

pub(super) struct C6EntropySource;

impl EntropySource for C6EntropySource {
    type Error = TrngError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        let trng = Trng::try_new()?;
        trng.read(output);
        Ok(())
    }
}

pub(super) type C6RuntimeEntropy = RuntimeEntropy<C6EntropySource>;

pub(super) fn seed_runtime_entropy(
    _active_source: &TrngSource<'_>,
) -> Result<C6RuntimeEntropy, TrngError> {
    RuntimeEntropy::try_new(C6EntropySource)
}
