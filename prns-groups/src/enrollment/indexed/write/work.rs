use super::super::work::WorkSource;
use super::*;

impl Edit {
    pub(in crate::enrollment) fn index_work(
        mut self,
        read: &Read,
        id: [u8; 32],
        source: WorkSource,
    ) -> Result<Self, EnrollmentError> {
        let original = read.work_ids.get(id.as_slice()).map_err(db_error)?;
        let old: Option<WorkSource> = self.projected(WORK_IDS, &id, &read.work_ids)?;
        if old.is_some_and(|old| old != source) {
            return Err(EnrollmentError::Conflict);
        }
        let value = self.encode(&source)?;
        self.replace(
            WORK_IDS,
            &id,
            original.as_ref().map(|row| row.value()),
            Some(value),
        )?;
        if old.is_none() {
            self.header.counts.work_ids = self
                .header
                .counts
                .work_ids
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn remove_work(
        mut self,
        read: &Read,
        id: [u8; 32],
        source: WorkSource,
    ) -> Result<Self, EnrollmentError> {
        let original = read.work_ids.get(id.as_slice()).map_err(db_error)?;
        let old: Option<WorkSource> = self.projected(WORK_IDS, &id, &read.work_ids)?;
        if old != Some(source) {
            return Err(EnrollmentError::Corrupt);
        }
        self.replace(
            WORK_IDS,
            &id,
            original.as_ref().map(|row| row.value()),
            None,
        )?;
        self.header.counts.work_ids = self
            .header
            .counts
            .work_ids
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        Ok(self)
    }

    pub(in crate::enrollment) fn index_invitation_work(
        mut self,
        read: &Read,
        invitation: [u8; 32],
        add: bool,
    ) -> Result<Self, EnrollmentError> {
        for id in super::super::work::invitation_ids(invitation) {
            self = if add {
                self.index_work(read, id, WorkSource::Invitation(invitation))?
            } else {
                self.remove_work(read, id, WorkSource::Invitation(invitation))?
            };
        }
        Ok(self)
    }
}
