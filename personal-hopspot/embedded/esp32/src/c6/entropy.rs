use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::once_lock::OnceLock;
use esp_hal::rng::{Trng, TrngError, TrngSource};
use personal_rns::runtime::{EntropyHandle, SharedRuntimeEntropy};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use static_cell::StaticCell;

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
pub(super) type C6EntropyHandle = EntropyHandle<CriticalSectionRawMutex, C6EntropySource>;
type SharedEntropy = SharedRuntimeEntropy<CriticalSectionRawMutex, C6EntropySource>;

static SHARED_ENTROPY: StaticCell<SharedEntropy> = StaticCell::new();
static ENTROPY_SERVICE: OnceLock<&'static SharedEntropy> = OnceLock::new();

pub(super) fn seed_runtime_entropy(
    _active_source: &TrngSource<'_>,
) -> Result<C6RuntimeEntropy, TrngError> {
    RuntimeEntropy::try_new(C6EntropySource)
}

pub(super) fn install(mut entropy: C6RuntimeEntropy) {
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
pub(super) fn runtime_entropy() -> C6EntropyHandle {
    ENTROPY_SERVICE
        .try_get()
        .expect("runtime entropy is installed before PRNS consumers")
        .handle()
}

#[expect(
    clippy::expect_used,
    reason = "runtime entropy is installed during ESP-NOW initialization before BLE initialization"
)]
pub(super) fn reseed_after_radio_start() {
    match ENTROPY_SERVICE
        .try_get()
        .expect("runtime entropy is installed before BLE initialization")
        .try_reseed()
    {
        Ok(()) => log::info!("runtime entropy reseeded after radio transition"),
        Err(error) => log::warn!("runtime entropy radio reseed deferred: {error:?}"),
    }
}
