#[cfg(feature = "std-sync-host")]
mod linux_sync;
#[cfg(feature = "std-sync-host")]
pub use linux_sync::{LinuxSync, WakeHandle};

#[cfg(feature = "embassy-contract")]
mod embassy_contract;
#[cfg(feature = "embassy-contract")]
pub use embassy_contract::EmbassyContractHost;

#[cfg(feature = "tokio-host")]
mod tokio_host;
#[cfg(feature = "tokio-host")]
pub use tokio_host::{TokioHost, TokioWakeHandle};
