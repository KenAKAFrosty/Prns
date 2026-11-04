//! Test conveniences construct the public canonical model; there is no second
//! production mutation entry point. Explicit replay tests retain Request values.
use super::*;
use crate::{ExecutionContext, Intent, InvitationId, OriginalScope, Outcome, RequestId};
impl<'db> Enrollment<'db> {
    pub(super) fn test_execute(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        intent: Intent,
        head: Option<[u8; 32]>,
        now: u64,
    ) -> Result<Outcome, EnrollmentError> {
        let original_scope = match (intent.group(), head) {
            (Some(group), Some(head)) => Some(OriginalScope::Control {
                control: prns_groups_core::ControlReference {
                    head: prns_groups_core::ControlId(head),
                    era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId(head)),
                },
                incarnation: self.group_scope(owner, group)?.incarnation(),
            }),
            (Some(group), None) => Some(self.group_scope(owner, group)?),
            _ => None,
        };
        self.execute_request(
            owner,
            crate::Request {
                id: RequestId(id),
                intent,
                original_scope,
            },
            ExecutionContext {
                clock: prns_groups_core::ClockReading::Trusted(
                    prns_groups_core::ClockInterval::new(Timestamp(now), Timestamp(now))
                        .map_err(|_| EnrollmentError::Clock)?,
                ),
            },
        )
    }
    pub(super) fn create(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        title: &str,
        now: u64,
    ) -> Result<GroupId, EnrollmentError> {
        match self.test_execute(
            owner,
            id,
            Intent::Create {
                profile: Default::default(),
                title: title.into(),
            },
            None,
            now,
        )? {
            Outcome::Created(group) => Ok(group),
            _ => Err(EnrollmentError::Corrupt),
        }
    }
    pub(super) fn invite(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        group: GroupId,
        recipient: IdentityHash,
        now: u64,
    ) -> Result<[u8; 32], EnrollmentError> {
        match self.test_execute(owner, id, Intent::Invite { group, recipient }, None, now)? {
            Outcome::Invited(id) => Ok(id.0),
            _ => Err(EnrollmentError::Corrupt),
        }
    }
    pub(super) fn respond(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        invitation: [u8; 32],
        accept: bool,
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(
            owner,
            id,
            Intent::Respond {
                invitation: InvitationId(invitation),
                accept,
            },
            None,
            now,
        )
        .map(|_| ())
    }
    pub(super) fn cancel_invitation(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        invitation: [u8; 32],
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(
            owner,
            id,
            Intent::CancelInvitation {
                invitation: InvitationId(invitation),
            },
            None,
            now,
        )
        .map(|_| ())
    }
    pub(super) fn dismiss_invitation(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        invitation: [u8; 32],
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(
            owner,
            id,
            Intent::DismissInvitation {
                invitation: InvitationId(invitation),
            },
            None,
            now,
        )
        .map(|_| ())
    }
    pub(super) fn remove_member(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        group: GroupId,
        member: Endpoint,
        head: [u8; 32],
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(
            owner,
            id,
            Intent::RemoveMember { group, member },
            Some(head),
            now,
        )
        .map(|_| ())
    }
    pub(super) fn close(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        group: GroupId,
        head: [u8; 32],
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(owner, id, Intent::Close { group }, Some(head), now)
            .map(|_| ())
    }
    pub(super) fn leave(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        group: GroupId,
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(owner, id, Intent::Leave { group }, None, now)
            .map(|_| ())
    }
    pub(super) fn delete_local(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        group: GroupId,
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.test_execute(owner, id, Intent::DeleteLocal { group }, None, now)
            .map(|_| ())
    }
}
