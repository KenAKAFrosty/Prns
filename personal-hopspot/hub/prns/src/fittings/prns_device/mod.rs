mod backend;
#[cfg(test)]
mod behavior;
mod lease;
#[cfg(test)]
mod tests;
mod transport;
mod work;

use alloc::sync::Arc;
use hopspot_hub_core::{Connection, DeviceId};
use lease::PrnsLinkLease;
use personal_rns::identity::IdentityHash;
use personal_rns::runtime::PrnsNodeHandle;

pub use backend::PrnsInventoryTransport;
pub use transport::{PrnsDeviceIncoming, PrnsDeviceOutgoing};
pub use work::PrnsDeviceWork;

#[derive(Debug)]
pub enum PrnsFittingError {
    WrongDevice {
        expected: DeviceId,
        received: DeviceId,
    },
    UnexpectedTarget {
        expected: IdentityHash,
        received: IdentityHash,
    },
    WorkerStopped {
        source: tokio::task::JoinError,
    },
}

pub struct PrnsDeviceFitting<Backend: PrnsInventoryTransport = PrnsNodeHandle> {
    device: DeviceId,
    backend: Arc<Backend>,
    active: Option<(Connection, PrnsLinkLease<Backend>)>,
}

impl<Backend: PrnsInventoryTransport> PrnsDeviceFitting<Backend> {
    pub fn new(device: DeviceId, backend: Backend) -> Self {
        Self {
            device,
            backend: Arc::new(backend),
            active: None,
        }
    }
}
