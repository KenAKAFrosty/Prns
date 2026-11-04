//! Bounded catalog maintenance over owning ordered indexes. Cursors are durable
//! progress, not new authorization or a replacement for exact retained proofs.
use super::*;
use std::ops::Bound;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) enum Phase {
    Requests,
    Cancellations,
    Invitations,
    Outputs,
    Terminals,
    Obligations,
}
impl Phase {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Requests => Self::Cancellations,
            Self::Cancellations => Self::Invitations,
            Self::Invitations => Self::Outputs,
            Self::Outputs => Self::Terminals,
            Self::Terminals => Self::Obligations,
            Self::Obligations => Self::Requests,
        }
    }
}

pub(in crate::enrollment) struct Plan {
    pub write: write::Write,
    pub result: super::super::Maintenance,
    pub output: Option<Entry>,
}

impl Read {
    /// Each category gets a turn even when an earlier category is continuously
    /// replenished. The invitation cursor advances past retained obligations.
    /// Output cleanup selects at most one namespace; its own work is bounded by
    /// the selected namespace policy, separately from this catalog row budget.
    pub(in crate::enrollment) fn maintenance(&self, now: u64) -> Result<Plan, EnrollmentError> {
        write::Edit::new(self)?.maintain(self, now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Cursor {
    pub time: u64,
    pub id: [u8; 32],
}
impl Cursor {
    pub(super) fn key(self) -> keys::TimeItemKey {
        keys::time_item(self.time, self.id)
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, EnrollmentError> {
        let key: &keys::TimeItemKey = bytes.try_into().map_err(|_| EnrollmentError::Corrupt)?;
        Ok(Self {
            time: u64::from_be_bytes(key[..8].try_into().map_err(|_| EnrollmentError::Corrupt)?),
            id: key[8..].try_into().map_err(|_| EnrollmentError::Corrupt)?,
        })
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Incoming {
    pub invitations: u64,
    pub cancellations: u64,
}

pub(super) fn pending(local: [u8; 16], invite: &Invite) -> bool {
    invite.offer.owner.rns != local && invite.state == InvitationState::Pending && !invite.dismissed
}

pub(super) fn check_incoming_index(
    read: &Read,
    peer: [u8; 16],
    id: [u8; 32],
    invitation: bool,
    expected: bool,
) -> Result<(), EnrollmentError> {
    let key = keys::peer_item(peer, id);
    let table = if invitation {
        &read.pending_invitations
    } else {
        &read.unsolicited_cancellations
    };
    let actual = table.get(key.as_slice()).map_err(db_error)?;
    if actual.as_ref().map(|r| r.value()) != expected.then_some(id.as_slice()) {
        return Err(EnrollmentError::Corrupt);
    }
    if expected {
        let bytes = read
            .incoming_peers
            .get(peer.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        let counter: Incoming = rows::decode_row(bytes.value(), read.header.limits.row_bytes()?)?;
        let selected = if invitation {
            counter.invitations
        } else {
            counter.cancellations
        };
        if selected == 0 || selected > u64::from(read.header.limits.max_incoming_per_peer) {
            return Err(EnrollmentError::Corrupt);
        }
    }
    Ok(())
}

pub(super) fn inspect_incoming(read: &Read) -> Result<(), EnrollmentError> {
    let mut total = Incoming::default();
    for row in read
        .incoming_peers
        .range::<&[u8]>((
            std::ops::Bound::Excluded(b"".as_slice()),
            std::ops::Bound::Unbounded,
        ))
        .map_err(db_error)?
    {
        let (key, value) = row.map_err(db_error)?;
        let peer: [u8; 16] = key
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let expected: Incoming = rows::decode_row(value.value(), read.header.limits.row_bytes()?)?;
        if expected == Incoming::default() || peer == read.header.local {
            return Err(EnrollmentError::Corrupt);
        }
        for (table, count, invitation) in [
            (&read.pending_invitations, expected.invitations, true),
            (
                &read.unsolicited_cancellations,
                expected.cancellations,
                false,
            ),
        ] {
            if count > u64::from(read.header.limits.max_incoming_per_peer) {
                return Err(EnrollmentError::Corrupt);
            }
            let first = keys::peer_item(peer, [0; 32]);
            let last = keys::peer_item(peer, [255; 32]);
            let mut actual = 0u64;
            for item in table
                .range(first.as_slice()..=last.as_slice())
                .map_err(db_error)?
            {
                let (key, value) = item.map_err(db_error)?;
                let id: [u8; 32] = key
                    .value()
                    .get(16..)
                    .ok_or(EnrollmentError::Corrupt)?
                    .try_into()
                    .map_err(|_| EnrollmentError::Corrupt)?;
                if value.value() != id {
                    return Err(EnrollmentError::Corrupt);
                }
                if invitation {
                    let invite = read.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
                    if invite.offer.owner.rns != peer || !pending(read.header.local, &invite) {
                        return Err(EnrollmentError::Corrupt);
                    }
                } else {
                    let cancellation = read.cancellation(id)?.ok_or(EnrollmentError::Corrupt)?;
                    if cancellation.owner != peer || read.invitation(id)?.is_some() {
                        return Err(EnrollmentError::Corrupt);
                    }
                }
                actual = actual.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
            }
            if actual != count {
                return Err(EnrollmentError::Corrupt);
            }
        }
        total.invitations = total
            .invitations
            .checked_add(expected.invitations)
            .ok_or(EnrollmentError::Corrupt)?;
        total.cancellations = total
            .cancellations
            .checked_add(expected.cancellations)
            .ok_or(EnrollmentError::Corrupt)?;
    }
    if total.invitations != read.header.counts.pending_invitations
        || total.cancellations != read.header.counts.unsolicited_cancellations
        || total.invitations > u64::from(read.header.limits.max_incoming_invitations)
        || total.cancellations > u64::from(read.header.limits.max_incoming_invitations)
    {
        return Err(EnrollmentError::Corrupt);
    }
    Ok(())
}

/// The maintenance range is bounded by candidate count, not by successful
/// removals. A single nonretirable row must never pin the next cursor forever.
pub(super) fn start(after: Option<Cursor>) -> Option<keys::TimeItemKey> {
    after.map(Cursor::key)
}
pub(super) fn lower(after: Option<&[u8]>) -> Bound<&[u8]> {
    Bound::Excluded(after.unwrap_or(b""))
}

pub(super) enum Action {
    Keep,
    Update,
    Remove,
}

pub(super) fn invitation_action(
    read: &Read,
    invite: &mut Invite,
    now: u64,
) -> Result<Action, EnrollmentError> {
    let outgoing = invite.offer.owner.rns == read.header.local;
    let binding = read.invitation_binding(invite.offer.id)?;
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
    let deleted = read.invitation_deleted(invite)?;
    let mut changed = false;
    if matches!(
        invite.state,
        InvitationState::Pending | InvitationState::JoinRequested
    ) && (closed || removed || deleted)
    {
        invite.state = InvitationState::Cancelled;
        invite.acknowledged = true;
        invite.deferred_join = None;
        invite.terminal_at.get_or_insert(now);
        changed = true;
    }
    // Added and accepted Join are retained independently of presentation and
    // original expiry until exact terminal or durable Joined evidence exists.
    if !closed
        && !removed
        && !deleted
        && ((outgoing && binding.is_some() && invite.state != InvitationState::Joined)
            || (!outgoing && invite.state == InvitationState::JoinRequested))
    {
        return Ok(Action::Keep);
    }
    if matches!(
        invite.state,
        InvitationState::Joined | InvitationState::Expired
    ) {
        let Some(terminal) = invite.terminal_at else {
            invite.terminal_at = Some(now);
            return Ok(Action::Update);
        };
        if terminal
            .checked_add(crate::enrollment::RETRY_HORIZON_SECS)
            .is_none_or(|end| now < end)
        {
            return Ok(if changed {
                Action::Update
            } else {
                Action::Keep
            });
        }
    } else if now < invite.offer.expires {
        return Ok(if changed {
            Action::Update
        } else {
            Action::Keep
        });
    }
    Ok(Action::Remove)
}

pub(super) fn check_outgoing_index(read: &Read, invite: &Invite) -> Result<(), EnrollmentError> {
    if invite.offer.owner.rns != read.header.local {
        return Ok(());
    }
    let expected = invite.state == InvitationState::Pending
        && read
            .invitation_bindings
            .get(invite.offer.id.as_slice())
            .map_err(db_error)?
            .is_none();
    let key = keys::audience(invite.offer.group, invite.offer.recipient);
    let actual = read
        .pending_recipients
        .get(key.as_slice())
        .map_err(db_error)?;
    if expected {
        if actual.as_ref().map(|row| row.value()) != Some(invite.offer.id.as_slice()) {
            return Err(EnrollmentError::Corrupt);
        }
    } else if actual
        .as_ref()
        .is_some_and(|row| row.value() == invite.offer.id)
    {
        return Err(EnrollmentError::Corrupt);
    }
    Ok(())
}

impl Read {
    pub(in crate::enrollment) fn pending_recipient(
        &self,
        group: [u8; 32],
        peer: [u8; 16],
    ) -> Result<Option<[u8; 32]>, EnrollmentError> {
        let key = keys::audience(group, peer);
        let Some(row) = self
            .pending_recipients
            .get(key.as_slice())
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let id = row
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let invite = self.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
        if invite.offer.group != group
            || invite.offer.recipient != peer
            || invite.offer.owner.rns != self.header.local
            || invite.state != InvitationState::Pending
            || self
                .invitation_bindings
                .get(id.as_slice())
                .map_err(db_error)?
                .is_some()
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(id))
    }
}

pub(super) fn inspect_outgoing(read: &Read) -> Result<(), EnrollmentError> {
    let mut total = 0u64;
    for row in read
        .current
        .range::<&[u8]>((lower(None), Bound::Unbounded))
        .map_err(db_error)?
    {
        let (key, value) = row.map_err(db_error)?;
        let group = key
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let current = rows::decode_current(value.value(), read.header.limits.row_bytes()?)?;
        let first = keys::audience(group, [0; 16]);
        let last = keys::audience(group, [u8::MAX; 16]);
        let mut count = 0u32;
        for item in read
            .pending_recipients
            .range(first.as_slice()..=last.as_slice())
            .map_err(db_error)?
        {
            let (key, value) = item.map_err(db_error)?;
            let peer = key
                .value()
                .get(32..)
                .ok_or(EnrollmentError::Corrupt)?
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let id = read
                .pending_recipient(group, peer)?
                .ok_or(EnrollmentError::Corrupt)?;
            if value.value() != id
                || current.owner != read.header.local
                || peer == read.header.local
            {
                return Err(EnrollmentError::Corrupt);
            }
            count = count.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
        }
        if count != current.pending_offers {
            return Err(EnrollmentError::Corrupt);
        }
        total = total
            .checked_add(u64::from(count))
            .ok_or(EnrollmentError::Corrupt)?;
    }
    if total != read.header.counts.pending_recipients {
        return Err(EnrollmentError::Corrupt);
    }
    Ok(())
}
