//! The host catalogue — concrete [`RuntimeHost`](super::RuntimeHost) impls, one
//! per platform + execution model (`LinuxSync`, `Esp32S3Embassy`,
//! `Esp32C6Sync`, …). Each is gated by its platform feature; the neutral
//! contract in the parent module carries no gate. Populated as the hosts land.

#[cfg(feature = "std-host")]
mod linux_sync;
#[cfg(feature = "std-host")]
pub use linux_sync::LinuxSync;

#[cfg(feature = "embassy-host")]
mod embassy_host;
#[cfg(feature = "embassy-host")]
pub use embassy_host::EmbassyHost;
