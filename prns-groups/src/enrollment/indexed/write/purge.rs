use super::*;

impl Edit {
    pub(in crate::enrollment) fn fence_delete(
        self,
        read: &Read,
        group: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let mut current: rows::Current = self
            .projected(CURRENT, &group, &read.current)?
            .ok_or(EnrollmentError::Missing)?;
        current.visible = false;
        current.purge_through = Some(current.generation);
        current.invitations_deleted_through = self.header.revision;
        self.put_current(read, group, &current)
    }

    pub(in crate::enrollment) fn purge_progress(
        mut self,
        after: Option<super::super::purge::Cursor>,
    ) -> Self {
        self.header.purge_after = after;
        self
    }
}
