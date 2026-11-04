//! Bounded read-only accounting over catalog-owned indexes. Opening the scan
//! reads the immutable header and counters; each step consumes one primary row
//! and only the exact related keys needed to interpret that row.
use super::*;
use crate::dependencies::CatalogCursor;
use crate::Dependencies;
use prns_groups_store_redb::change::Stamp;
use std::ops::Bound;

pub(in crate::enrollment) enum Step {
    Namespace { entry: Box<Entry>, archived: bool },
    CatalogRow,
    Complete,
}

impl Read {
    pub(in crate::enrollment) fn dependency_seed(
        &self,
        stamp: Stamp,
    ) -> Result<Dependencies, EnrollmentError> {
        let mut result = Dependencies::empty(stamp);
        let counts = self.header.counts;
        result.archived_incarnations = counts.archived;
        result.retired_incarnations = counts.retired;
        result.pending_departures = counts.departures;
        // Namespace receipts are projected into this same exact index. Counting
        // their namespace copies again would overstate retained results.
        result.retained_control_results = counts.requests;
        result.passive_fences = counts.retired;
        add(&mut result.passive_fences, counts.cancellations)?;
        add(
            &mut result.passive_fences,
            u64::from(self.header.request_retired_before != 0),
        )?;
        Ok(result)
    }

    pub(in crate::enrollment) fn dependency_step(
        &self,
        cursor: &mut CatalogCursor,
        report: &mut Dependencies,
    ) -> Result<Step, EnrollmentError> {
        // Empty phases can be skipped without consuming a work slot. There are
        // only five phases, so this loop never walks an unbounded empty range.
        loop {
            match *cursor {
                CatalogCursor::Incarnations(after) => {
                    let Some(key) = next_key(&self.incarnations, after)? else {
                        *cursor = CatalogCursor::Invitations(None);
                        continue;
                    };
                    let row = self.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
                    *cursor = CatalogCursor::Incarnations(Some(key));
                    return Ok(match row {
                        rows::Incarnation::Live(entry) | rows::Incarnation::Archived(entry)
                            if self.generation_deleted(entry.group, entry.generation)? =>
                        {
                            add(&mut report.pending_local_purges, 1)?;
                            Step::CatalogRow
                        }
                        rows::Incarnation::Live(entry) => Step::Namespace {
                            entry: Box::new(entry),
                            archived: false,
                        },
                        rows::Incarnation::Archived(entry) => Step::Namespace {
                            entry: Box::new(entry),
                            archived: true,
                        },
                        rows::Incarnation::Retired(entry) => {
                            add(&mut report.pending_local_purges, u64::from(!entry.purged))?;
                            Step::CatalogRow
                        }
                    });
                }
                CatalogCursor::Invitations(after) => {
                    let Some(id) = next_key(&self.invitations, after)? else {
                        *cursor = CatalogCursor::Notices(None);
                        continue;
                    };
                    let invite = self.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
                    let outbound = invite.offer.owner.rns == self.header.local;
                    let pending = if outbound {
                        !invite.acknowledged
                            && matches!(
                                invite.state,
                                InvitationState::Pending | InvitationState::Cancelled
                            )
                    } else {
                        invite.state == InvitationState::JoinRequested
                            || (!invite.acknowledged && invite.state == InvitationState::Declined)
                    };
                    add(&mut report.pending_enrollment, u64::from(pending))?;
                    *cursor = CatalogCursor::Invitations(Some(id));
                    return Ok(Step::CatalogRow);
                }
                CatalogCursor::Notices(after) => {
                    let Some(id) = next_key(&self.notices, after)? else {
                        *cursor = CatalogCursor::Terminals(None);
                        continue;
                    };
                    let notice = self.notice(id)?.ok_or(EnrollmentError::Corrupt)?;
                    add(
                        &mut report.pending_terminal_notices,
                        u64::from(!notice.acknowledged),
                    )?;
                    *cursor = CatalogCursor::Notices(Some(id));
                    return Ok(Step::CatalogRow);
                }
                CatalogCursor::Terminals(after) => {
                    let Some(id) = next_key(&self.terminals, after)? else {
                        *cursor = CatalogCursor::Audience(None);
                        continue;
                    };
                    let row = self.terminal(id)?.ok_or(EnrollmentError::Corrupt)?;
                    add(
                        &mut report.passive_fences,
                        u64::from(matches!(row.proof.statement.kind, TerminalKind::Closed)),
                    )?;
                    *cursor = CatalogCursor::Terminals(Some(id));
                    return Ok(Step::CatalogRow);
                }
                CatalogCursor::Audience(after) => {
                    let Some(key) = next_key(&self.audience, after)? else {
                        *cursor = CatalogCursor::Complete;
                        continue;
                    };
                    let group = key[..32].try_into().map_err(|_| EnrollmentError::Corrupt)?;
                    let recipient = key[32..].try_into().map_err(|_| EnrollmentError::Corrupt)?;
                    let audience = self
                        .audience(group, recipient)?
                        .ok_or(EnrollmentError::Corrupt)?;
                    if audience.close_notice_funded {
                        if let Some(terminal) =
                            self.terminal_subject(group, &TerminalKind::Closed)?
                        {
                            if terminal.proof.statement.owner.rns != self.header.local
                                || terminal.complete
                                || terminal.after.is_some_and(|after| recipient <= after)
                            {
                                return Err(EnrollmentError::Corrupt);
                            }
                            // The funded obligation exists before its notice
                            // row is materialized by bounded Close fan-out.
                            add(&mut report.pending_terminal_notices, 1)?;
                        }
                    }
                    *cursor = CatalogCursor::Audience(Some(key));
                    return Ok(Step::CatalogRow);
                }
                CatalogCursor::Complete => return Ok(Step::Complete),
            }
        }
    }
}

fn next_key<const N: usize>(
    table: &ReadOnlyTable<&[u8], &[u8]>,
    after: Option<[u8; N]>,
) -> Result<Option<[u8; N]>, EnrollmentError> {
    let start = after
        .as_ref()
        .map_or(Bound::Excluded(b"".as_slice()), |key| {
            Bound::Excluded(key.as_slice())
        });
    let mut range = table
        .range::<&[u8]>((start, Bound::Unbounded))
        .map_err(db_error)?;
    let Some(row) = range.next() else {
        return Ok(None);
    };
    let (key, _) = row.map_err(db_error)?;
    key.value()
        .try_into()
        .map(Some)
        .map_err(|_| EnrollmentError::Corrupt)
}

fn add(target: &mut u64, count: u64) -> Result<(), EnrollmentError> {
    *target = target.checked_add(count).ok_or(EnrollmentError::Corrupt)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn catalog_scan_decodes_one_candidate_per_step_and_rejects_late_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("dependencies.redb")).unwrap();
        let local = IdentityHash::new([1; 16]);
        let incarnation = StoreIncarnation::new([3; 24]).unwrap();
        let tx = db.begin_write().unwrap();
        initialize_in(&tx, local, CatalogLimits::default()).unwrap();
        prns_groups_store_redb::change::initialize_in(&tx, incarnation).unwrap();
        tx.commit().unwrap();
        let owner = Member::new(
            IdentityHash::new([2; 16]),
            LocalIdentity::generate([2; 32], 1).unwrap().binding(),
        );
        for n in 1..=3 {
            let invite = Invite {
                admission_revision: 0,
                offer: Offer {
                    id: [n; 32],
                    group: [9; 32],
                    title: "Offer".into(),
                    profile: Default::default(),
                    owner: owner.clone(),
                    recipient: local.into_bytes(),
                    created: 1_800_000_000,
                    expires: 1_800_000_060,
                },
                state: InvitationState::Declined,
                acknowledged: false,
                terminal_at: Some(1_800_000_001),
                deferred_join: None,
                dismissed: false,
            };
            let read = Read::open(&db.begin_read().unwrap(), local).unwrap();
            let write = Edit::new(&read)
                .unwrap()
                .put_invitation(&read, &invite)
                .unwrap()
                .finish()
                .unwrap();
            let tx = db.begin_write().unwrap();
            write.stage(&tx, local).unwrap();
            prns_groups_store_redb::change::commit(tx, None).unwrap();
        }
        let stamp = prns_groups_store_redb::change::read(&db).unwrap().unwrap();
        let read = Read::open(&db.begin_read().unwrap(), local).unwrap();
        read.inspect().unwrap();
        work::take();
        let mut report = read.dependency_seed(stamp).unwrap();
        assert_eq!(work::take(), 0, "seeding must not decode catalog rows");
        assert_eq!(report.pending_enrollment, 0);
        let mut cursor = CatalogCursor::Incarnations(None);
        for count in 1..=2 {
            assert!(matches!(
                read.dependency_step(&mut cursor, &mut report).unwrap(),
                Step::CatalogRow
            ));
            assert_eq!(report.pending_enrollment, count);
            assert_eq!(work::take(), 1, "each step decodes only its candidate");
        }
        // Physical damage after cold validation is discovered only when its
        // exact key is visited; earlier bounded steps did not read that row.
        let tx = db.begin_write().unwrap();
        tx.open_table(INVITATIONS)
            .unwrap()
            .insert([3; 32].as_slice(), b"null".as_slice())
            .unwrap();
        tx.commit().unwrap();
        let read = Read::open(&db.begin_read().unwrap(), local).unwrap();
        let before = (cursor, report);
        assert!(read.dependency_step(&mut cursor, &mut report).is_err());
        assert_eq!((cursor, report), before);
        assert_eq!(
            prns_groups_store_redb::change::read(&db).unwrap(),
            Some(stamp)
        );
    }
}
