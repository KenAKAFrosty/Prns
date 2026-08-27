use crate::identity::IdentityHash;

use super::{
    RemoteControlControllerIdentity, RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData,
};

pub const DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAllowedControllersError {
    Empty,
    TooMany { actual: usize, maximum: usize },
    Duplicate { identity: IdentityHash },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlAllowedControllers<'a> {
    identities: &'a [RemoteControlControllerIdentity],
}

impl<'a> RemoteControlAllowedControllers<'a> {
    #[must_use]
    pub const fn identities(&self) -> &'a [RemoteControlControllerIdentity] {
        self.identities
    }
}

impl<'a> TryFrom<&'a [RemoteControlControllerIdentity]> for RemoteControlAllowedControllers<'a> {
    type Error = RemoteControlAllowedControllersError;

    fn try_from(identities: &'a [RemoteControlControllerIdentity]) -> Result<Self, Self::Error> {
        if identities.is_empty() {
            return Err(RemoteControlAllowedControllersError::Empty);
        }
        if identities.len() > DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS {
            return Err(RemoteControlAllowedControllersError::TooMany {
                actual: identities.len(),
                maximum: DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS,
            });
        }
        for (index, identity) in identities.iter().enumerate() {
            let identity_hash = identity.identity_hash();
            if identities
                .iter()
                .skip(index.saturating_add(1))
                .any(|candidate| candidate.identity_hash() == identity_hash)
            {
                return Err(RemoteControlAllowedControllersError::Duplicate {
                    identity: identity_hash,
                });
            }
        }
        Ok(Self { identities })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlInitialAccess<'a> {
    Nobody,
    Controllers(RemoteControlAllowedControllers<'a>),
}

impl RemoteControlInitialAccess<'_> {
    #[must_use]
    pub const fn controllers(&self) -> &[RemoteControlControllerIdentity] {
        match self {
            Self::Nobody => &[],
            Self::Controllers(controllers) => controllers.identities(),
        }
    }
}

pub struct RemoteControlService<'a> {
    identity_secrets: RemoteControlNodeIdentitySecrets,
    default_public_app_data: RemoteControlPublicAppData<'a>,
    initial_access: RemoteControlInitialAccess<'a>,
}

impl<'a> RemoteControlService<'a> {
    #[must_use]
    pub const fn new(
        identity_secrets: RemoteControlNodeIdentitySecrets,
        default_public_app_data: RemoteControlPublicAppData<'a>,
        initial_access: RemoteControlInitialAccess<'a>,
    ) -> Self {
        Self {
            identity_secrets,
            default_public_app_data,
            initial_access,
        }
    }

    #[must_use]
    pub const fn identity_secrets(&self) -> &RemoteControlNodeIdentitySecrets {
        &self.identity_secrets
    }

    #[must_use]
    pub const fn default_public_app_data(&self) -> &RemoteControlPublicAppData<'a> {
        &self.default_public_app_data
    }

    #[must_use]
    pub const fn initial_access(&self) -> &RemoteControlInitialAccess<'a> {
        &self.initial_access
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlNodeIdentitySecrets,
        RemoteControlPublicAppData<'a>,
        RemoteControlInitialAccess<'a>,
    ) {
        (
            self.identity_secrets,
            self.default_public_app_data,
            self.initial_access,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
        IDENTITY_SECRET_KEY_LEN,
    };
    use crate::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlTargetIdentitySecret,
    };

    const TOO_MANY_CONTROLLERS: usize =
        DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS.saturating_add(1);

    fn controller(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    #[test]
    fn initial_access_requires_an_explicit_nobody_or_nonempty_controller_set() {
        assert_eq!(RemoteControlInitialAccess::Nobody.controllers(), &[],);
        assert_eq!(
            RemoteControlAllowedControllers::try_from([].as_slice()),
            Err(RemoteControlAllowedControllersError::Empty),
        );

        let controllers = [controller(1), controller(2)];
        let allowed = RemoteControlAllowedControllers::try_from(controllers.as_slice());
        assert_eq!(
            allowed
                .as_ref()
                .map(RemoteControlAllowedControllers::identities),
            Ok(controllers.as_slice()),
        );
        assert_eq!(
            allowed
                .map(RemoteControlInitialAccess::Controllers)
                .as_ref()
                .map(RemoteControlInitialAccess::controllers),
            Ok(controllers.as_slice()),
        );
    }

    #[test]
    fn initial_access_accepts_exactly_eight_distinct_controllers() {
        let maximum =
            core::array::from_fn::<_, DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS, _>(|index| {
                controller(index as u8)
            });
        let oversized =
            core::array::from_fn::<_, TOO_MANY_CONTROLLERS, _>(|index| controller(index as u8));

        assert_eq!(
            RemoteControlAllowedControllers::try_from(maximum.as_slice())
                .as_ref()
                .map(|controllers| controllers.identities().len()),
            Ok(DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS),
        );
        assert_eq!(
            RemoteControlAllowedControllers::try_from(oversized.as_slice()),
            Err(RemoteControlAllowedControllersError::TooMany {
                actual: oversized.len(),
                maximum: DEFAULT_MAX_REMOTE_CONTROL_ALLOWED_CONTROLLERS,
            }),
        );
    }

    #[test]
    fn initial_access_rejects_a_duplicate_controller_identity() {
        let duplicate = controller(3);
        let controllers = [controller(1), duplicate, controller(2), duplicate];

        assert_eq!(
            RemoteControlAllowedControllers::try_from(controllers.as_slice()),
            Err(RemoteControlAllowedControllersError::Duplicate {
                identity: duplicate.identity_hash(),
            }),
        );
    }

    #[test]
    fn service_moves_all_configuration_back_out_without_cloning_secrets() {
        let identity_secrets = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [0x31; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [0x32; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap();
        let identities = identity_secrets.identities();
        let default_public_app_data =
            RemoteControlPublicAppData::try_from(b"application".as_slice()).unwrap();
        let service = RemoteControlService::new(
            identity_secrets,
            default_public_app_data,
            RemoteControlInitialAccess::Nobody,
        );

        let (identity_secrets, default_public_app_data, initial_access) = service.into_parts();

        assert_eq!(identity_secrets.identities(), identities);
        assert_eq!(default_public_app_data.as_bytes(), b"application");
        assert_eq!(initial_access, RemoteControlInitialAccess::Nobody);
    }
}
