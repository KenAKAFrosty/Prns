use super::*;
use prns_groups_mls::ApplicationTransaction;
type Tx<'a, 'db> = ApplicationTransaction<'a, RedbBackend<'db>>;

fn put<T: Serialize>(
    tx: &Tx<'_, '_>,
    key: &[u8],
    value: &T,
    limit: usize,
) -> Result<(), EnrollmentError> {
    let bytes = encode_with_limit(value, limit)?;
    tx.put(key, &bytes).map_err(|_| EnrollmentError::Corrupt)
}
fn read_tx<T: serde::de::DeserializeOwned>(
    tx: &Tx<'_, '_>,
    key: &[u8],
) -> Result<Option<T>, EnrollmentError> {
    tx.read(key)
        .map_err(|_| EnrollmentError::Corrupt)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| EnrollmentError::Corrupt))
        .transpose()
}
/// Existing links are checked, never silently repaired on a replay.
fn link<T: Serialize>(
    tx: &Tx<'_, '_>,
    key: &[u8],
    value: &T,
    insert: bool,
    limit: usize,
) -> Result<(), EnrollmentError> {
    let bytes = encode_with_limit(value, limit)?;
    match tx.read(key).map_err(|_| EnrollmentError::Corrupt)? {
        Some(existing) if existing.as_slice() == bytes => Ok(()),
        Some(_) => Err(EnrollmentError::Conflict),
        None if insert => tx.put(key, &bytes).map_err(|_| EnrollmentError::Corrupt),
        None => Err(EnrollmentError::Corrupt),
    }
}
/// Immutable payload and reciprocal ordinal, committed with the header count.
fn row<T: Serialize + serde::de::DeserializeOwned>(
    tx: &Tx<'_, '_>,
    kind: u8,
    order: u8,
    id: &[u8],
    value: &T,
    count: &mut u64,
    limit: usize,
) -> Result<bool, EnrollmentError> {
    let row_key = key(kind, id);
    if let Some(old) = read_tx::<Stored<T>>(tx, &row_key)? {
        if old.ordinal >= *count {
            return Err(EnrollmentError::Corrupt);
        }
        if encode_with_limit(&old.value, limit)? != encode_with_limit(value, limit)? {
            return Err(EnrollmentError::Conflict);
        }
        link(
            tx,
            &key(order, &old.ordinal.to_be_bytes()),
            &id,
            false,
            limit,
        )?;
        return Ok(false);
    }
    let ordinal = *count;
    *count = count.checked_add(1).ok_or(EnrollmentError::Capacity)?;
    link(tx, &key(order, &ordinal.to_be_bytes()), &id, true, limit)?;
    put(tx, &row_key, &Stored { ordinal, value }, limit)?;
    Ok(true)
}
impl Snapshot {
    pub(in crate::enrollment) fn stage(&self, tx: &Tx<'_, '_>) -> Result<(), EnrollmentError> {
        self.validate_counts()?;
        put(tx, STATE_KEY, &self.header, self.value_limit()?)
    }
    pub(in crate::enrollment) fn stage_control(
        &mut self,
        tx: &Tx<'_, '_>,
        control: &Control,
    ) -> Result<(), EnrollmentError> {
        let epoch = control.signed.manifest.epoch;
        let limit = self.value_limit()?;
        let inserted = row(
            tx,
            CONTROL,
            CONTROL_ORDER,
            &epoch.to_be_bytes(),
            control,
            &mut self.control_count,
            limit,
        )?;
        link(
            tx,
            &key(CONTROL_HEAD, &control.signed.head()?),
            &epoch,
            inserted,
            limit,
        )?;
        link(
            tx,
            &key(CONTROL_PARENT, &control.signed.manifest.parent),
            &epoch,
            inserted,
            limit,
        )?;
        for member in &control.signed.manifest.members {
            if let Some(past) =
                read_tx::<Stored<PastMember>>(tx, &key(PAST_MEMBER, &member_key(member)))?
            {
                if past.value.member != *member {
                    return Err(EnrollmentError::Conflict);
                }
            }
        }
        if let ControlKind::Remove(member) = &control.signed.manifest.kind {
            let id = member_key(member);
            let past = match read_tx::<Stored<PastMember>>(tx, &key(PAST_MEMBER, &id))? {
                Some(old) if &old.value.member == member => old.value,
                Some(_) => return Err(EnrollmentError::Conflict),
                None => PastMember {
                    member: member.clone(),
                    removed_epoch: epoch,
                },
            };
            row(
                tx,
                PAST_MEMBER,
                PAST_MEMBER_ORDER,
                &id,
                &past,
                &mut self.past_member_count,
                limit,
            )?;
        }
        Ok(())
    }
    pub(in crate::enrollment) fn stage_added(
        &mut self,
        tx: &Tx<'_, '_>,
        added: &Added,
    ) -> Result<(), EnrollmentError> {
        let limit = self.value_limit()?;
        let inserted = row(
            tx,
            ADDED,
            ADDED_ORDER,
            &added.invitation,
            added,
            &mut self.added_count,
            limit,
        )?;
        link(
            tx,
            &key(ADDED_EPOCH, &added.signed.manifest.epoch.to_be_bytes()),
            &added.invitation,
            inserted,
            limit,
        )?;
        let member = key(ADDED_MEMBER, &member_key(&added.member));
        if let Some(previous) = read_tx::<[u8; 32]>(tx, &member)? {
            let old: Stored<Added> = required(read_tx(tx, &key(ADDED, &previous))?)?;
            if old.value.member != added.member {
                return Err(EnrollmentError::Conflict);
            }
        } else {
            if !inserted {
                return Err(EnrollmentError::Corrupt);
            }
            self.added_member_count = self
                .added_member_count
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        if inserted {
            put(tx, &member, &added.invitation, limit)?;
        }
        Ok(())
    }
    pub(in crate::enrollment) fn stage_terminal(
        &mut self,
        tx: &Tx<'_, '_>,
        proof: &Terminal,
    ) -> Result<(), EnrollmentError> {
        let id = proof.id()?;
        let limit = self.value_limit()?;
        let inserted = row(
            tx,
            TERMINAL,
            TERMINAL_ORDER,
            &id,
            proof,
            &mut self.terminal_count,
            limit,
        )?;
        let subject_key = key(TERMINAL_SUBJECT, &subject(&proof.statement.kind));
        if let Some(previous) = read_tx::<[u8; 32]>(tx, &subject_key)? {
            let old: Stored<Terminal> = required(read_tx(tx, &key(TERMINAL, &previous))?)?;
            if old.value.statement.kind != proof.statement.kind {
                return Err(EnrollmentError::Corrupt);
            }
            if inserted
                && (old.value.statement.revision >= proof.statement.revision
                    || matches!(proof.statement.kind, TerminalKind::Closed))
            {
                return Err(EnrollmentError::Conflict);
            }
        } else {
            if !inserted {
                return Err(EnrollmentError::Corrupt);
            }
            self.terminal_subject_count = self
                .terminal_subject_count
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        if inserted {
            put(tx, &subject_key, &id, limit)?;
        }
        Ok(())
    }
    pub(in crate::enrollment) fn stage_request(
        &mut self,
        tx: &Tx<'_, '_>,
        request: &Request,
    ) -> Result<(), EnrollmentError> {
        let limit = self.value_limit()?;
        row(
            tx,
            REQUEST,
            REQUEST_ORDER,
            &request.id,
            request,
            &mut self.management_count,
            limit,
        )?;
        Ok(())
    }
}
