use crate::identity::IdentityHash;
use crate::wire::DestinationHash;

use super::{
    RemoteControlControllerGrant, RemoteControlNodeIdentitySecrets, RemoteControlPublicAppData,
    RemoteControlRequestKind, RemoteControlRequestSet,
};

pub const DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS: usize = 8;
pub const REMOTE_CONTROL_REQUEST_ENDPOINT_ID: &str = "/remote-control";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerGrantsError {
    Empty,
    TooMany { actual: usize, maximum: usize },
    Duplicate { identity: IdentityHash },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerGrants<'a> {
    grants: &'a [RemoteControlControllerGrant],
}

impl<'a> RemoteControlControllerGrants<'a> {
    #[must_use]
    pub const fn grants(&self) -> &'a [RemoteControlControllerGrant] {
        self.grants
    }
}

impl<'a> TryFrom<&'a [RemoteControlControllerGrant]> for RemoteControlControllerGrants<'a> {
    type Error = RemoteControlControllerGrantsError;

    fn try_from(grants: &'a [RemoteControlControllerGrant]) -> Result<Self, Self::Error> {
        if grants.is_empty() {
            return Err(RemoteControlControllerGrantsError::Empty);
        }
        if grants.len() > DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS {
            return Err(RemoteControlControllerGrantsError::TooMany {
                actual: grants.len(),
                maximum: DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
            });
        }
        for (index, grant) in grants.iter().enumerate() {
            let identity_hash = grant.controller().identity_hash();
            if grants
                .iter()
                .skip(index.saturating_add(1))
                .any(|candidate| candidate.controller().identity_hash() == identity_hash)
            {
                return Err(RemoteControlControllerGrantsError::Duplicate {
                    identity: identity_hash,
                });
            }
        }
        Ok(Self { grants })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlInitialAccess<'a> {
    Nobody,
    Grants(RemoteControlControllerGrants<'a>),
}

impl RemoteControlInitialAccess<'_> {
    #[must_use]
    pub const fn grants(&self) -> &[RemoteControlControllerGrant] {
        match self {
            Self::Nobody => &[],
            Self::Grants(grants) => grants.grants(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlSelfAnnouncement {
    Unavailable,
    Destination(DestinationHash),
}

// Available owns identity secrets until assembly consumes the recipe. The no-alloc path cannot
// box them, and Unavailable must remain a payload-free state rather than inventing identities.
#[allow(clippy::large_enum_variant)]
pub enum RemoteControlService<'a> {
    Unavailable,
    Available(RemoteControlConfiguration<'a>),
}

pub struct RemoteControlConfiguration<'a> {
    identity_secrets: RemoteControlNodeIdentitySecrets,
    default_public_app_data: RemoteControlPublicAppData<'a>,
    initial_access: RemoteControlInitialAccess<'a>,
    self_announcement: RemoteControlSelfAnnouncement,
}

impl<'a> RemoteControlService<'a> {
    #[must_use]
    pub const fn new(
        identity_secrets: RemoteControlNodeIdentitySecrets,
        default_public_app_data: RemoteControlPublicAppData<'a>,
        initial_access: RemoteControlInitialAccess<'a>,
        self_announcement: RemoteControlSelfAnnouncement,
    ) -> Self {
        Self::Available(RemoteControlConfiguration {
            identity_secrets,
            default_public_app_data,
            initial_access,
            self_announcement,
        })
    }

    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    #[must_use]
    pub const fn configuration(&self) -> Option<&RemoteControlConfiguration<'a>> {
        match self {
            Self::Unavailable => None,
            Self::Available(configuration) => Some(configuration),
        }
    }

    #[must_use]
    pub fn into_configuration(self) -> Option<RemoteControlConfiguration<'a>> {
        match self {
            Self::Unavailable => None,
            Self::Available(configuration) => Some(configuration),
        }
    }

    #[must_use]
    pub fn available_requests(&self) -> RemoteControlRequestSet {
        self.configuration().map_or_else(
            RemoteControlRequestSet::empty,
            RemoteControlConfiguration::available_requests,
        )
    }
}

impl<'a> RemoteControlConfiguration<'a> {
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
    pub const fn self_announcement(&self) -> RemoteControlSelfAnnouncement {
        self.self_announcement
    }

    #[must_use]
    pub fn available_requests(&self) -> RemoteControlRequestSet {
        let mut available = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
        match self.self_announcement {
            RemoteControlSelfAnnouncement::Unavailable => {}
            RemoteControlSelfAnnouncement::Destination(_) => {
                let _inserted = available.insert(RemoteControlRequestKind::AnnounceSelf);
            }
        }
        available
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlNodeIdentitySecrets,
        RemoteControlPublicAppData<'a>,
        RemoteControlInitialAccess<'a>,
        RemoteControlSelfAnnouncement,
    ) {
        (
            self.identity_secrets,
            self.default_public_app_data,
            self.initial_access,
            self.self_announcement,
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
        RemoteControlControllerGrantError, RemoteControlControllerIdentity,
        RemoteControlControllerIdentitySecret, RemoteControlRequestKind, RemoteControlRequestSet,
        RemoteControlTargetIdentitySecret,
    };

    const TOO_MANY_GRANTS: usize = DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS.saturating_add(1);

    fn controller(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn grant(fill: u8) -> RemoteControlControllerGrant {
        RemoteControlControllerGrant::new(
            controller(fill),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap()
    }

    fn identity_secrets() -> RemoteControlNodeIdentitySecrets {
        RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [0x31; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [0x32; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap()
    }

    #[test]
    fn a_controller_grant_requires_at_least_one_permitted_request() {
        assert_eq!(
            RemoteControlControllerGrant::new(controller(1), RemoteControlRequestSet::empty()),
            Err(RemoteControlControllerGrantError::NoPermittedRequests),
        );
        let controller = controller(2);
        let permitted_requests =
            RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf);
        let grant = RemoteControlControllerGrant::new(controller, permitted_requests).unwrap();

        assert_eq!(grant.controller(), &controller);
        assert_eq!(grant.permitted_requests(), &permitted_requests);
        assert!(!grant.permits(RemoteControlRequestKind::Describe));
        assert!(grant.permits(RemoteControlRequestKind::AnnounceSelf));
    }

    #[test]
    fn initial_access_requires_an_explicit_nobody_or_nonempty_grant_set() {
        assert_eq!(RemoteControlInitialAccess::Nobody.grants(), &[],);
        assert_eq!(
            RemoteControlControllerGrants::try_from([].as_slice()),
            Err(RemoteControlControllerGrantsError::Empty),
        );

        let grants = [grant(1), grant(2)];
        let allowed = RemoteControlControllerGrants::try_from(grants.as_slice());
        assert_eq!(
            allowed.as_ref().map(RemoteControlControllerGrants::grants),
            Ok(grants.as_slice()),
        );
        assert_eq!(
            allowed
                .map(RemoteControlInitialAccess::Grants)
                .as_ref()
                .map(RemoteControlInitialAccess::grants),
            Ok(grants.as_slice()),
        );
    }

    #[test]
    fn initial_access_accepts_exactly_eight_distinct_grants() {
        let maximum =
            core::array::from_fn::<_, DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS, _>(|index| {
                grant(index as u8)
            });
        let oversized = core::array::from_fn::<_, TOO_MANY_GRANTS, _>(|index| grant(index as u8));

        assert_eq!(
            RemoteControlControllerGrants::try_from(maximum.as_slice())
                .as_ref()
                .map(|grants| grants.grants().len()),
            Ok(DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS),
        );
        assert_eq!(
            RemoteControlControllerGrants::try_from(oversized.as_slice()),
            Err(RemoteControlControllerGrantsError::TooMany {
                actual: oversized.len(),
                maximum: DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
            }),
        );
    }

    #[test]
    fn initial_access_rejects_duplicate_controller_grants() {
        let duplicate = grant(3);
        let grants = [grant(1), duplicate, grant(2), duplicate];

        assert_eq!(
            RemoteControlControllerGrants::try_from(grants.as_slice()),
            Err(RemoteControlControllerGrantsError::Duplicate {
                identity: duplicate.controller().identity_hash(),
            }),
        );
    }

    #[test]
    fn service_moves_all_configuration_back_out_without_cloning_secrets() {
        let identity_secrets = identity_secrets();
        let identities = identity_secrets.identities();
        let default_public_app_data =
            RemoteControlPublicAppData::try_from(b"application".as_slice()).unwrap();
        let service = RemoteControlService::new(
            identity_secrets,
            default_public_app_data,
            RemoteControlInitialAccess::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        );

        let (identity_secrets, default_public_app_data, initial_access, self_announcement) =
            service.into_configuration().unwrap().into_parts();

        assert_eq!(identity_secrets.identities(), identities);
        assert_eq!(default_public_app_data.as_bytes(), b"application");
        assert_eq!(initial_access, RemoteControlInitialAccess::Nobody);
        assert_eq!(
            self_announcement,
            RemoteControlSelfAnnouncement::Unavailable
        );
    }

    #[test]
    fn service_availability_is_explicit_and_available_actions_are_derived() {
        let unavailable = RemoteControlService::Unavailable;
        let describe_only = RemoteControlService::new(
            identity_secrets(),
            RemoteControlPublicAppData::empty(),
            RemoteControlInitialAccess::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        );
        let available = RemoteControlService::new(
            identity_secrets(),
            RemoteControlPublicAppData::empty(),
            RemoteControlInitialAccess::Nobody,
            RemoteControlSelfAnnouncement::Destination(DestinationHash::new([0x43; 16])),
        );

        assert!(!unavailable.is_available());
        assert!(unavailable.configuration().is_none());
        assert!(describe_only.is_available());
        assert_eq!(
            unavailable.available_requests(),
            RemoteControlRequestSet::empty(),
        );
        assert_eq!(
            describe_only.available_requests(),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
        assert_eq!(
            available.available_requests(),
            RemoteControlRequestSet::all(),
        );
    }
}
