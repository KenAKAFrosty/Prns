//! Ordered keys for the enrollment catalog. These are local storage keys, not
//! another enrollment wire format. Fixed-width components keep range bounds
//! independent of the number of stored memberships or invitations.
use super::super::EnrollmentError;

pub(in crate::enrollment) type IncarnationKey = [u8; 36];
pub(in crate::enrollment) type GroupItemKey = [u8; 64];
pub(in crate::enrollment) type PeerItemKey = [u8; 48];
pub(in crate::enrollment) type TimeItemKey = [u8; 40];
pub(in crate::enrollment) type AudienceKey = [u8; 48];
pub(in crate::enrollment) type TerminalSubjectKey = [u8; 73];

pub(in crate::enrollment) fn terminal_subject(
    group: [u8; 32],
    kind: &super::TerminalKind,
) -> TerminalSubjectKey {
    let mut key = [0; 73];
    key[..32].copy_from_slice(&group);
    if let super::TerminalKind::Removed(member) = kind {
        key[32] = 1;
        key[33..65].copy_from_slice(&member.endpoint);
        key[65..].copy_from_slice(&member.incarnation.to_be_bytes());
    }
    key
}

pub(in crate::enrollment) fn audience(group: [u8; 32], recipient: [u8; 16]) -> AudienceKey {
    let mut key = [0; 48];
    key[..32].copy_from_slice(&group);
    key[32..].copy_from_slice(&recipient);
    key
}

pub(in crate::enrollment) fn incarnation(group: [u8; 32], generation: u32) -> IncarnationKey {
    let mut key = [0; 36];
    key[..32].copy_from_slice(&group);
    key[32..].copy_from_slice(&generation.to_be_bytes());
    key
}

pub(in crate::enrollment) fn split_incarnation(
    bytes: &[u8],
) -> Result<([u8; 32], u32), EnrollmentError> {
    let key: &IncarnationKey = bytes.try_into().map_err(|_| EnrollmentError::Corrupt)?;
    let group = key[..32].try_into().map_err(|_| EnrollmentError::Corrupt)?;
    let generation =
        u32::from_be_bytes(key[32..].try_into().map_err(|_| EnrollmentError::Corrupt)?);
    Ok((group, generation))
}

pub(in crate::enrollment) fn group_item(group: [u8; 32], item: [u8; 32]) -> GroupItemKey {
    let mut key = [0; 64];
    key[..32].copy_from_slice(&group);
    key[32..].copy_from_slice(&item);
    key
}

pub(in crate::enrollment) fn peer_item(peer: [u8; 16], item: [u8; 32]) -> PeerItemKey {
    let mut key = [0; 48];
    key[..16].copy_from_slice(&peer);
    key[16..].copy_from_slice(&item);
    key
}

pub(in crate::enrollment) fn time_item(time: u64, item: [u8; 32]) -> TimeItemKey {
    let mut key = [0; 40];
    key[..8].copy_from_slice(&time.to_be_bytes());
    key[8..].copy_from_slice(&item);
    key
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn incarnation_ranges_keep_generations_and_adjacent_groups_separate() {
        let group = [23; 32];
        let later_group = [24; 32];
        let generations = [0, 1, 255, 256, 65_535, 65_536, u32::MAX];
        let keys = generations.map(|generation| incarnation(group, generation));
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        for (key, generation) in keys.into_iter().zip(generations) {
            assert_eq!(split_incarnation(&key).unwrap(), (group, generation));
            assert!(key < incarnation(later_group, 0));
        }
        assert!(split_incarnation(&[0; 35]).is_err());
        assert!(split_incarnation(&[0; 37]).is_err());
    }

    #[test]
    fn timestamp_indexes_order_full_width_time_before_the_identifier() {
        assert!(time_item(255, [255; 32]) < time_item(256, [0; 32]));
        assert!(time_item(u64::MAX - 1, [255; 32]) < time_item(u64::MAX, [0; 32]));
        assert!(group_item([1; 32], [255; 32]) < group_item([2; 32], [0; 32]));
        assert!(peer_item([1; 16], [255; 32]) < peer_item([2; 16], [0; 32]));
    }
}
