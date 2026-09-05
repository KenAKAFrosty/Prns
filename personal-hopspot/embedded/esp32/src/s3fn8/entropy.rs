use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::once_lock::OnceLock;
use esp_hal::rng::{Trng, TrngError, TrngSource};
use personal_rns::runtime::{EntropyHandle, SharedRuntimeEntropy};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use static_cell::StaticCell;

pub(super) struct S3Fn8EntropySource;

impl EntropySource for S3Fn8EntropySource {
    type Error = TrngError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        let trng = Trng::try_new()?;
        trng.read(output);
        Ok(())
    }
}

pub(super) type S3Fn8RuntimeEntropy = RuntimeEntropy<S3Fn8EntropySource>;
pub(super) type S3Fn8EntropyHandle = EntropyHandle<CriticalSectionRawMutex, S3Fn8EntropySource>;
type SharedEntropy = SharedRuntimeEntropy<CriticalSectionRawMutex, S3Fn8EntropySource>;

static SHARED_ENTROPY: StaticCell<SharedEntropy> = StaticCell::new();
static ENTROPY_SERVICE: OnceLock<&'static SharedEntropy> = OnceLock::new();

pub(super) fn seed_runtime_entropy(
    _active_source: &TrngSource<'_>,
) -> Result<S3Fn8RuntimeEntropy, TrngError> {
    RuntimeEntropy::try_new(S3Fn8EntropySource)
}

pub(super) fn install(mut entropy: S3Fn8RuntimeEntropy) {
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

pub(super) fn runtime_entropy() -> S3Fn8EntropyHandle {
    match ENTROPY_SERVICE.try_get() {
        Some(service) => service.handle(),
        None => panic!("runtime entropy is installed before PRNS consumers"),
    }
}

pub(super) fn reseed_after_radio_start() {
    let service = match ENTROPY_SERVICE.try_get() {
        Some(service) => service,
        None => panic!("runtime entropy is installed before Bluetooth initialization"),
    };
    match service.try_reseed() {
        Ok(()) => log::info!("runtime entropy reseeded after radio transition"),
        Err(error) => log::warn!("runtime entropy radio reseed deferred: {error:?}"),
    }
}
