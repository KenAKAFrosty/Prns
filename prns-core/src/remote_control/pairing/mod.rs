use crate::identity::IdentityHash;
use crate::routing::announce::{derive_destination_hash, DottedNameHash};
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::{
    RemoteControlRequestSet, REMOTE_CONTROL_APPLICATION_NAME, REMOTE_CONTROL_NAMESPACE_ASPECT,
    REMOTE_CONTROL_SERVICE_ASPECT,
};

mod availability;
mod controller_pairing;
mod exchange;
mod target_pairing;

pub use availability::*;
pub use controller_pairing::*;
pub use exchange::*;
pub use target_pairing::*;

pub const REMOTE_CONTROL_PAIRING_APPLICATION_NAME: &str = REMOTE_CONTROL_APPLICATION_NAME;
pub const REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS: &[&str] = &[
    REMOTE_CONTROL_NAMESPACE_ASPECT,
    REMOTE_CONTROL_SERVICE_ASPECT,
    "pairing",
];

//Materialized here since it's a stable value (avoids paying the hash cost over and over at runtime for no reason)
const REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH: DottedNameHash =
    DottedNameHash::new([0x4d, 0x56, 0x19, 0xbe, 0x2d, 0xb0, 0xbb, 0xf5, 0x34, 0x16]);

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingIdentity {
    identity_hash: IdentityHash,
}

impl RemoteControlPairingIdentity {
    #[must_use]
    pub const fn new(identity_hash: IdentityHash) -> Self {
        Self { identity_hash }
    }

    #[must_use]
    pub const fn identity_hash(&self) -> IdentityHash {
        self.identity_hash
    }

    #[must_use]
    pub fn endpoint(&self) -> RemoteControlPairingEndpoint {
        self.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingEndpoint {
    destination_hash: DestinationHash,
}

impl RemoteControlPairingEndpoint {
    #[must_use]
    pub const fn destination_hash(&self) -> DestinationHash {
        self.destination_hash
    }
}

impl From<&RemoteControlPairingIdentity> for RemoteControlPairingEndpoint {
    fn from(identity: &RemoteControlPairingIdentity) -> Self {
        Self {
            destination_hash: derive_destination_hash(
                &identity.identity_hash,
                &REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH,
            ),
        }
    }
}

impl From<RemoteControlPairingEndpoint> for DestinationHash {
    fn from(endpoint: RemoteControlPairingEndpoint) -> Self {
        endpoint.destination_hash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingPermissionsError {
    NoPermittedRequests,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlPairingPermissions {
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlPairingPermissions {
    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn into_permitted_requests(self) -> RemoteControlRequestSet {
        self.permitted_requests
    }
}

impl TryFrom<RemoteControlRequestSet> for RemoteControlPairingPermissions {
    type Error = RemoteControlPairingPermissionsError;

    fn try_from(permitted_requests: RemoteControlRequestSet) -> Result<Self, Self::Error> {
        if permitted_requests.is_empty() {
            return Err(RemoteControlPairingPermissionsError::NoPermittedRequests);
        }
        Ok(Self { permitted_requests })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingWindowError {
    DeadlineNotFuture {
        opened_at: InstantMillis,
        expires_at: InstantMillis,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingWindow {
    expires_at: InstantMillis,
}

impl RemoteControlPairingWindow {
    pub fn new(
        opened_at: InstantMillis,
        expires_at: InstantMillis,
    ) -> Result<Self, RemoteControlPairingWindowError> {
        if expires_at <= opened_at {
            return Err(RemoteControlPairingWindowError::DeadlineNotFuture {
                opened_at,
                expires_at,
            });
        }
        Ok(Self { expires_at })
    }

    #[must_use]
    pub const fn expires_at(&self) -> InstantMillis {
        self.expires_at
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingSession {
    identity: RemoteControlPairingIdentity,
    window: RemoteControlPairingWindow,
    permissions: RemoteControlPairingPermissions,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenRemoteControlPairingSession<'a> {
    target_identity: IdentityHash,
    session: &'a RemoteControlPairingSession,
}

impl<'a> OpenRemoteControlPairingSession<'a> {
    #[must_use]
    pub const fn target_identity(self) -> IdentityHash {
        self.target_identity
    }

    #[must_use]
    pub const fn session(self) -> &'a RemoteControlPairingSession {
        self.session
    }
}

impl RemoteControlPairingSession {
    #[must_use]
    pub fn new(
        identity: RemoteControlPairingIdentity,
        window: RemoteControlPairingWindow,
        permissions: RemoteControlPairingPermissions,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
    ) -> Self {
        Self {
            identity,
            window,
            permissions,
            attempt_timeout,
        }
    }

    #[must_use]
    pub const fn identity(&self) -> &RemoteControlPairingIdentity {
        &self.identity
    }

    #[must_use]
    pub fn endpoint(&self) -> RemoteControlPairingEndpoint {
        self.identity.endpoint()
    }

    #[must_use]
    pub const fn window(&self) -> &RemoteControlPairingWindow {
        &self.window
    }

    #[must_use]
    pub const fn permissions(&self) -> &RemoteControlPairingPermissions {
        &self.permissions
    }

    #[must_use]
    pub const fn attempt_timeout(&self) -> RemoteControlPairingAttemptTimeout {
        self.attempt_timeout
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlPairingIdentity,
        RemoteControlPairingWindow,
        RemoteControlPairingPermissions,
        RemoteControlPairingAttemptTimeout,
    ) {
        (
            self.identity,
            self.window,
            self.permissions,
            self.attempt_timeout,
        )
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum RemoteControlPairingPhase {
    #[default]
    Unavailable,
    Closed {
        target_identity: IdentityHash,
    },
    Open {
        target_identity: IdentityHash,
        session: RemoteControlPairingSession,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlPairingView<'a> {
    Unavailable,
    Closed,
    Open(&'a RemoteControlPairingSession),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteControlPairingState {
    phase: RemoteControlPairingPhase,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum OpenRemoteControlPairingOutcome {
    Opened,
    Unavailable {
        unopened: RemoteControlPairingSession,
    },
    AlreadyOpen {
        unopened: RemoteControlPairingSession,
    },
    DeadlineElapsed {
        unopened: RemoteControlPairingSession,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum CloseRemoteControlPairingOutcome {
    Closed {
        session: RemoteControlPairingSession,
    },
    AlreadyClosed,
    Unavailable,
}

impl RemoteControlPairingState {
    #[must_use]
    pub(crate) const fn available(target_identity: IdentityHash) -> Self {
        Self {
            phase: RemoteControlPairingPhase::Closed { target_identity },
        }
    }

    #[must_use]
    pub(crate) const fn open_session(&self) -> Option<OpenRemoteControlPairingSession<'_>> {
        match &self.phase {
            RemoteControlPairingPhase::Open {
                target_identity,
                session,
            } => Some(OpenRemoteControlPairingSession {
                target_identity: *target_identity,
                session,
            }),
            RemoteControlPairingPhase::Unavailable | RemoteControlPairingPhase::Closed { .. } => {
                None
            }
        }
    }

    #[must_use]
    pub(crate) const fn view(&self) -> RemoteControlPairingView<'_> {
        match &self.phase {
            RemoteControlPairingPhase::Unavailable => RemoteControlPairingView::Unavailable,
            RemoteControlPairingPhase::Closed { .. } => RemoteControlPairingView::Closed,
            RemoteControlPairingPhase::Open { session, .. } => {
                RemoteControlPairingView::Open(session)
            }
        }
    }

    pub(crate) fn open(
        &mut self,
        session: RemoteControlPairingSession,
        now: InstantMillis,
    ) -> OpenRemoteControlPairingOutcome {
        match self.phase {
            RemoteControlPairingPhase::Unavailable => {
                OpenRemoteControlPairingOutcome::Unavailable { unopened: session }
            }
            RemoteControlPairingPhase::Closed { target_identity }
                if now < session.window.expires_at =>
            {
                self.phase = RemoteControlPairingPhase::Open {
                    target_identity,
                    session,
                };
                OpenRemoteControlPairingOutcome::Opened
            }
            RemoteControlPairingPhase::Closed { .. } => {
                OpenRemoteControlPairingOutcome::DeadlineElapsed { unopened: session }
            }
            RemoteControlPairingPhase::Open { .. } => {
                OpenRemoteControlPairingOutcome::AlreadyOpen { unopened: session }
            }
        }
    }

    pub(crate) fn close(&mut self) -> CloseRemoteControlPairingOutcome {
        match core::mem::take(&mut self.phase) {
            RemoteControlPairingPhase::Unavailable => CloseRemoteControlPairingOutcome::Unavailable,
            RemoteControlPairingPhase::Closed { target_identity } => {
                self.phase = RemoteControlPairingPhase::Closed { target_identity };
                CloseRemoteControlPairingOutcome::AlreadyClosed
            }
            RemoteControlPairingPhase::Open {
                target_identity,
                session,
            } => {
                self.phase = RemoteControlPairingPhase::Closed { target_identity };
                CloseRemoteControlPairingOutcome::Closed { session }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_control::RemoteControlRequestKind;
    use crate::routing::announce::expand_name;
    use crate::units::DurationMillis;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    fn session(identity_fill: u8, opened_at: u64, expires_at: u64) -> RemoteControlPairingSession {
        RemoteControlPairingSession::new(
            RemoteControlPairingIdentity::new(IdentityHash::new(
                [identity_fill; TRUNCATED_HASH_BYTE_LEN],
            )),
            RemoteControlPairingWindow::new(InstantMillis(opened_at), InstantMillis(expires_at))
                .unwrap(),
            RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap(),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(500)).unwrap(),
        )
    }

    #[test]
    fn pairing_uses_its_own_canonical_destination_name() {
        assert_eq!(
            expand_name(
                REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
                REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
            ),
            Ok(REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH),
        );
        let identity =
            RemoteControlPairingIdentity::new(IdentityHash::new([0x41; TRUNCATED_HASH_BYTE_LEN]));
        assert_eq!(
            identity.endpoint().destination_hash(),
            derive_destination_hash(
                &identity.identity_hash(),
                &REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH,
            ),
        );
    }

    #[test]
    fn pairing_permissions_cannot_be_empty() {
        assert_eq!(
            RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::empty()),
            Err(RemoteControlPairingPermissionsError::NoPermittedRequests),
        );
        let permitted = RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf);
        assert_eq!(
            RemoteControlPairingPermissions::try_from(permitted)
                .as_ref()
                .map(RemoteControlPairingPermissions::permitted_requests),
            Ok(&permitted),
        );
    }

    #[test]
    fn pairing_windows_require_a_future_deadline() {
        for expires_at in [InstantMillis(999), InstantMillis(1_000)] {
            assert_eq!(
                RemoteControlPairingWindow::new(InstantMillis(1_000), expires_at),
                Err(RemoteControlPairingWindowError::DeadlineNotFuture {
                    opened_at: InstantMillis(1_000),
                    expires_at,
                }),
            );
        }
        assert_eq!(
            RemoteControlPairingWindow::new(InstantMillis(1_000), InstantMillis(1_001))
                .as_ref()
                .map(RemoteControlPairingWindow::expires_at),
            Ok(InstantMillis(1_001)),
        );
    }

    #[test]
    fn unavailable_pairing_is_distinct_from_an_available_closed_window() {
        let mut unavailable = RemoteControlPairingState::default();
        assert_eq!(unavailable.view(), RemoteControlPairingView::Unavailable);
        assert_eq!(
            unavailable.close(),
            CloseRemoteControlPairingOutcome::Unavailable,
        );
        assert_eq!(
            unavailable.open(session(0x50, 1_000, 2_000), InstantMillis(1_000)),
            OpenRemoteControlPairingOutcome::Unavailable {
                unopened: session(0x50, 1_000, 2_000),
            },
        );

        let target = IdentityHash::new([0xA1; TRUNCATED_HASH_BYTE_LEN]);
        let mut state = RemoteControlPairingState::available(target);
        assert_eq!(state.view(), RemoteControlPairingView::Closed);
        assert_eq!(
            state.close(),
            CloseRemoteControlPairingOutcome::AlreadyClosed,
        );

        let open_session = session(0x51, 1_000, 2_000);
        let endpoint = open_session.endpoint();
        assert_eq!(
            state.open(open_session, InstantMillis(1_000)),
            OpenRemoteControlPairingOutcome::Opened,
        );
        assert_eq!(state.open_session().unwrap().target_identity(), target);
        assert_eq!(
            state.close(),
            CloseRemoteControlPairingOutcome::Closed {
                session: session(0x51, 1_000, 2_000),
            },
        );
        assert_eq!(state.view(), RemoteControlPairingView::Closed);
        assert_eq!(endpoint, session(0x51, 1_000, 2_000).endpoint(),);
    }

    #[test]
    fn opening_twice_keeps_the_live_session_and_returns_the_unopened_one() {
        let mut state = RemoteControlPairingState::available(IdentityHash::new(
            [0xA2; TRUNCATED_HASH_BYTE_LEN],
        ));
        let live = session(0x61, 1_000, 2_000);
        let live_endpoint = live.endpoint();
        let unopened = session(0x62, 1_000, 3_000);

        assert_eq!(
            state.open(live, InstantMillis(1_000)),
            OpenRemoteControlPairingOutcome::Opened,
        );
        assert_eq!(
            state.open(unopened, InstantMillis(1_000)),
            OpenRemoteControlPairingOutcome::AlreadyOpen {
                unopened: session(0x62, 1_000, 3_000),
            },
        );
        assert_eq!(
            state.view(),
            RemoteControlPairingView::Open(&session(0x61, 1_000, 2_000)),
        );
        assert_eq!(live_endpoint, session(0x61, 1_000, 2_000).endpoint(),);
    }

    #[test]
    fn opening_after_the_prepared_session_deadline_returns_it_without_opening() {
        let mut state = RemoteControlPairingState::available(IdentityHash::new(
            [0xA3; TRUNCATED_HASH_BYTE_LEN],
        ));

        assert_eq!(
            state.open(session(0x63, 1_000, 2_000), InstantMillis(2_000)),
            OpenRemoteControlPairingOutcome::DeadlineElapsed {
                unopened: session(0x63, 1_000, 2_000),
            },
        );
        assert_eq!(state.view(), RemoteControlPairingView::Closed);
    }
}
