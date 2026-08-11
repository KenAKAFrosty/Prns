use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

use super::node::Mtx;

pub(super) const RUNTIME_ENTROPY_SEED_LEN: usize = 32;
static RUNTIME_ENTROPY: BlockingMutex<Mtx, RefCell<Option<ChaCha20Rng>>> =
    BlockingMutex::new(RefCell::new(None));

/// Seed the runtime CSPRNG from an independent, full-width hardware RNG draw before SoftDevice takes ownership of the nRF RNG peripheral.
pub(super) fn initialize_runtime_entropy(seed: &[u8; RUNTIME_ENTROPY_SEED_LEN]) {
    RUNTIME_ENTROPY.lock(|slot| {
        let mut slot = slot.borrow_mut();
        assert!(
            slot.is_none(),
            "runtime entropy is initialized exactly once"
        );
        *slot = Some(ChaCha20Rng::from_seed(*seed));
    });
}

/// Supply all post-boot cryptographic entropy from one serialized CSPRNG stream.
pub(super) fn runtime_entropy(bytes: &mut [u8]) {
    RUNTIME_ENTROPY.lock(|slot| {
        slot.borrow_mut()
            .as_mut()
            .expect("runtime entropy is initialized before the PRNS engine")
            .fill_bytes(bytes);
    });
}
