//! Size the owning enrollment representation without materializing a maximum
//! roster or MLS message. This is a wire bound, never a storage reservation.
use super::*;

/// The Welcome contains two signed manifests and one opaque MLS message. With
/// maximum-width fields and the largest control kind it also bounds the other
/// current enrollment envelopes. Tests encode those actual variants to guard
/// that relationship whenever this private protocol changes.
pub(crate) fn required_frame_bytes(profile: crate::GroupProfile) -> Result<usize, EnrollmentError> {
    let members = usize::try_from(profile.max_members).map_err(|_| EnrollmentError::Invalid)?;
    required_frame_bytes_for_members(profile, members)
}

/// The same owning representation at an actual/prospective roster size. Keep
/// the immutable profile intact, including its serialized numeric widths.
pub(crate) fn required_frame_bytes_for_members(
    profile: crate::GroupProfile,
    members: usize,
) -> Result<usize, EnrollmentError> {
    let wire = usize::try_from(profile.max_wire_bytes).map_err(|_| EnrollmentError::Invalid)?;
    let remaining = members.checked_sub(1).ok_or(EnrollmentError::Invalid)?;
    let member = widest_member();
    let signed = widest_signed(member.clone(), profile);
    let welcome = Reply::Welcome {
        added: Added {
            member: member.clone(),
            invitation: [255; 32],
            request_hash: [255; 32],
            signed: signed.clone(),
            welcome: Vec::new(),
        },
        tip: Box::new(signed),
    };
    let fixed = encode(&Wire {
        version: WIRE_VERSION,
        payload: welcome,
    })?
    .len();
    let roster_growth = encode(&member)?
        .len()
        .checked_add(1) // Separator after each additional JSON member.
        .and_then(|row| row.checked_mul(remaining))
        .and_then(|roster| roster.checked_mul(2))
        .ok_or(EnrollmentError::Capacity)?;
    fixed
        .checked_add(roster_growth)
        .and_then(|size| size.checked_add(base64::encoded_len(wire, true)?))
        .ok_or(EnrollmentError::Capacity)
}

fn widest_member() -> Member {
    Member {
        rns: [255; 16],
        endpoint: [255; 32],
        incarnation: u64::MAX,
        key: [255; 32],
    }
}

fn widest_signed(member: Member, profile: crate::GroupProfile) -> Signed {
    Signed {
        manifest: Manifest {
            version: WIRE_VERSION,
            group: [255; 32],
            owner: [255; 16],
            // The public title limit is in UTF-8 bytes. JSON uses at most six
            // bytes per input byte; NUL attains that bound without a long title.
            title: "\0".repeat(profile::MAX_TITLE_BYTES),
            profile,
            parent: [255; 32],
            revision: u64::MAX,
            epoch: u64::MAX,
            commit_digest: [255; 32],
            kind: ControlKind::Remove(member.clone()),
            members: vec![member],
        },
        signature: vec![255; 64],
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn encoded<T: Serialize>(payload: T) -> usize {
        encode(&Wire {
            version: WIRE_VERSION,
            payload,
        })
        .unwrap()
        .len()
    }

    #[test]
    fn bound_covers_actual_enrollment_variants_and_two_full_signed_rosters() {
        for members in [1, 8, 256, 512] {
            let profile = crate::GroupProfile {
                max_members: members,
                ..crate::GroupProfile::default()
            };
            let bound = required_frame_bytes(profile).unwrap();
            let member = widest_member();
            let mut signed = widest_signed(member.clone(), profile);
            signed.manifest.members = vec![member.clone(); members as usize];
            let opaque = vec![255; profile.max_wire_bytes as usize];
            let terminal = Terminal {
                statement: TerminalStatement {
                    version: WIRE_VERSION,
                    group: [255; 32],
                    title: signed.manifest.title.clone(),
                    owner: member.clone(),
                    head: [255; 32],
                    revision: u64::MAX,
                    kind: TerminalKind::Removed(member.clone()),
                },
                signature: vec![255; 64],
            };
            let added = Added {
                member: member.clone(),
                invitation: [255; 32],
                request_hash: [255; 32],
                signed: signed.clone(),
                welcome: opaque.clone(),
            };
            assert_eq!(
                encoded(Reply::Welcome {
                    added,
                    tip: Box::new(signed.clone())
                }),
                bound
            );
            for reply in [
                Reply::Ack,
                Reply::Expired,
                Reply::Cancelled,
                Reply::UnsupportedProfile,
                Reply::Terminal(terminal.clone()),
                Reply::AwaitingDelivery,
                Reply::Control(None),
                Reply::Control(Some(Control {
                    signed: signed.clone(),
                    commit: opaque.clone(),
                })),
            ] {
                assert!(encoded(reply) <= bound);
            }
            for request in [
                Frame::Leave(Departure {
                    id: [255; 32],
                    group: [255; 32],
                    owner: member.clone(),
                    member: member.clone(),
                    known_head: [255; 32],
                }),
                Frame::Terminal(terminal),
                Frame::Cancel {
                    invitation: [255; 32],
                    group: [255; 32],
                    expires: u64::MAX,
                },
                Frame::Offer(Offer {
                    id: [255; 32],
                    group: [255; 32],
                    title: signed.manifest.title,
                    profile,
                    owner: member.clone(),
                    recipient: [255; 16],
                    created: u64::MAX,
                    expires: u64::MAX,
                }),
                Frame::Join {
                    invitation: [255; 32],
                    profile: [255; 32],
                    member: member.clone(),
                    package: opaque,
                },
                Frame::Joined {
                    invitation: [255; 32],
                    head: [255; 32],
                },
                Frame::Decline {
                    invitation: [255; 32],
                },
                Frame::Sync {
                    member,
                    group: [255; 32],
                    head: [255; 32],
                },
            ] {
                assert!(encoded(request) <= bound);
            }
        }
    }

    #[test]
    fn byte_bound_does_not_allocate_or_iterate_the_declared_member_capacity() {
        let result = required_frame_bytes(crate::GroupProfile {
            max_members: u32::MAX,
            max_wire_bytes: u32::MAX,
            ..crate::GroupProfile::default()
        });
        if usize::BITS >= 64 {
            assert!(result.unwrap() > u32::MAX as usize);
        } else {
            assert!(matches!(result, Err(EnrollmentError::Capacity)));
        }
    }
}
