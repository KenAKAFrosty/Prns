//! Exact ownership of the already-declared protocol work identifiers. This
//! index routes completion to one source row; it neither stores frames nor
//! adds a queue. Inactive invitation keys remain until that invitation retires.
use super::*;

#[cfg(test)]
std::thread_local! { static DECODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn decoded() {
    DECODED.with(|n| n.set(n.get() + 1));
}
#[cfg(test)]
pub(super) fn take() -> usize {
    DECODED.with(|n| n.replace(0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) enum WorkSource {
    Invitation([u8; 32]),
    Departure([u8; 32]),
    Sync([u8; 32]),
    Notice([u8; 32]),
}

pub(super) fn invitation_ids(id: [u8; 32]) -> [[u8; 32]; 5] {
    [
        b"offer".as_slice(),
        b"cancel",
        b"join",
        b"joined",
        b"decline",
    ]
    .map(|domain| work_id(domain, &id))
}
pub(super) fn invitation_work_removal_cost() -> (usize, usize) {
    let ids = invitation_ids([0; 32]);
    (ids.len(), std::mem::size_of_val(&ids))
}
pub(super) fn sync_id(local: [u8; 16], current: &rows::Current) -> Option<[u8; 32]> {
    (current.visible
        && current.owner != local
        && current.lifecycle == GroupLifecycle::Active
        && current.departure.is_none())
    .then_some(current.head)
    .flatten()
    .map(|head| work_id(b"sync", &head))
}

impl Read {
    pub(in crate::enrollment) fn work_source(
        &self,
        id: [u8; 32],
    ) -> Result<Option<WorkSource>, EnrollmentError> {
        let Some(value) = self.work_ids.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let source: WorkSource = rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
        let valid = match source {
            WorkSource::Invitation(invitation) => {
                invitation_ids(invitation).contains(&id) && self.invitation(invitation)?.is_some()
            }
            WorkSource::Departure(departure) => {
                work_id(b"leave", &departure) == id && self.departure(departure)?.is_some()
            }
            WorkSource::Sync(group) => self
                .current(group)?
                .is_some_and(|current| sync_id(self.header.local, &current) == Some(id)),
            WorkSource::Notice(notice) => notice == id && self.notice(notice)?.is_some(),
        };
        if !valid {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(source))
    }

    fn require_work(&self, id: [u8; 32], source: WorkSource) -> Result<(), EnrollmentError> {
        // Strict cold validation already visits/validates each primary. Decode
        // the exact index row here without recursively re-reading its payload.
        let value = self
            .work_ids
            .get(id.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        let actual: WorkSource = rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
        if actual != source {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(())
    }

    pub(super) fn inspect_work(&self) -> Result<(), EnrollmentError> {
        let mut expected = 0u64;
        for (table, source) in [
            (
                &self.invitations,
                WorkSource::Invitation as fn([u8; 32]) -> WorkSource,
            ),
            (&self.departures, WorkSource::Departure),
            (&self.notices, WorkSource::Notice),
            (&self.current, WorkSource::Sync),
        ] {
            for row in table
                .range::<&[u8]>((
                    std::ops::Bound::Excluded(b"".as_slice()),
                    std::ops::Bound::Unbounded,
                ))
                .map_err(db_error)?
            {
                let (key, value) = row.map_err(db_error)?;
                let id: [u8; 32] = key
                    .value()
                    .try_into()
                    .map_err(|_| EnrollmentError::Corrupt)?;
                match source(id) {
                    WorkSource::Invitation(invitation) => {
                        for work in invitation_ids(invitation) {
                            self.require_work(work, WorkSource::Invitation(invitation))?;
                            expected = expected.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                        }
                    }
                    WorkSource::Departure(departure) => {
                        self.require_work(
                            work_id(b"leave", &departure),
                            WorkSource::Departure(departure),
                        )?;
                        expected = expected.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                    }
                    WorkSource::Notice(notice) => {
                        self.require_work(notice, WorkSource::Notice(notice))?;
                        expected = expected.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                    }
                    WorkSource::Sync(group) => {
                        let current =
                            rows::decode_current(value.value(), self.header.limits.row_bytes()?)?;
                        if let Some(work) = sync_id(self.header.local, &current) {
                            self.require_work(work, WorkSource::Sync(group))?;
                            expected = expected.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                        }
                    }
                }
            }
        }
        if expected != self.header.counts.work_ids {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(())
    }
}
