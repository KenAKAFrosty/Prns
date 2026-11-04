use super::*;

/// Private evidence that receipt lookup, fresh control fencing and admission
/// already ran on this lane. A replay deliberately has no execution clock.
pub(super) struct Admitted {
    pub request: [u8; 32],
    pub fingerprint: [u8; 32],
    pub catalog: Catalog,
    pub replay: Option<[u8; 32]>,
    pub clock: Option<prns_groups_core::ClockInterval>,
}

impl<'db> Enrollment<'db> {
    pub(crate) fn group_scope(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
    ) -> Result<crate::OriginalScope, EnrollmentError> {
        let catalog = self.read()?;
        let entry = Self::find(&catalog, group.0)?;
        self.entry_scope(owner, &entry)
    }
    pub(super) fn entry_scope(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<crate::OriginalScope, EnrollmentError> {
        self.adapter(owner, entry, None, |_, snapshot| {
            if let Some(signed) = &snapshot.current {
                signed.verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
                let head = prns_groups_core::ControlId(signed.head()?);
                Ok(crate::OriginalScope::Control {
                    control: prns_groups_core::ControlReference {
                        head,
                        era: prns_groups_core::AuthorizationEra(head),
                    },
                    incarnation: StoreIncarnation::new(entry.incarnation)
                        .map_err(|_| EnrollmentError::Corrupt)?,
                })
            } else {
                Ok(crate::OriginalScope::Unjoined(
                    StoreIncarnation::new(entry.incarnation)
                        .map_err(|_| EnrollmentError::Corrupt)?,
                ))
            }
        })
    }
    pub(crate) fn execute_request(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        request: crate::Request,
        context: crate::ExecutionContext,
    ) -> Result<crate::Outcome, EnrollmentError> {
        self.process_request(owner, request, context, true)?
            .ok_or(EnrollmentError::Corrupt)
    }
    /// Recover only an exact durable receipt, including its admitted follow-up
    /// work. Absence never runs maintenance or admits a new domain mutation.
    pub(crate) fn recover_request(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        request: crate::Request,
    ) -> Result<Option<crate::Outcome>, EnrollmentError> {
        self.process_request(
            owner,
            request,
            crate::ExecutionContext {
                clock: prns_groups_core::ClockReading::Unknown,
            },
            false,
        )
    }
    /// Positive proof of an exact durable local request binding. This performs
    /// no maintenance or admitted follow-up work. False is not proof that an
    /// effect never happened: old receipts can have been retired or purged.
    pub fn has_retained_request(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        request: &crate::Request,
    ) -> Result<bool, EnrollmentError> {
        let fingerprint = self.request_fingerprint(request)?;
        Ok(self
            .retained_binding(owner, request.id.0, fingerprint)?
            .1
            .is_some())
    }
    pub(super) fn request_fingerprint(
        &self,
        request: &crate::Request,
    ) -> Result<[u8; 32], EnrollmentError> {
        use crate::Intent;
        request.validate().map_err(|_| EnrollmentError::Invalid)?;
        let control = request.original_scope.map(|scope| match scope {
            crate::OriginalScope::Control {
                control,
                incarnation,
            } => (
                0u8,
                control.head.0.to_vec(),
                control.era.0 .0.to_vec(),
                *incarnation.as_bytes(),
            ),
            crate::OriginalScope::Unjoined(incarnation) => {
                (1u8, Vec::new(), Vec::new(), *incarnation.as_bytes())
            }
        });
        // These are local receipt fingerprints, not signed protocol frames.
        // Group-scoped v2 inputs bind the complete captured control context.
        let bytes = match &request.intent {
            Intent::Publish(_) => return Err(EnrollmentError::Invalid),
            Intent::Create { title, profile } => {
                if title.trim().is_empty() || title.len() > profile::MAX_TITLE_BYTES {
                    return Err(EnrollmentError::Invalid);
                }
                profile.validate().map_err(|_| EnrollmentError::Invalid)?;
                encode(&("create", title, profile))?
            }
            Intent::Invite { group, recipient } => {
                if *recipient == self.local {
                    return Err(EnrollmentError::Invalid);
                }
                encode(&("invite", group.0, recipient.into_bytes(), control))?
            }
            Intent::Respond { invitation, accept } => encode(&("respond", invitation.0, accept))?,
            Intent::CancelInvitation { invitation } => encode(&("cancel", invitation.0))?,
            Intent::DismissInvitation { invitation } => encode(&("dismiss", invitation.0))?,
            Intent::RemoveMember { group, member } => encode(&(
                "remove",
                group.0,
                member.id.0,
                member.incarnation.get(),
                control,
            ))?,
            Intent::Leave { group } => encode(&("leave", group.0, control))?,
            Intent::Close { group } => encode(&("close", group.0, control))?,
            Intent::DeleteLocal { group } => encode(&("delete-local", group.0, control))?,
        };
        Ok(hash(&bytes))
    }
    fn process_request(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        request: crate::Request,
        context: crate::ExecutionContext,
        allow_new: bool,
    ) -> Result<Option<crate::Outcome>, EnrollmentError> {
        use crate::{Intent, InvitationId, Outcome};
        let fingerprint = self.request_fingerprint(&request)?;
        let class = match request.intent {
            Intent::RemoveMember { .. }
            | Intent::Leave { .. }
            | Intent::Close { .. }
            | Intent::DeleteLocal { .. } => AdmissionClass::Management,
            _ => AdmissionClass::Ordinary,
        };
        let Some(admitted) =
            self.admission_class(owner, &request, fingerprint, context, class, allow_new)?
        else {
            return Ok(None);
        };
        let outcome = match request.intent {
            Intent::Publish(_) => Err(EnrollmentError::Invalid),
            Intent::Create { title, profile } => self
                .create_admitted(owner, admitted, &title, profile)
                .map(Outcome::Created),
            Intent::Invite { group, recipient } => self
                .invite_admitted(owner, admitted, group, recipient)
                .map(|id| Outcome::Invited(InvitationId(id))),
            Intent::Respond { invitation, accept } => self
                .respond_admitted(owner, admitted, invitation.0, accept)
                .map(|()| Outcome::Saved),
            Intent::CancelInvitation { invitation } => self
                .cancel_invitation_admitted(owner, admitted, invitation.0)
                .map(|()| Outcome::Saved),
            Intent::DismissInvitation { invitation } => self
                .dismiss_invitation_admitted(owner, admitted, invitation.0)
                .map(|()| Outcome::Saved),
            Intent::RemoveMember { group, member } => self
                .remove_member_admitted(
                    owner,
                    admitted,
                    group,
                    member,
                    request
                        .original_scope
                        .and_then(crate::OriginalScope::control)
                        .ok_or(EnrollmentError::Invalid)?
                        .head
                        .0,
                )
                .map(|()| Outcome::Saved),
            Intent::Leave { group } => self
                .leave_admitted(owner, admitted, group)
                .map(|()| Outcome::Saved),
            Intent::Close { group } => self
                .close_admitted(
                    owner,
                    admitted,
                    group,
                    request
                        .original_scope
                        .and_then(crate::OriginalScope::control)
                        .ok_or(EnrollmentError::Invalid)?
                        .head
                        .0,
                )
                .map(|()| Outcome::Saved),
            Intent::DeleteLocal { group } => self
                .delete_local_admitted(owner, admitted, group)
                .map(|()| Outcome::Saved),
        };
        outcome.map(Some)
    }
}
