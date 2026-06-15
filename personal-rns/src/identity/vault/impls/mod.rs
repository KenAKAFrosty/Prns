#[cfg(feature = "std")]
mod file;
#[cfg(feature = "std")]
pub use file::{read_identity_file, FileVault, FileVaultError};

#[cfg(feature = "std")]
mod host;
#[cfg(feature = "std")]
pub use host::{HostLoadSource, HostVault, HostVaultError};
