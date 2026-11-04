//! Delete fences are constant-size facts. Physical cleanup advances over an
//! exclusive persisted key, visiting a bounded prefix and at most one namespace.
use super::*;
use std::ops::Bound;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Cursor {
    group: [u8; 32],
    generation: u32,
}

impl Cursor {
    pub(super) fn from_key(key: keys::IncarnationKey) -> Result<Self, EnrollmentError> {
        let (group, generation) = keys::split_incarnation(&key)?;
        Ok(Self { group, generation })
    }
    fn key(self) -> keys::IncarnationKey {
        keys::incarnation(self.group, self.generation)
    }
}

pub(in crate::enrollment) struct Plan {
    pub after: Option<Cursor>,
    pub target: Option<(keys::IncarnationKey, rows::RetiredIncarnation)>,
}

impl Read {
    pub(super) fn inspect_purge_cursor(&self) -> Result<(), EnrollmentError> {
        if let Some(after) = self.header.purge_after {
            self.incarnation(after.key())?
                .ok_or(EnrollmentError::Corrupt)?;
        }
        Ok(())
    }
    pub(in crate::enrollment) fn generation_deleted(
        &self,
        group: [u8; 32],
        generation: u32,
    ) -> Result<bool, EnrollmentError> {
        let current = self.current(group)?.ok_or(EnrollmentError::Corrupt)?;
        Ok(current.purge_through.is_some_and(|last| generation <= last))
    }

    pub(in crate::enrollment) fn purge_plan(&self) -> Result<Plan, EnrollmentError> {
        let after = self.header.purge_after.map(Cursor::key);
        let lower = after
            .as_ref()
            .map_or(Bound::Excluded(b"".as_slice()), |key| {
                Bound::Excluded(key.as_slice())
            });
        let mut last = self.header.purge_after;
        let limit = usize::try_from(self.header.limits.max_maintenance_rows)
            .map_err(|_| EnrollmentError::Capacity)?;
        let mut visited = 0;
        for row in self
            .incarnations
            .range::<&[u8]>((lower, Bound::Unbounded))
            .map_err(db_error)?
            .take(limit)
        {
            visited += 1;
            let (key, _) = row.map_err(db_error)?;
            let key: keys::IncarnationKey = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let (group, generation) = keys::split_incarnation(&key)?;
            let row = self.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
            last = Some(Cursor::from_key(key)?);
            let target = match row {
                rows::Incarnation::Retired(row) if !row.purged => Some(row),
                rows::Incarnation::Live(entry) | rows::Incarnation::Archived(entry)
                    if self.generation_deleted(group, generation)? =>
                {
                    Some(rows::RetiredIncarnation::from_entry(&entry, false))
                }
                _ => None,
            };
            if let Some(target) = target {
                return Ok(Plan {
                    after: last,
                    target: Some((key, target)),
                });
            }
        }
        Ok(Plan {
            after: if visited == limit { last } else { None },
            target: None,
        })
    }
}
