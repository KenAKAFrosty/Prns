#[cfg(feature = "std-sync-host")]
mod linux_sync;
#[cfg(feature = "std-sync-host")]
pub use linux_sync::LinuxSync;

#[cfg(feature = "embassy-contract")]
mod embassy_contract;
#[cfg(feature = "embassy-contract")]
pub use embassy_contract::EmbassyContractHost;
