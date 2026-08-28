use crate::identity::IdentityHash;
use crate::routing::announce::{derive_destination_hash, DottedNameHash};
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::{
    RemoteControlRequestSet, REMOTE_CONTROL_APPLICATION_NAME, REMOTE_CONTROL_NAMESPACE_ASPECT,
    REMOTE_CONTROL_SERVICE_ASPECT,
};

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

#[derive(Debug, PartialEq, Eq)]
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
}

impl RemoteControlPairingSession {
    #[must_use]
    pub fn new(
        identity: RemoteControlPairingIdentity,
        window: RemoteControlPairingWindow,
        permissions: RemoteControlPairingPermissions,
    ) -> Self {
        Self {
            identity,
            window,
            permissions,
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
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlPairingIdentity,
        RemoteControlPairingWindow,
        RemoteControlPairingPermissions,
    ) {
        (self.identity, self.window, self.permissions)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum RemoteControlPairingPhase {
    #[default]
    Closed,
    Open(RemoteControlPairingSession),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlPairingView<'a> {
    Closed,
    Open(&'a RemoteControlPairingSession),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemoteControlPairingState {
    phase: RemoteControlPairingPhase,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum OpenRemoteControlPairingOutcome {
    Opened,
    AlreadyOpen {
        unopened: RemoteControlPairingSession,
    },
    DeadlineElapsed {
        unopened: RemoteControlPairingSession,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum CloseRemoteControlPairingOutcome {
    Closed {
        session: RemoteControlPairingSession,
    },
    AlreadyClosed,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ExpireRemoteControlPairingOutcome {
    Expired {
        session: RemoteControlPairingSession,
    },
    NotDue {
        expires_at: InstantMillis,
    },
    AlreadyClosed,
}

impl RemoteControlPairingState {
    #[must_use]
    pub const fn view(&self) -> RemoteControlPairingView<'_> {
        match &self.phase {
            RemoteControlPairingPhase::Closed => RemoteControlPairingView::Closed,
            RemoteControlPairingPhase::Open(session) => RemoteControlPairingView::Open(session),
        }
    }

    pub fn open(
        &mut self,
        session: RemoteControlPairingSession,
        now: InstantMillis,
    ) -> OpenRemoteControlPairingOutcome {
        match self.phase {
            RemoteControlPairingPhase::Closed if now < session.window.expires_at => {
                self.phase = RemoteControlPairingPhase::Open(session);
                OpenRemoteControlPairingOutcome::Opened
            }
            RemoteControlPairingPhase::Closed => {
                OpenRemoteControlPairingOutcome::DeadlineElapsed { unopened: session }
            }
            RemoteControlPairingPhase::Open(_) => {
                OpenRemoteControlPairingOutcome::AlreadyOpen { unopened: session }
            }
        }
    }

    pub fn close(&mut self) -> CloseRemoteControlPairingOutcome {
        match core::mem::take(&mut self.phase) {
            RemoteControlPairingPhase::Closed => CloseRemoteControlPairingOutcome::AlreadyClosed,
            RemoteControlPairingPhase::Open(session) => {
                CloseRemoteControlPairingOutcome::Closed { session }
            }
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> ExpireRemoteControlPairingOutcome {
        match core::mem::take(&mut self.phase) {
            RemoteControlPairingPhase::Closed => ExpireRemoteControlPairingOutcome::AlreadyClosed,
            RemoteControlPairingPhase::Open(session) if now < session.window.expires_at => {
                let expires_at = session.window.expires_at;
                self.phase = RemoteControlPairingPhase::Open(session);
                ExpireRemoteControlPairingOutcome::NotDue { expires_at }
            }
            RemoteControlPairingPhase::Open(session) => {
                ExpireRemoteControlPairingOutcome::Expired { session }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_control::RemoteControlRequestKind;
    use crate::routing::announce::expand_name;
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
    fn pairing_is_closed_by_default_and_closing_is_total() {
        let mut state = RemoteControlPairingState::default();
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
        let mut state = RemoteControlPairingState::default();
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
        let mut state = RemoteControlPairingState::default();

        assert_eq!(
            state.open(session(0x63, 1_000, 2_000), InstantMillis(2_000)),
            OpenRemoteControlPairingOutcome::DeadlineElapsed {
                unopened: session(0x63, 1_000, 2_000),
            },
        );
        assert_eq!(state.view(), RemoteControlPairingView::Closed);
    }

    #[test]
    fn expiry_keeps_the_session_before_its_deadline_and_moves_it_out_at_the_boundary() {
        let mut state = RemoteControlPairingState::default();
        assert_eq!(
            state.open(session(0x71, 1_000, 2_000), InstantMillis(1_000)),
            OpenRemoteControlPairingOutcome::Opened,
        );
        assert_eq!(
            state.expire(InstantMillis(1_999)),
            ExpireRemoteControlPairingOutcome::NotDue {
                expires_at: InstantMillis(2_000),
            },
        );
        assert_eq!(
            state.expire(InstantMillis(2_000)),
            ExpireRemoteControlPairingOutcome::Expired {
                session: session(0x71, 1_000, 2_000),
            },
        );
        assert_eq!(state.view(), RemoteControlPairingView::Closed);
        assert_eq!(
            state.expire(InstantMillis(2_001)),
            ExpireRemoteControlPairingOutcome::AlreadyClosed,
        );
    }
}
