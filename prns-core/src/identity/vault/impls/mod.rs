#[cfg(feature = "std")]
mod file;
#[cfg(feature = "std")]
pub use file::{read_identity_file, FileVault, FileVaultError};

#[cfg(feature = "std")]
mod rns_compatibility;
#[cfg(feature = "std")]
pub use rns_compatibility::{LoadSource, RnsCompatibilityVault, RnsCompatibilityVaultError};

#[cfg(feature = "_keyring-vault")]
mod os_keyring;
#[cfg(feature = "_keyring-vault")]
pub use os_keyring::{KeyringVault, KeyringVaultError};

#[cfg(feature = "flash")]
mod flash;
#[cfg(feature = "flash")]
pub use flash::{FlashVault, FlashVaultError};
