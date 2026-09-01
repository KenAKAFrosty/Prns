use esp_hal::peripherals::{ADC1, RNG};
use esp_hal::rng::{Trng, TrngSource};
use personal_hopspot_core::HopspotS3FlashLayout;
use prns_core::entropy::{EntropySource, RuntimeEntropy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum S3EntropyError {
    RuntimeSourcePending,
}

pub(crate) enum S3EntropySource {
    BootTrng(Trng),
    PendingRadio,
}

impl EntropySource for S3EntropySource {
    type Error = S3EntropyError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        match self {
            Self::BootTrng(trng) => {
                trng.read(output);
                Ok(())
            }
            Self::PendingRadio => Err(S3EntropyError::RuntimeSourcePending),
        }
    }
}

pub(crate) type S3RuntimeEntropy = RuntimeEntropy<S3EntropySource>;

pub(crate) struct S3RuntimeBootstrap {
    pub(super) identities: crate::identity::S3IdentityBootstraps,
    pub(super) entropy: S3RuntimeEntropy,
}

pub(crate) async fn bootstrap_s3_runtime(
    rng: &mut RNG<'static>,
    adc: &mut ADC1<'static>,
    flash_layout: HopspotS3FlashLayout,
) -> S3RuntimeBootstrap {
    let trng_source = TrngSource::new(rng.reborrow(), adc.reborrow());
    let trng = Trng::try_new().expect("the S3 boot TRNG source was just enabled");
    let entropy = RuntimeEntropy::try_new(S3EntropySource::BootTrng(trng))
        .expect("the enabled S3 boot TRNG fills the initial seed");
    let (identities, entropy) =
        crate::identity::bootstrap_s3_identities(flash_layout.into(), entropy).await;

    let entropy = entropy.with_source(S3EntropySource::PendingRadio);
    drop(trng_source);

    S3RuntimeBootstrap {
        identities,
        entropy,
    }
}
