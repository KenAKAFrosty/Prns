use crate::identity::vault::{
    IdentityLabel, IdentityLabelError, IdentityOrigin, IdentitySecretKey, IdentityVault,
};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

use super::{
    RemoteControlControllerIdentitySecret, RemoteControlNodeIdentitySecrets,
    RemoteControlNodeIdentitySecretsError, RemoteControlTargetIdentitySecret,
};

pub const REMOTE_CONTROL_IDENTITY_VAULT_SLOTS: usize = 2;
const CONTROLLER_IDENTITY_LABEL: &str = "controller";
const TARGET_IDENTITY_LABEL: &str = "target";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlNodeIdentityOrigins {
    controller: IdentityOrigin,
    target: IdentityOrigin,
}

impl RemoteControlNodeIdentityOrigins {
    #[must_use]
    pub const fn controller(&self) -> IdentityOrigin {
        self.controller
    }

    #[must_use]
    pub const fn target(&self) -> IdentityOrigin {
        self.target
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlNodeIdentityBootstrapError<VaultError, EntropyError> {
    InvalidLabel(IdentityLabelError),
    ControllerLoad(VaultError),
    TargetLoad(VaultError),
    ControllerEntropy(EntropyError),
    TargetEntropy(EntropyError),
    InvalidPair(RemoteControlNodeIdentitySecretsError),
    ControllerStore(VaultError),
    TargetStore(VaultError),
    ControllerVerificationLoad(VaultError),
    TargetVerificationLoad(VaultError),
    ControllerVerificationMissing,
    TargetVerificationMissing,
    ControllerVerificationMismatch,
    TargetVerificationMismatch,
}

impl<VaultError, EntropyError> core::fmt::Display
    for RemoteControlNodeIdentityBootstrapError<VaultError, EntropyError>
where
    VaultError: core::fmt::Display,
    EntropyError: core::fmt::Display,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidLabel(_) => {
                formatter.write_str("a RemoteControl identity label is invalid")
            }
            Self::ControllerLoad(error) => {
                write!(
                    formatter,
                    "controller identity could not be loaded: {error}"
                )
            }
            Self::TargetLoad(error) => {
                write!(formatter, "target identity could not be loaded: {error}")
            }
            Self::ControllerEntropy(error) => {
                write!(
                    formatter,
                    "controller identity could not be generated: {error}"
                )
            }
            Self::TargetEntropy(error) => {
                write!(formatter, "target identity could not be generated: {error}")
            }
            Self::InvalidPair(_) => {
                formatter.write_str("controller and target resolve to the same identity")
            }
            Self::ControllerStore(error) => {
                write!(
                    formatter,
                    "controller identity could not be stored: {error}"
                )
            }
            Self::TargetStore(error) => {
                write!(formatter, "target identity could not be stored: {error}")
            }
            Self::ControllerVerificationLoad(error) => write!(
                formatter,
                "stored controller identity could not be verified: {error}"
            ),
            Self::TargetVerificationLoad(error) => write!(
                formatter,
                "stored target identity could not be verified: {error}"
            ),
            Self::ControllerVerificationMissing => {
                formatter.write_str("stored controller identity is missing")
            }
            Self::TargetVerificationMissing => {
                formatter.write_str("stored target identity is missing")
            }
            Self::ControllerVerificationMismatch => formatter
                .write_str("stored controller identity does not match the generated identity"),
            Self::TargetVerificationMismatch => {
                formatter.write_str("stored target identity does not match the generated identity")
            }
        }
    }
}

impl<VaultError, EntropyError> core::error::Error
    for RemoteControlNodeIdentityBootstrapError<VaultError, EntropyError>
where
    VaultError: core::error::Error + 'static,
    EntropyError: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::ControllerLoad(error)
            | Self::TargetLoad(error)
            | Self::ControllerStore(error)
            | Self::TargetStore(error)
            | Self::ControllerVerificationLoad(error)
            | Self::TargetVerificationLoad(error) => Some(error),
            Self::ControllerEntropy(error) | Self::TargetEntropy(error) => Some(error),
            Self::InvalidLabel(_)
            | Self::InvalidPair(_)
            | Self::ControllerVerificationMissing
            | Self::TargetVerificationMissing
            | Self::ControllerVerificationMismatch
            | Self::TargetVerificationMismatch => None,
        }
    }
}

pub struct RemoteControlNodeIdentityBootstrap {
    secrets: RemoteControlNodeIdentitySecrets,
    origins: RemoteControlNodeIdentityOrigins,
}

impl RemoteControlNodeIdentityBootstrap {
    pub fn load_or_generate<Vault, EntropyError>(
        vault: &mut Vault,
        mut fill_entropy: impl FnMut(&mut [u8]) -> Result<(), EntropyError>,
    ) -> Result<Self, RemoteControlNodeIdentityBootstrapError<Vault::Error, EntropyError>>
    where
        Vault: IdentityVault,
    {
        let controller_label = IdentityLabel::new(CONTROLLER_IDENTITY_LABEL)
            .map_err(RemoteControlNodeIdentityBootstrapError::InvalidLabel)?;
        let target_label = IdentityLabel::new(TARGET_IDENTITY_LABEL)
            .map_err(RemoteControlNodeIdentityBootstrapError::InvalidLabel)?;

        let loaded_controller = vault
            .load(&controller_label)
            .map_err(RemoteControlNodeIdentityBootstrapError::ControllerLoad)?;
        let loaded_target = vault
            .load(&target_label)
            .map_err(RemoteControlNodeIdentityBootstrapError::TargetLoad)?;

        let controller_origin = match loaded_controller {
            Some(_) => IdentityOrigin::Loaded,
            None => IdentityOrigin::Generated,
        };
        let target_origin = match loaded_target {
            Some(_) => IdentityOrigin::Loaded,
            None => IdentityOrigin::Generated,
        };

        let controller_secret = match loaded_controller {
            Some(secret) => secret,
            None => {
                let mut secret = Zeroizing::new([0; IDENTITY_SECRET_KEY_LEN]);
                fill_entropy(&mut secret[..])
                    .map_err(RemoteControlNodeIdentityBootstrapError::ControllerEntropy)?;
                secret
            }
        };
        let target_secret = match loaded_target {
            Some(secret) => secret,
            None => {
                let mut secret = Zeroizing::new([0; IDENTITY_SECRET_KEY_LEN]);
                fill_entropy(&mut secret[..])
                    .map_err(RemoteControlNodeIdentityBootstrapError::TargetEntropy)?;
                secret
            }
        };

        RemoteControlNodeIdentitySecrets::validate_secret_keys(&controller_secret, &target_secret)
            .map_err(RemoteControlNodeIdentityBootstrapError::InvalidPair)?;

        if controller_origin == IdentityOrigin::Generated {
            vault
                .store(&controller_label, &controller_secret)
                .map_err(RemoteControlNodeIdentityBootstrapError::ControllerStore)?;
        }
        if target_origin == IdentityOrigin::Generated {
            vault
                .store(&target_label, &target_secret)
                .map_err(RemoteControlNodeIdentityBootstrapError::TargetStore)?;
        }

        Self::verify_controller(vault, &controller_label, &controller_secret)?;
        Self::verify_target(vault, &target_label, &target_secret)?;

        let secrets = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(controller_secret),
            RemoteControlTargetIdentitySecret::from(target_secret),
        )
        .map_err(RemoteControlNodeIdentityBootstrapError::InvalidPair)?;

        Ok(Self {
            secrets,
            origins: RemoteControlNodeIdentityOrigins {
                controller: controller_origin,
                target: target_origin,
            },
        })
    }

    fn verify_controller<Vault, EntropyError>(
        vault: &Vault,
        label: &IdentityLabel,
        expected: &IdentitySecretKey,
    ) -> Result<(), RemoteControlNodeIdentityBootstrapError<Vault::Error, EntropyError>>
    where
        Vault: IdentityVault,
    {
        let Some(stored) = vault
            .load(label)
            .map_err(RemoteControlNodeIdentityBootstrapError::ControllerVerificationLoad)?
        else {
            return Err(RemoteControlNodeIdentityBootstrapError::ControllerVerificationMissing);
        };
        if stored != *expected {
            return Err(RemoteControlNodeIdentityBootstrapError::ControllerVerificationMismatch);
        }
        Ok(())
    }

    fn verify_target<Vault, EntropyError>(
        vault: &Vault,
        label: &IdentityLabel,
        expected: &IdentitySecretKey,
    ) -> Result<(), RemoteControlNodeIdentityBootstrapError<Vault::Error, EntropyError>>
    where
        Vault: IdentityVault,
    {
        let Some(stored) = vault
            .load(label)
            .map_err(RemoteControlNodeIdentityBootstrapError::TargetVerificationLoad)?
        else {
            return Err(RemoteControlNodeIdentityBootstrapError::TargetVerificationMissing);
        };
        if stored != *expected {
            return Err(RemoteControlNodeIdentityBootstrapError::TargetVerificationMismatch);
        }
        Ok(())
    }

    #[must_use]
    pub const fn secrets(&self) -> &RemoteControlNodeIdentitySecrets {
        &self.secrets
    }

    #[must_use]
    pub const fn origins(&self) -> &RemoteControlNodeIdentityOrigins {
        &self.origins
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlNodeIdentitySecrets,
        RemoteControlNodeIdentityOrigins,
    ) {
        (self.secrets, self.origins)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::identity::vault::Removal;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MemoryVaultError {
        LoadRefused,
        StoreRefused,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MemoryVaultFault {
        None,
        RefuseControllerLoad,
        RefuseTargetStore,
        CorruptControllerVerification,
    }

    struct MemoryVault {
        entries: HashMap<String, [u8; IDENTITY_SECRET_KEY_LEN]>,
        fault: MemoryVaultFault,
    }

    impl Default for MemoryVault {
        fn default() -> Self {
            Self {
                entries: HashMap::new(),
                fault: MemoryVaultFault::None,
            }
        }
    }

    impl IdentityVault for MemoryVault {
        type Error = MemoryVaultError;

        fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error> {
            if self.fault == MemoryVaultFault::RefuseControllerLoad
                && label.as_str() == CONTROLLER_IDENTITY_LABEL
            {
                return Err(MemoryVaultError::LoadRefused);
            }
            let Some(secret) = self.entries.get(label.as_str()) else {
                return Ok(None);
            };
            let mut loaded = *secret;
            if self.fault == MemoryVaultFault::CorruptControllerVerification
                && label.as_str() == CONTROLLER_IDENTITY_LABEL
            {
                loaded[0] ^= 0xFF;
            }
            Ok(Some(Zeroizing::new(loaded)))
        }

        fn store(
            &mut self,
            label: &IdentityLabel,
            secret: &[u8; IDENTITY_SECRET_KEY_LEN],
        ) -> Result<(), Self::Error> {
            if self.fault == MemoryVaultFault::RefuseTargetStore
                && label.as_str() == TARGET_IDENTITY_LABEL
            {
                return Err(MemoryVaultError::StoreRefused);
            }
            self.entries.insert(label.as_str().to_owned(), *secret);
            Ok(())
        }

        fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
            Ok(match self.entries.remove(label.as_str()) {
                Some(_) => Removal::Removed,
                None => Removal::NothingStored,
            })
        }

        fn stored_blob_len(&self, _label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
            Ok(None)
        }

        fn load_blob<'buffer>(
            &self,
            _label: &IdentityLabel,
            _buffer: &'buffer mut [u8],
        ) -> Result<Option<&'buffer [u8]>, Self::Error> {
            Ok(None)
        }

        fn store_blob(&mut self, _label: &IdentityLabel, _blob: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum EntropyError {
        Unavailable,
    }

    fn distinct_entropy() -> impl FnMut(&mut [u8]) -> Result<(), EntropyError> {
        let mut fill = 0x31;
        move |bytes| {
            bytes.fill(fill);
            fill = 0x42;
            Ok(())
        }
    }

    #[test]
    fn fresh_bootstrap_generates_verifies_and_returns_both_secrets() {
        let mut vault = MemoryVault::default();

        let bootstrap =
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, distinct_entropy())
                .unwrap();

        assert_eq!(
            bootstrap.origins(),
            &RemoteControlNodeIdentityOrigins {
                controller: IdentityOrigin::Generated,
                target: IdentityOrigin::Generated,
            },
        );
        assert_ne!(
            bootstrap
                .secrets()
                .identities()
                .controller()
                .identity_hash(),
            bootstrap.secrets().identities().target().identity_hash(),
        );
        assert_eq!(vault.entries.len(), 2);
    }

    #[test]
    fn subsequent_bootstrap_loads_the_stable_pair_without_entropy() {
        let mut vault = MemoryVault::default();
        let first =
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, distinct_entropy())
                .unwrap();
        let first_identities = first.secrets().identities();
        let mut entropy_calls = 0;

        let second = RemoteControlNodeIdentityBootstrap::load_or_generate(
            &mut vault,
            |_bytes| -> Result<(), EntropyError> {
                entropy_calls += 1;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(entropy_calls, 0);
        assert_eq!(second.secrets().identities(), first_identities);
        assert_eq!(
            second.origins(),
            &RemoteControlNodeIdentityOrigins {
                controller: IdentityOrigin::Loaded,
                target: IdentityOrigin::Loaded,
            },
        );
    }

    #[test]
    fn a_partial_first_write_is_completed_without_rotating_the_stored_controller() {
        let mut vault = MemoryVault {
            entries: HashMap::new(),
            fault: MemoryVaultFault::RefuseTargetStore,
        };

        assert!(matches!(
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, distinct_entropy(),),
            Err(RemoteControlNodeIdentityBootstrapError::TargetStore(
                MemoryVaultError::StoreRefused,
            )),
        ));
        let stored_controller = vault.entries.get(CONTROLLER_IDENTITY_LABEL).copied();
        assert!(!vault.entries.contains_key(TARGET_IDENTITY_LABEL));

        vault.fault = MemoryVaultFault::None;
        let mut entropy_calls = 0;
        let bootstrap = RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |bytes| {
            entropy_calls += 1;
            bytes.fill(0x53);
            Ok::<(), EntropyError>(())
        })
        .unwrap();

        assert_eq!(entropy_calls, 1);
        assert_eq!(
            vault.entries.get(CONTROLLER_IDENTITY_LABEL).copied(),
            stored_controller,
        );
        assert_eq!(
            bootstrap.origins(),
            &RemoteControlNodeIdentityOrigins {
                controller: IdentityOrigin::Loaded,
                target: IdentityOrigin::Generated,
            },
        );
    }

    #[test]
    fn equal_generated_identities_are_rejected_before_storage() {
        let mut vault = MemoryVault::default();

        let result = RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |bytes| {
            bytes.fill(0x64);
            Ok::<(), EntropyError>(())
        });

        assert!(matches!(
            result,
            Err(RemoteControlNodeIdentityBootstrapError::InvalidPair(
                RemoteControlNodeIdentitySecretsError::ControllerAndTargetAreSameIdentity,
            )),
        ));
        assert!(vault.entries.is_empty());
    }

    #[test]
    fn entropy_failure_returns_no_pair_and_stores_nothing() {
        let mut vault = MemoryVault::default();

        let result = RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, |_bytes| {
            Err(EntropyError::Unavailable)
        });

        assert!(matches!(
            result,
            Err(RemoteControlNodeIdentityBootstrapError::ControllerEntropy(
                EntropyError::Unavailable,
            )),
        ));
        assert!(vault.entries.is_empty());
    }

    #[test]
    fn vault_load_failure_is_preserved_by_bootstrap_phase() {
        let mut vault = MemoryVault {
            entries: HashMap::new(),
            fault: MemoryVaultFault::RefuseControllerLoad,
        };

        let result =
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, distinct_entropy());

        assert!(matches!(
            result,
            Err(RemoteControlNodeIdentityBootstrapError::ControllerLoad(
                MemoryVaultError::LoadRefused,
            )),
        ));
    }

    #[test]
    fn post_store_verification_rejects_changed_secret_material() {
        let mut vault = MemoryVault {
            entries: HashMap::new(),
            fault: MemoryVaultFault::CorruptControllerVerification,
        };

        let result =
            RemoteControlNodeIdentityBootstrap::load_or_generate(&mut vault, distinct_entropy());

        assert!(matches!(
            result,
            Err(RemoteControlNodeIdentityBootstrapError::ControllerVerificationMismatch),
        ));
    }
}
