cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod file;
        mod rns_compatibility;

        pub use file::{read_identity_file, FileVault, FileVaultError};
        pub use rns_compatibility::{
            LoadSource, RnsCompatibilityVault, RnsCompatibilityVaultError,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "_keyring-vault")] {
        mod os_keyring;

        pub use os_keyring::{
            KeyringService, KeyringServiceError, KeyringVault, KeyringVaultError,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "flash")] {
        mod flash;

        pub use flash::{FlashVault, FlashVaultError};
    }
}
