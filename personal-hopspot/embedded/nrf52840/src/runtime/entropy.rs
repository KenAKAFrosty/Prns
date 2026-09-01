use core::convert::Infallible;

use embassy_nrf::mode::Blocking;
use embassy_nrf::rng::Rng;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::once_lock::OnceLock;
use personal_rns::runtime::{EntropyHandle, SharedRuntimeEntropy};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use static_cell::StaticCell;

#[cfg(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
use nrf_softdevice::{RandomError, Softdevice, SoftdeviceRandom};

pub(super) struct HalEntropySource(Rng<'static, Blocking>);

impl EntropySource for HalEntropySource {
    type Error = Infallible;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        self.0.blocking_fill_bytes(output);
        Ok(())
    }
}

#[cfg(feature = "board-t1000e")]
pub(super) type NrfEntropySource = HalEntropySource;

#[cfg(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
pub(super) enum NrfEntropySource {
    PendingSoftDevice,
    SoftDevice(SoftdeviceRandom),
}

#[cfg(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
impl EntropySource for NrfEntropySource {
    type Error = RandomError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        match self {
            Self::PendingSoftDevice => Err(RandomError::NotEnoughEntropy),
            Self::SoftDevice(random) => {
                for chunk in output.chunks_mut(u8::MAX as usize) {
                    random.random_bytes(chunk)?;
                }
                Ok(())
            }
        }
    }
}

type SharedEntropy = SharedRuntimeEntropy<CriticalSectionRawMutex, NrfEntropySource>;
pub(super) type RuntimeEntropyHandle = EntropyHandle<CriticalSectionRawMutex, NrfEntropySource>;

static SHARED_ENTROPY: StaticCell<SharedEntropy> = StaticCell::new();
static ENTROPY_HANDLE: OnceLock<RuntimeEntropyHandle> = OnceLock::new();

pub(super) fn seed_from_hal(rng: Rng<'static, Blocking>) -> RuntimeEntropy<HalEntropySource> {
    match RuntimeEntropy::try_new(HalEntropySource(rng)) {
        Ok(entropy) => entropy,
        Err(never) => match never {},
    }
}

fn install(entropy: RuntimeEntropy<NrfEntropySource>) {
    let handle = SHARED_ENTROPY.init(SharedEntropy::new(entropy)).handle();
    assert!(
        ENTROPY_HANDLE.init(handle).is_ok(),
        "runtime entropy is installed exactly once"
    );
}

#[cfg(feature = "board-t1000e")]
pub(super) fn install_hal_runtime_entropy(entropy: RuntimeEntropy<NrfEntropySource>) {
    install(entropy);
}

#[cfg(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
pub(super) fn prepare_softdevice_runtime_entropy(
    entropy: RuntimeEntropy<HalEntropySource>,
) -> RuntimeEntropy<NrfEntropySource> {
    entropy.with_source(NrfEntropySource::PendingSoftDevice)
}

#[cfg(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
pub(super) fn install_softdevice_runtime_entropy(
    entropy: RuntimeEntropy<NrfEntropySource>,
    softdevice: &'static Softdevice,
) {
    let mut entropy = entropy.with_source(NrfEntropySource::SoftDevice(softdevice.random()));
    let _ = entropy.try_reseed();
    install(entropy);
}

#[expect(
    clippy::expect_used,
    reason = "runtime entropy is installed before any engine or interface consumer starts"
)]
pub(super) fn runtime_entropy() -> RuntimeEntropyHandle {
    *ENTROPY_HANDLE
        .try_get()
        .expect("runtime entropy is installed before the PRNS engine")
}
