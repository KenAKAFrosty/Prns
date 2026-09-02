use crate::entropy::{EntropySource, RuntimeEntropy};
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
pub enum RemoteControlNodeIdentityBootstrapError<VaultError> {
    InvalidLabel(IdentityLabelError),
    ControllerLoad(VaultError),
    TargetLoad(VaultError),
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

impl<VaultError> core::fmt::Display for RemoteControlNodeIdentityBootstrapError<VaultError>
where
    VaultError: core::fmt::Display,
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

impl<VaultError> core::error::Error for RemoteControlNodeIdentityBootstrapError<VaultError>
where
    VaultError: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::ControllerLoad(error)
            | Self::TargetLoad(error)
            | Self::ControllerStore(error)
            | Self::TargetStore(error)
            | Self::ControllerVerificationLoad(error)
            | Self::TargetVerificationLoad(error) => Some(error),
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
    /// Loads the stable controller/target pair or generates it from an initialized runtime stream.
    ///
    /// Arbitrary callbacks cannot cross this bootstrap boundary.
    ///
    /// ```compile_fail
    /// use prns_core::identity::vault::IdentityVault;
    /// use prns_core::remote_control::RemoteControlNodeIdentityBootstrap;
    ///
    /// fn bootstrap<V: IdentityVault>(vault: &mut V) {
    ///     let mut callback = |output: &mut [u8]| output.fill(0x42);
    ///     let _ = RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
    ///         vault,
    ///         &mut callback,
    ///     );
    /// }
    /// ```
    pub fn load_or_generate_with_runtime_entropy<Vault, S>(
        vault: &mut Vault,
        entropy: &mut RuntimeEntropy<S>,
    ) -> Result<Self, RemoteControlNodeIdentityBootstrapError<Vault::Error>>
    where
        Vault: IdentityVault,
        S: EntropySource,
    {
        Self::load_or_generate_inner(vault, |output| entropy.fill_random(output))
    }

    fn load_or_generate_inner<Vault>(
        vault: &mut Vault,
        mut fill_random: impl FnMut(&mut [u8]),
    ) -> Result<Self, RemoteControlNodeIdentityBootstrapError<Vault::Error>>
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
                fill_random(&mut secret[..]);
                secret
            }
        };
        let target_secret = match loaded_target {
            Some(secret) => secret,
            None => {
                let mut secret = Zeroizing::new([0; IDENTITY_SECRET_KEY_LEN]);
                fill_random(&mut secret[..]);
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

    fn verify_controller<Vault>(
        vault: &Vault,
        label: &IdentityLabel,
        expected: &IdentitySecretKey,
    ) -> Result<(), RemoteControlNodeIdentityBootstrapError<Vault::Error>>
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

    fn verify_target<Vault>(
        vault: &Vault,
        label: &IdentityLabel,
        expected: &IdentitySecretKey,
    ) -> Result<(), RemoteControlNodeIdentityBootstrapError<Vault::Error>>
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
    use core::convert::Infallible;
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

    struct TestEntropySource(u8);

    impl EntropySource for TestEntropySource {
        type Error = Infallible;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            output.fill(self.0);
            Ok(())
        }
    }

    fn runtime_entropy(seed: u8) -> RuntimeEntropy<TestEntropySource> {
        RuntimeEntropy::try_new(TestEntropySource(seed)).unwrap()
    }

    fn distinct_entropy() -> impl FnMut(&mut [u8]) {
        let mut fill = 0x31;
        move |bytes| {
            bytes.fill(fill);
            fill = 0x42;
        }
    }

    #[test]
    fn fresh_bootstrap_generates_verifies_and_returns_both_secrets() {
        let mut vault = MemoryVault::default();
        let mut entropy = runtime_entropy(0x31);

        let bootstrap = RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
            &mut vault,
            &mut entropy,
        )
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
        let first = RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
            &mut vault,
            &mut runtime_entropy(0x31),
        )
        .unwrap();
        let first_identities = first.secrets().identities();
        let mut load_entropy = runtime_entropy(0x92);
        let mut untouched_entropy = runtime_entropy(0x92);
        let second = RemoteControlNodeIdentityBootstrap::load_or_generate_with_runtime_entropy(
            &mut vault,
            &mut load_entropy,
        )
        .unwrap();

        assert_eq!(second.secrets().identities(), first_identities);
        assert_eq!(
            second.origins(),
            &RemoteControlNodeIdentityOrigins {
                controller: IdentityOrigin::Loaded,
                target: IdentityOrigin::Loaded,
            },
        );

        let mut after_load = [0_u8; 32];
        let mut untouched = [0_u8; 32];
        load_entropy.fill_random(&mut after_load);
        untouched_entropy.fill_random(&mut untouched);
        assert_eq!(after_load, untouched);
    }

    #[test]
    fn a_partial_first_write_is_completed_without_rotating_the_stored_controller() {
        let mut vault = MemoryVault {
            entries: HashMap::new(),
            fault: MemoryVaultFault::RefuseTargetStore,
        };

        assert!(matches!(
            RemoteControlNodeIdentityBootstrap::load_or_generate_inner(
                &mut vault,
                distinct_entropy(),
            ),
            Err(RemoteControlNodeIdentityBootstrapError::TargetStore(
                MemoryVaultError::StoreRefused,
            )),
        ));
        let stored_controller = vault.entries.get(CONTROLLER_IDENTITY_LABEL).copied();
        assert!(!vault.entries.contains_key(TARGET_IDENTITY_LABEL));

        vault.fault = MemoryVaultFault::None;
        let mut entropy_calls = 0;
        let bootstrap =
            RemoteControlNodeIdentityBootstrap::load_or_generate_inner(&mut vault, |bytes| {
                entropy_calls += 1;
                bytes.fill(0x53);
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

        let result =
            RemoteControlNodeIdentityBootstrap::load_or_generate_inner(&mut vault, |bytes| {
                bytes.fill(0x64);
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
    fn vault_load_failure_is_preserved_by_bootstrap_phase() {
        let mut vault = MemoryVault {
            entries: HashMap::new(),
            fault: MemoryVaultFault::RefuseControllerLoad,
        };

        let result = RemoteControlNodeIdentityBootstrap::load_or_generate_inner(
            &mut vault,
            distinct_entropy(),
        );

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

        let result = RemoteControlNodeIdentityBootstrap::load_or_generate_inner(
            &mut vault,
            distinct_entropy(),
        );

        assert!(matches!(
            result,
            Err(RemoteControlNodeIdentityBootstrapError::ControllerVerificationMismatch),
        ));
    }
}
