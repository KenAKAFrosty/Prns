use super::*;
impl Snapshot {
    pub(in crate::enrollment) fn control(
        &self,
        adapter: &impl Facts,
        epoch: u64,
    ) -> Result<Option<Control>, EnrollmentError> {
        let value: Option<Control> = read_row(
            adapter,
            CONTROL,
            CONTROL_ORDER,
            &epoch.to_be_bytes(),
            self.control_count,
        )?;
        if value
            .as_ref()
            .is_some_and(|c| c.signed.manifest.epoch != epoch)
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(value)
    }
    pub(in crate::enrollment) fn control_head(
        &self,
        adapter: &impl Facts,
        head: [u8; 32],
    ) -> Result<Option<Control>, EnrollmentError> {
        let Some(epoch) = read::<u64>(adapter, &key(CONTROL_HEAD, &head))? else {
            return Ok(None);
        };
        let value = required(self.control(adapter, epoch)?)?;
        if value.signed.head()? != head {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(value))
    }
    pub(in crate::enrollment) fn control_after(
        &self,
        adapter: &impl Facts,
        parent: [u8; 32],
    ) -> Result<Option<Control>, EnrollmentError> {
        let Some(epoch) = read::<u64>(adapter, &key(CONTROL_PARENT, &parent))? else {
            return Ok(None);
        };
        let value = required(self.control(adapter, epoch)?)?;
        if value.signed.manifest.parent != parent {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(value))
    }
    pub(in crate::enrollment) fn added(
        &self,
        adapter: &impl Facts,
        id: [u8; 32],
    ) -> Result<Option<Added>, EnrollmentError> {
        let row: Option<Added> = read_row(adapter, ADDED, ADDED_ORDER, &id, self.added_count)?;
        if row.as_ref().is_some_and(|a| a.invitation != id) {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(row)
    }
    pub(in crate::enrollment) fn added_epoch(
        &self,
        adapter: &impl Facts,
        epoch: u64,
    ) -> Result<Option<Added>, EnrollmentError> {
        let Some(id) = read::<[u8; 32]>(adapter, &key(ADDED_EPOCH, &epoch.to_be_bytes()))? else {
            return Ok(None);
        };
        let row = required(self.added(adapter, id)?)?;
        if row.signed.manifest.epoch != epoch {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn added_member(
        &self,
        adapter: &impl Facts,
        member: &Member,
    ) -> Result<Option<Added>, EnrollmentError> {
        let Some(id) = read::<[u8; 32]>(adapter, &key(ADDED_MEMBER, &member_key(member)))? else {
            return Ok(None);
        };
        let row = required(self.added(adapter, id)?)?;
        if &row.member != member {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn terminal(
        &self,
        adapter: &impl Facts,
        id: [u8; 32],
    ) -> Result<Option<Terminal>, EnrollmentError> {
        let row: Option<Terminal> =
            read_row(adapter, TERMINAL, TERMINAL_ORDER, &id, self.terminal_count)?;
        if row
            .as_ref()
            .map(Terminal::id)
            .transpose()?
            .is_some_and(|value| value != id)
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(row)
    }
    pub(in crate::enrollment) fn terminal_subject(
        &self,
        adapter: &impl Facts,
        kind: &TerminalKind,
    ) -> Result<Option<Terminal>, EnrollmentError> {
        let Some(id) = read::<[u8; 32]>(adapter, &key(TERMINAL_SUBJECT, &subject(kind)))? else {
            return Ok(None);
        };
        let row = required(self.terminal(adapter, id)?)?;
        if &row.statement.kind != kind {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn acknowledged(
        &self,
        adapter: &impl Facts,
        id: [u8; 32],
    ) -> Result<bool, EnrollmentError> {
        match read_row::<bool>(adapter, ACK, ACK_ORDER, &id, self.ack_count)? {
            None => Ok(false),
            Some(true) => Ok(true),
            Some(false) => Err(EnrollmentError::Corrupt),
        }
    }
    pub(in crate::enrollment) fn management(
        &self,
        adapter: &impl Facts,
        id: [u8; 32],
    ) -> Result<Option<Request>, EnrollmentError> {
        let row: Option<Request> =
            read_row(adapter, REQUEST, REQUEST_ORDER, &id, self.management_count)?;
        if row.as_ref().is_some_and(|r| r.id != id) {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(row)
    }
    pub(in crate::enrollment) fn added_at(
        &self,
        adapter: &impl Facts,
        ordinal: u64,
    ) -> Result<Added, EnrollmentError> {
        if ordinal >= self.added_count {
            return Err(EnrollmentError::Invalid);
        }
        let id = required(read::<Vec<u8>>(
            adapter,
            &key(ADDED_ORDER, &ordinal.to_be_bytes()),
        )?)?;
        required(self.added(
            adapter,
            id.try_into().map_err(|_| EnrollmentError::Corrupt)?,
        )?)
    }
    pub(in crate::enrollment) fn terminal_at(
        &self,
        adapter: &impl Facts,
        ordinal: u64,
    ) -> Result<Terminal, EnrollmentError> {
        if ordinal >= self.terminal_count {
            return Err(EnrollmentError::Invalid);
        }
        let id = required(read::<Vec<u8>>(
            adapter,
            &key(TERMINAL_ORDER, &ordinal.to_be_bytes()),
        )?)?;
        required(self.terminal(
            adapter,
            id.try_into().map_err(|_| EnrollmentError::Corrupt)?,
        )?)
    }
    pub(in crate::enrollment) fn past_member(
        &self,
        adapter: &impl Facts,
        endpoint: Endpoint,
    ) -> Result<Option<Member>, EnrollmentError> {
        let id = member_key_parts(endpoint.id.0, endpoint.incarnation.get());
        let row: Option<PastMember> = read_row(
            adapter,
            PAST_MEMBER,
            PAST_MEMBER_ORDER,
            &id,
            self.past_member_count,
        )?;
        if row
            .as_ref()
            .is_some_and(|row| member_key(&row.member) != id)
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(row.map(|row| row.member))
    }

    pub(super) fn past_record_at(
        &self,
        adapter: &impl Facts,
        ordinal: u64,
    ) -> Result<PastMember, EnrollmentError> {
        if ordinal >= self.past_member_count {
            return Err(EnrollmentError::Invalid);
        }
        let id = required(read::<Vec<u8>>(
            adapter,
            &key(PAST_MEMBER_ORDER, &ordinal.to_be_bytes()),
        )?)?;
        if id.len() != 40 {
            return Err(EnrollmentError::Corrupt);
        }
        let past: PastMember = required(read_row(
            adapter,
            PAST_MEMBER,
            PAST_MEMBER_ORDER,
            &id,
            self.past_member_count,
        )?)?;
        if member_key(&past.member) != id.as_slice() {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(past)
    }
}

pub(super) fn read_row<T: serde::de::DeserializeOwned>(
    adapter: &impl Facts,
    kind: u8,
    order: u8,
    id: &[u8],
    count: u64,
) -> Result<Option<T>, EnrollmentError> {
    let Some(row) = read::<Stored<T>>(adapter, &key(kind, id))? else {
        return Ok(None);
    };
    if row.ordinal >= count {
        return Err(EnrollmentError::Corrupt);
    }
    let actual = required(read::<Vec<u8>>(
        adapter,
        &key(order, &row.ordinal.to_be_bytes()),
    )?)?;
    if actual != id {
        return Err(EnrollmentError::Corrupt);
    }
    Ok(Some(row.value))
}
