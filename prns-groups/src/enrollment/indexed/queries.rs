//! Canonical read projections from exact catalog rows. These never initialize a
//! namespace, perform invitation maintenance, or load a service/MLS roster.
use super::*;
use crate::{
    ConversationCapabilities, ConversationPage, ConversationSummary, InvitationId, InvitationPage,
    PageLimits,
};
use std::ops::Bound;

impl Enrollment<'_> {
    pub fn conversation_summary(
        &self,
        group: GroupId,
    ) -> Result<Option<ConversationSummary>, EnrollmentError> {
        let read = Read::open(&self.database.begin_read().map_err(db_error)?, self.local)?;
        read.current(group.0)?
            .map(|row| summary(self.local, group.0, row))
            .transpose()
            .map(Option::flatten)
    }

    pub fn conversation_summaries(
        &self,
        after: Option<GroupId>,
        limits: PageLimits,
    ) -> Result<ConversationPage, EnrollmentError> {
        let read = Read::open(&self.database.begin_read().map_err(db_error)?, self.local)?;
        let mut buffer = Buffer::<ConversationSummary>::new(
            limits,
            profile::MAX_TITLE_BYTES,
            std::mem::size_of::<ConversationPage>(),
        )?;
        let start = after
            .as_ref()
            .map_or(Bound::Excluded(b"".as_slice()), |id| {
                Bound::Excluded(id.0.as_slice())
            });
        let mut next = None;
        let mut last = None;
        for (visited, row) in read
            .current
            .range::<&[u8]>((start, Bound::Unbounded))
            .map_err(db_error)?
            .enumerate()
        {
            let (key, value) = row.map_err(db_error)?;
            if visited == buffer.max_rows {
                next = last;
                break;
            }
            let group = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let current = rows::decode_current(value.value(), read.header.limits.row_bytes()?)?;
            if let Some(item) = summary(self.local, group, current)? {
                let dynamic = item.title.capacity();
                if !buffer.push(item, dynamic)? {
                    next = last;
                    break;
                }
            }
            last = Some(GroupId(group));
        }
        Ok(ConversationPage {
            items: buffer.items,
            next,
        })
    }

    pub fn managed_profile(
        &self,
        group: GroupId,
    ) -> Result<Option<crate::GroupProfile>, EnrollmentError> {
        let read = Read::open(&self.database.begin_read().map_err(db_error)?, self.local)?;
        let Some(current) = read.current(group.0)?.filter(|row| row.visible) else {
            return Ok(None);
        };
        match read
            .incarnation(keys::incarnation(group.0, current.generation))?
            .ok_or(EnrollmentError::Corrupt)?
        {
            rows::Incarnation::Live(entry) => Ok(Some(entry.profile)),
            _ => Err(EnrollmentError::Corrupt),
        }
    }

    /// One exact catalog row with the same visibility and time projection as
    /// invitation pages. No namespace, maintenance or range traversal is needed.
    pub fn invitation(
        &self,
        id: InvitationId,
        now: u64,
    ) -> Result<Option<Invitation>, EnrollmentError> {
        let read = Read::open(&self.database.begin_read().map_err(db_error)?, self.local)?;
        let Some(invite) = read.invitation(id.0)? else {
            return Ok(None);
        };
        if invite.dismissed || read.invitation_deleted(&invite)? {
            return Ok(None);
        }
        read.invitation_view(invite, now).map(Some)
    }

    pub fn invitations_page(
        &self,
        now: u64,
        after: Option<InvitationId>,
        limits: PageLimits,
    ) -> Result<InvitationPage, EnrollmentError> {
        let read = Read::open(&self.database.begin_read().map_err(db_error)?, self.local)?;
        let now = now.max(read.header.invitation_time_floor);
        let mut buffer = Buffer::<Invitation>::new(
            limits,
            profile::MAX_TITLE_BYTES,
            std::mem::size_of::<InvitationPage>(),
        )?;
        let start = after
            .as_ref()
            .map_or(Bound::Excluded(b"".as_slice()), |id| {
                Bound::Excluded(id.0.as_slice())
            });
        let mut next = None;
        let mut last = None;
        for (visited, row) in read
            .invitations
            .range::<&[u8]>((start, Bound::Unbounded))
            .map_err(db_error)?
            .enumerate()
        {
            let (key, _) = row.map_err(db_error)?;
            if visited == buffer.max_rows {
                next = last;
                break;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let invite = read.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
            if !invite.dismissed && !read.invitation_deleted(&invite)? {
                let item = invitation(&read, invite, now)?;
                let dynamic = item.title.capacity();
                if !buffer.push(item, dynamic)? {
                    next = last;
                    break;
                }
            }
            last = Some(InvitationId(id));
        }
        Ok(InvitationPage {
            items: buffer.items,
            next,
        })
    }
}

fn summary(
    local: IdentityHash,
    group: [u8; 32],
    row: rows::Current,
) -> Result<Option<ConversationSummary>, EnrollmentError> {
    if !row.visible {
        return Ok(None);
    }
    let owner = Some(IdentityHash::new(row.owner));
    let members = u64::from(row.member_count);
    let lifecycle = if row.departure.is_some() && row.lifecycle == GroupLifecycle::Active {
        GroupLifecycle::Leaving
    } else {
        row.lifecycle
    };
    let control = row.head.map(|head| {
        let head = prns_groups_core::ControlId(head);
        prns_groups_core::ControlReference {
            head,
            era: prns_groups_core::AuthorizationEra(head),
        }
    });
    Ok(Some(ConversationSummary {
        group: GroupId(group),
        title: row.title,
        owner_identity: owner,
        lifecycle,
        ready: row.ready,
        member_count: members,
        control,
        capabilities: ConversationCapabilities::for_state(
            owner, local, lifecycle, row.ready, members,
        ),
    }))
}

fn invitation(read: &Read, invite: Invite, now: u64) -> Result<Invitation, EnrollmentError> {
    let outgoing = invite.offer.owner.rns == read.header.local;
    let current = read.current(invite.offer.group)?;
    let binding = read.invitation_binding(invite.offer.id)?;
    let joined = binding.as_ref().is_some_and(|binding| {
        outgoing
            || current.as_ref().is_some_and(|current| {
                current.generation == binding.namespace.generation && current.head.is_some()
            })
    });
    let closed = read
        .terminal_subject(invite.offer.group, &TerminalKind::Closed)?
        .is_some();
    let removed = binding
        .as_ref()
        .map(|binding| {
            read.terminal_subject(
                invite.offer.group,
                &TerminalKind::Removed(binding.member.clone()),
            )
        })
        .transpose()?
        .flatten()
        .is_some();
    let terminal = matches!(
        invite.state,
        InvitationState::Cancelled
            | InvitationState::Expired
            | InvitationState::Declined
            | InvitationState::UnsupportedProfile
    );
    let state = if closed || removed || read.invitation_deleted(&invite)? {
        InvitationState::Cancelled
    } else if terminal {
        invite.state
    } else if joined && outgoing {
        if invite.state == InvitationState::Joined {
            InvitationState::Joined
        } else {
            InvitationState::JoinRequested
        }
    } else if joined && current.as_ref().is_some_and(|row| row.ready) {
        InvitationState::Joined
    } else if invite.state == InvitationState::Pending && now >= invite.offer.expires {
        InvitationState::Expired
    } else {
        invite.state
    };
    let waiting_for_delivery = !joined
        && matches!(
            state,
            InvitationState::Pending | InvitationState::JoinRequested
        )
        && invite.deferred_join.is_some()
        && now < invite.offer.expires;
    let can_cancel = outgoing
        && state == InvitationState::Pending
        && !joined
        && current
            .as_ref()
            .is_some_and(|row| row.visible && row.lifecycle == GroupLifecycle::Active);
    Ok(Invitation {
        id: InvitationId(invite.offer.id),
        group: GroupId(invite.offer.group),
        title: invite.offer.title.into_boxed_str().into_string(),
        profile: invite.offer.profile,
        inviter: IdentityHash::new(invite.offer.owner.rns),
        recipient: IdentityHash::new(invite.offer.recipient),
        expires_at_secs: invite.offer.expires,
        outgoing,
        state,
        waiting_for_delivery,
        can_cancel,
        can_dismiss: Enrollment::dismissible(state),
    })
}

impl Read {
    pub(in crate::enrollment) fn invitation_deleted(
        &self,
        invite: &Invite,
    ) -> Result<bool, EnrollmentError> {
        if invite.admission_revision == 0 || invite.admission_revision > self.header.revision {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(self
            .current(invite.offer.group)?
            .is_some_and(|row| invite.admission_revision <= row.invitations_deleted_through))
    }
    pub(in crate::enrollment) fn invitation_view(
        &self,
        invite: Invite,
        now: u64,
    ) -> Result<Invitation, EnrollmentError> {
        invitation(self, invite, now.max(self.header.invitation_time_floor))
    }
}

/// The two catalog projections own only one dynamic title. Share their checked
/// result allocation, while each range retains its own typed cursor semantics.
struct Buffer<T> {
    items: Vec<T>,
    max_rows: usize,
    max_bytes: usize,
    dynamic: usize,
}
impl<T> Buffer<T> {
    fn new(
        limits: PageLimits,
        max_dynamic_row: usize,
        result_bytes: usize,
    ) -> Result<Self, EnrollmentError> {
        let (max_rows, max_bytes) = limits.checked().map_err(|_| EnrollmentError::Invalid)?;
        // The returned owning record and actual collection capacities share
        // one budget. One bounded decoded row remains separate scratch.
        let max_bytes = max_bytes
            .checked_sub(result_bytes)
            .ok_or(EnrollmentError::Invalid)?;
        if std::mem::size_of::<T>()
            .checked_add(max_dynamic_row)
            .is_none_or(|n| n > max_bytes)
        {
            return Err(EnrollmentError::Invalid);
        }
        Ok(Self {
            items: Vec::new(),
            max_rows,
            max_bytes,
            dynamic: 0,
        })
    }
    fn push(&mut self, value: T, dynamic: usize) -> Result<bool, EnrollmentError> {
        let next = self
            .items
            .len()
            .checked_add(1)
            .ok_or(EnrollmentError::Capacity)?;
        let dynamic = self
            .dynamic
            .checked_add(dynamic)
            .ok_or(EnrollmentError::Capacity)?;
        if next
            .checked_mul(std::mem::size_of::<T>())
            .and_then(|n| n.checked_add(dynamic))
            .is_none_or(|n| n > self.max_bytes)
        {
            return Ok(false);
        }
        if next > self.items.capacity() {
            self.items
                .try_reserve_exact(1)
                .map_err(|_| EnrollmentError::Capacity)?;
        }
        if self
            .items
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .and_then(|n| n.checked_add(dynamic))
            .is_none_or(|n| n > self.max_bytes)
        {
            return Err(EnrollmentError::Capacity);
        }
        self.items.push(value);
        self.dynamic = dynamic;
        Ok(true)
    }
}
