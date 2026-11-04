//! Bounded delivery selection from exact catalog work identities. Completing a
//! response resolves one indexed source and never scans every pending frame.
use super::*;
use std::ops::Bound;

impl<'db> Enrollment<'db> {
    pub fn pending(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        now: u64,
        after: Option<[u8; 32]>,
        limits: crate::PageLimits,
    ) -> Result<WorkPage, EnrollmentError> {
        let (rows, bytes) = limits.checked().map_err(|_| EnrollmentError::Invalid)?;
        let fixed = std::mem::size_of::<WorkPage>();
        // Query output is charged by actual owned bytes. A catalog's frame
        // ceiling does not require every caller to allocate that worst case.
        if fixed
            .checked_add(std::mem::size_of::<Work>())
            .is_none_or(|n| n > bytes)
        {
            return Err(EnrollmentError::Invalid);
        }
        // Selection is read-only. The owner schedules bounded maintenance
        // explicitly, independently of how many continuation pages it reads.
        let catalog = self.read()?;
        let start = after
            .as_ref()
            .map_or(Bound::Excluded(b"".as_slice()), |id| {
                Bound::Excluded(id.as_slice())
            });
        let mut items = Vec::new();
        let mut allocated = fixed;
        let mut last = after;
        let mut next = None;
        for (visited, row) in catalog
            .work_ids
            .range::<&[u8]>((start, Bound::Unbounded))
            .map_err(db_error)?
            .enumerate()
        {
            let (key, _) = row.map_err(db_error)?;
            if visited == rows {
                next = last;
                break;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            if let Some(work) = self.pending_work(owner, &catalog, id, now)? {
                let length = items
                    .len()
                    .checked_add(1)
                    .ok_or(EnrollmentError::Capacity)?;
                let dynamic = allocated
                    .checked_add(work.frame.capacity())
                    .ok_or(EnrollmentError::Capacity)?;
                if length
                    .checked_mul(std::mem::size_of::<Work>())
                    .and_then(|n| n.checked_add(dynamic))
                    .is_none_or(|n| n > bytes)
                {
                    // Never return an unchanged cursor for an oversized first
                    // active row. Earlier inactive candidates may still give
                    // this page genuine, bounded traversal progress.
                    if last == after {
                        return Err(EnrollmentError::Capacity);
                    }
                    next = last;
                    break;
                }
                if length > items.capacity() {
                    items
                        .try_reserve_exact(1)
                        .map_err(|_| EnrollmentError::Capacity)?;
                }
                if items
                    .capacity()
                    .checked_mul(std::mem::size_of::<Work>())
                    .and_then(|n| n.checked_add(dynamic))
                    .is_none_or(|n| n > bytes)
                {
                    return Err(EnrollmentError::Capacity);
                }
                allocated = dynamic;
                items.push(work);
            }
            last = Some(id);
        }
        Ok(WorkPage { items, next })
    }

    pub(super) fn pending_work(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        catalog: &Catalog,
        id: [u8; 32],
        now: u64,
    ) -> Result<Option<Work>, EnrollmentError> {
        use indexed::work::WorkSource;
        let Some(source) = catalog.work_source(id)? else {
            return Ok(None);
        };
        let selected = match source {
            WorkSource::Departure(departure) => {
                let row = catalog
                    .departure(departure)?
                    .ok_or(EnrollmentError::Corrupt)?;
                Some((IdentityHash::new(row.owner.rns), Frame::Leave(row)))
            }
            WorkSource::Notice(notice) => {
                let row = catalog.notice(notice)?.ok_or(EnrollmentError::Corrupt)?;
                if row.acknowledged {
                    None
                } else {
                    let proof = catalog
                        .terminal(row.terminal)?
                        .ok_or(EnrollmentError::Corrupt)?
                        .proof;
                    Some((IdentityHash::new(row.recipient), Frame::Terminal(proof)))
                }
            }
            WorkSource::Sync(group) => {
                let current = catalog.current(group)?.ok_or(EnrollmentError::Corrupt)?;
                if current.lifecycle != GroupLifecycle::Active
                    || current.departure.is_some()
                    || !current.visible
                {
                    None
                } else {
                    let entry = Self::find(catalog, group)?;
                    Some((
                        IdentityHash::new(entry.owner.rns),
                        Frame::Sync {
                            group,
                            head: current.head.ok_or(EnrollmentError::Corrupt)?,
                            member: entry.local.clone(),
                        },
                    ))
                }
            }
            WorkSource::Invitation(invitation) => {
                let invite = catalog
                    .invitation(invitation)?
                    .ok_or(EnrollmentError::Corrupt)?;
                if catalog.invitation_deleted(&invite)? {
                    return Ok(None);
                }
                let view = catalog.invitation_view(invite.clone(), now)?;
                if matches!(view.state, InvitationState::Cancelled)
                    && invite.state != InvitationState::Cancelled
                {
                    None
                } else if invite.offer.owner.rns == self.local.into_bytes() {
                    if invite.state == InvitationState::Cancelled
                        && !invite.acknowledged
                        && id == work_id(b"cancel", &invitation)
                    {
                        Some((
                            IdentityHash::new(invite.offer.recipient),
                            Frame::Cancel {
                                invitation,
                                group: invite.offer.group,
                                expires: invite.offer.expires,
                            },
                        ))
                    } else if invite.state == InvitationState::Pending
                        && !invite.acknowledged
                        && now.max(catalog.header.invitation_time_floor) < invite.offer.expires
                        && id == work_id(b"offer", &invitation)
                    {
                        Some((
                            IdentityHash::new(invite.offer.recipient),
                            Frame::Offer(invite.offer),
                        ))
                    } else {
                        None
                    }
                } else if invite.state == InvitationState::JoinRequested {
                    let Some(entry) = catalog.entry(invite.offer.group)? else {
                        return Ok(None);
                    };
                    if entry.invitation != Some(invitation) {
                        return Ok(None);
                    }
                    self.adapter(owner, &entry, None, |_, snapshot| {
                        if snapshot.ready()?
                            && !invite.acknowledged
                            && id == work_id(b"joined", &invitation)
                        {
                            Ok(Some((
                                IdentityHash::new(entry.owner.rns),
                                Frame::Joined {
                                    invitation,
                                    head: snapshot
                                        .current
                                        .as_ref()
                                        .ok_or(EnrollmentError::Corrupt)?
                                        .head()?,
                                },
                            )))
                        } else if snapshot.current.is_none()
                            && snapshot.lifecycle == GroupLifecycle::Active
                            && id == work_id(b"join", &invitation)
                        {
                            let Some(package) = snapshot.package.as_ref() else {
                                return Ok(None);
                            };
                            Ok(Some((
                                IdentityHash::new(entry.owner.rns),
                                Frame::Join {
                                    invitation,
                                    profile: profile_digest(entry.profile)?,
                                    member: entry.local.clone(),
                                    package: package.clone(),
                                },
                            )))
                        } else {
                            Ok(None)
                        }
                    })?
                } else if invite.state == InvitationState::Declined
                    && !invite.acknowledged
                    && id == work_id(b"decline", &invitation)
                {
                    Some((
                        IdentityHash::new(invite.offer.owner.rns),
                        Frame::Decline { invitation },
                    ))
                } else {
                    None
                }
            }
        };
        selected
            .map(|(recipient, frame)| {
                Ok(Work {
                    id,
                    recipient,
                    frame: self.frame(&frame)?,
                })
            })
            .transpose()
    }
}
