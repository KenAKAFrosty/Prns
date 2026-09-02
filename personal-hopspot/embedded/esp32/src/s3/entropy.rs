use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::once_lock::OnceLock;
use esp_hal::peripherals::{ADC1, RNG};
use esp_hal::rng::{Trng, TrngError, TrngSource};
use personal_hopspot_core::HopspotS3FlashLayout;
use personal_rns::runtime::{EntropyHandle, SharedRuntimeEntropy};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use static_cell::StaticCell;

pub(crate) struct S3EntropySource;

impl EntropySource for S3EntropySource {
    type Error = TrngError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        let trng = Trng::try_new()?;
        trng.read(output);
        Ok(())
    }
}

pub(crate) type S3RuntimeEntropy = RuntimeEntropy<S3EntropySource>;
pub(crate) type S3EntropyHandle = EntropyHandle<CriticalSectionRawMutex, S3EntropySource>;
type SharedEntropy = SharedRuntimeEntropy<CriticalSectionRawMutex, S3EntropySource>;

static SHARED_ENTROPY: StaticCell<SharedEntropy> = StaticCell::new();
static ENTROPY_SERVICE: OnceLock<&'static SharedEntropy> = OnceLock::new();

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
    let entropy = RuntimeEntropy::try_new(S3EntropySource)
        .expect("the enabled S3 boot TRNG fills the initial seed");
    let (identities, entropy) =
        crate::identity::bootstrap_s3_identities(flash_layout.into(), entropy).await;

    drop(trng_source);

    S3RuntimeBootstrap {
        identities,
        entropy,
    }
}

pub(crate) fn install(mut entropy: S3RuntimeEntropy) {
    match entropy.try_reseed() {
        Ok(()) => log::info!("runtime entropy reseeded from the active radio source"),
        Err(error) => log::warn!("runtime entropy radio reseed deferred: {error:?}"),
    }

    let service: &'static SharedEntropy = SHARED_ENTROPY.init(SharedEntropy::new(entropy));
    assert!(
        ENTROPY_SERVICE.init(service).is_ok(),
        "runtime entropy service is installed exactly once"
    );
}

#[expect(
    clippy::expect_used,
    reason = "runtime entropy is installed before any engine or interface consumer starts"
)]
pub(crate) fn runtime_entropy() -> S3EntropyHandle {
    ENTROPY_SERVICE
        .try_get()
        .expect("runtime entropy is installed before PRNS consumers")
        .handle()
}

#[expect(
    clippy::expect_used,
    reason = "runtime entropy is installed during Wi-Fi initialization before BLE initialization"
)]
pub(crate) fn reseed_after_radio_start() {
    match ENTROPY_SERVICE
        .try_get()
        .expect("runtime entropy is installed before BLE initialization")
        .try_reseed()
    {
        Ok(()) => log::info!("runtime entropy reseeded after radio transition"),
        Err(error) => log::warn!("runtime entropy radio reseed deferred: {error:?}"),
    }
}
