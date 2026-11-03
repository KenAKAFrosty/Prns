//! Allocation-free checks for directed enrollment and owner-ordered additions.
//! These checks do not authenticate a signature or transport peer. Adapters must
//! verify those proofs before treating a valid transition as authoritative.
use crate::{Endpoint, GroupId, Timestamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InvitationId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrollmentError {
    WrongGroup,
    WrongParent,
    WrongEpoch,
    InvalidRoster,
    Capacity,
    NotAnAddition,
    NotARemoval,
    OwnerRemoval,
    Expired,
    InvalidValidity,
}

/// Validate one removal against an authenticated predecessor. Remaining endpoint
/// incarnations are unchanged and the immutable owner must remain a member.
#[allow(clippy::too_many_arguments)]
pub fn validate_remove_transition(
    previous_group: GroupId,
    previous_head: [u8; 32],
    previous_epoch: u64,
    previous_members: &[Endpoint],
    next_group: GroupId,
    parent: [u8; 32],
    next_epoch: u64,
    next_members: &[Endpoint],
    owner: Endpoint,
    max_members: usize,
) -> Result<Endpoint, EnrollmentError> {
    if previous_group != next_group {
        return Err(EnrollmentError::WrongGroup);
    }
    if previous_head != parent {
        return Err(EnrollmentError::WrongParent);
    }
    if previous_epoch.checked_add(1) != Some(next_epoch) {
        return Err(EnrollmentError::WrongEpoch);
    }
    if previous_members.len() > max_members || next_members.len() > max_members {
        return Err(EnrollmentError::Capacity);
    }
    for roster in [previous_members, next_members] {
        if roster.is_empty()
            || roster
                .windows(2)
                .any(|p| p[0] >= p[1] || p[0].id == p[1].id)
        {
            return Err(EnrollmentError::InvalidRoster);
        }
    }
    if !previous_members.contains(&owner) || !next_members.contains(&owner) {
        return Err(EnrollmentError::OwnerRemoval);
    }
    if next_members.len().checked_add(1) != Some(previous_members.len())
        || next_members
            .iter()
            .any(|member| previous_members.binary_search(member).is_err())
    {
        return Err(EnrollmentError::NotARemoval);
    }
    previous_members
        .iter()
        .find(|member| next_members.binary_search(member).is_err())
        .copied()
        .ok_or(EnrollmentError::NotARemoval)
}

/// The whole interval must be current; a caller with an uncertain clock must
/// defer acceptance rather than silently extend the owner's invitation.
pub fn validate_invitation_time(
    created: Timestamp,
    expires: Timestamp,
    now_lower: Timestamp,
    now_upper: Timestamp,
    maximum_lifetime_secs: u64,
) -> Result<(), EnrollmentError> {
    if created >= expires
        || expires.0 - created.0 > maximum_lifetime_secs
        || now_lower > now_upper
        || now_lower < created
    {
        return Err(EnrollmentError::InvalidValidity);
    }
    if now_upper >= expires {
        return Err(EnrollmentError::Expired);
    }
    Ok(())
}

/// Validate the structural portion of a single-member, add-only transition.
/// Endpoints are sorted and unique, including their stable endpoint IDs. An
/// incarnation replacement is a removal plus addition and is excluded here.
#[allow(clippy::too_many_arguments)]
pub fn validate_add_transition(
    previous_group: GroupId,
    previous_head: [u8; 32],
    previous_epoch: u64,
    previous_members: &[Endpoint],
    next_group: GroupId,
    parent: [u8; 32],
    next_epoch: u64,
    next_members: &[Endpoint],
    max_members: usize,
) -> Result<Endpoint, EnrollmentError> {
    if previous_group != next_group {
        return Err(EnrollmentError::WrongGroup);
    }
    if previous_head != parent {
        return Err(EnrollmentError::WrongParent);
    }
    if previous_epoch.checked_add(1) != Some(next_epoch) {
        return Err(EnrollmentError::WrongEpoch);
    }
    if next_members.len() > max_members {
        return Err(EnrollmentError::Capacity);
    }
    for roster in [previous_members, next_members] {
        if roster.is_empty()
            || roster
                .windows(2)
                .any(|p| p[0] >= p[1] || p[0].id == p[1].id)
        {
            return Err(EnrollmentError::InvalidRoster);
        }
    }
    if previous_members.len().checked_add(1) != Some(next_members.len())
        || previous_members
            .iter()
            .any(|member| next_members.binary_search(member).is_err())
    {
        return Err(EnrollmentError::NotAnAddition);
    }
    next_members
        .iter()
        .find(|member| previous_members.binary_search(member).is_err())
        .copied()
        .ok_or(EnrollmentError::NotAnAddition)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{EndpointId, EndpointIncarnation};
    fn endpoint(id: u8) -> Endpoint {
        Endpoint {
            id: EndpointId([id; 32]),
            incarnation: EndpointIncarnation::new(1).unwrap(),
        }
    }
    #[test]
    fn additions_cannot_hide_replacements_forks_or_reordered_rosters() {
        let group = GroupId([1; 32]);
        let prior = [endpoint(1), endpoint(3)];
        let next = [endpoint(1), endpoint(2), endpoint(3)];
        let check = |parent, epoch, roster: &[Endpoint]| {
            validate_add_transition(group, [7; 32], 4, &prior, group, parent, epoch, roster, 8)
        };
        assert_eq!(check([7; 32], 5, &next), Ok(endpoint(2)));
        assert_eq!(check([8; 32], 5, &next), Err(EnrollmentError::WrongParent));
        assert_eq!(check([7; 32], 7, &next), Err(EnrollmentError::WrongEpoch));
        assert_eq!(
            check([7; 32], 5, &[endpoint(1), endpoint(2), endpoint(4)]),
            Err(EnrollmentError::NotAnAddition)
        );
        assert_eq!(
            check([7; 32], 5, &[endpoint(2), endpoint(1), endpoint(3)]),
            Err(EnrollmentError::InvalidRoster)
        );
    }
    #[test]
    fn removal_preserves_owner_and_every_remaining_incarnation() {
        let group = GroupId([1; 32]);
        let prior = [endpoint(1), endpoint(2), endpoint(3)];
        let check = |parent, epoch, roster: &[Endpoint]| {
            validate_remove_transition(
                group,
                [7; 32],
                4,
                &prior,
                group,
                parent,
                epoch,
                roster,
                endpoint(1),
                8,
            )
        };
        assert_eq!(
            check([7; 32], 5, &[endpoint(1), endpoint(3)]),
            Ok(endpoint(2))
        );
        assert_eq!(
            check([8; 32], 5, &[endpoint(1), endpoint(3)]),
            Err(EnrollmentError::WrongParent)
        );
        assert_eq!(
            check([7; 32], 6, &[endpoint(1), endpoint(3)]),
            Err(EnrollmentError::WrongEpoch)
        );
        assert_eq!(
            check([7; 32], 5, &[endpoint(2), endpoint(3)]),
            Err(EnrollmentError::OwnerRemoval)
        );
        assert_eq!(
            check([7; 32], 5, &[endpoint(1), endpoint(4)]),
            Err(EnrollmentError::NotARemoval)
        );
        assert_eq!(
            check([7; 32], 5, &[endpoint(3), endpoint(1)]),
            Err(EnrollmentError::InvalidRoster)
        );
        let replacement = Endpoint {
            incarnation: crate::EndpointIncarnation::new(2).unwrap(),
            ..endpoint(3)
        };
        assert_eq!(
            check([7; 32], 5, &[endpoint(1), replacement]),
            Err(EnrollmentError::NotARemoval)
        );
    }
    #[test]
    fn expiry_uses_the_upper_clock_bound() {
        assert_eq!(
            validate_invitation_time(
                Timestamp(1),
                Timestamp(20),
                Timestamp(18),
                Timestamp(19),
                30
            ),
            Ok(())
        );
        assert_eq!(
            validate_invitation_time(
                Timestamp(1),
                Timestamp(20),
                Timestamp(19),
                Timestamp(20),
                30
            ),
            Err(EnrollmentError::Expired)
        );
        assert_eq!(
            validate_invitation_time(
                Timestamp(20),
                Timestamp(1),
                Timestamp(20),
                Timestamp(20),
                30
            ),
            Err(EnrollmentError::InvalidValidity)
        );
    }
}
