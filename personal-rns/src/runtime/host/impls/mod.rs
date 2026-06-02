//! The host catalogue — concrete [`Host`](super::Host) impls, one per platform +
//! execution model (`LinuxSync`, `EmbassyHost`, …). Each is gated by its platform
//! feature; the neutral contract in the parent module carries no gate. Populated
//! as the hosts land.

#[cfg(feature = "std-host")]
mod linux_sync;
#[cfg(feature = "std-host")]
pub use linux_sync::LinuxSync;

#[cfg(feature = "embassy-host")]
mod embassy_host;
#[cfg(feature = "embassy-host")]
pub use embassy_host::EmbassyHost;

#[cfg(feature = "embassy-contract")]
mod embassy_contract;
#[cfg(feature = "embassy-contract")]
pub use embassy_contract::EmbassyContractHost;
