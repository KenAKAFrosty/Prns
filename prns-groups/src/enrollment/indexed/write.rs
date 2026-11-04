//! A bounded catalog projection, enlisted in the existing physical commit.
//! No payloads from unaffected rows are retained. The header revision fences
//! concurrent changes; per-row hashes additionally bind exactly what is edited.
use super::*;
use sha2::Digest;
mod maintenance;
mod projection;
mod purge;
mod terminal_obligations;
mod work;

struct RowChange {
    table: TableDefinition<'static, &'static [u8], &'static [u8]>,
    key: Vec<u8>,
    before: Option<[u8; 32]>,
    after: Option<Zeroizing<Vec<u8>>>,
}

pub(in crate::enrollment) const fn row_slot_bytes() -> usize {
    std::mem::size_of::<RowChange>()
}

/// Local mutation work limits, separate from the durable catalog byte quota.
/// These are supplied from the selected catalog policy by the owning operation.
pub(in crate::enrollment) struct Budget {
    pub rows: usize,
    pub bytes: usize,
    pub row_bytes: usize,
}

pub(in crate::enrollment) struct Edit {
    before: [u8; 32],
    before_revision: u64,
    header: Header,
    rows: Vec<RowChange>,
    budget: Budget,
    allocated: usize,
}

/// The final witness and its rows are retained by the existing participant only
/// until the one catalog/namespace transaction settles. Dropping the projection
/// zeroizes encoded private membership material.
pub(in crate::enrollment) struct Write {
    pub before: [u8; 32],
    pub after: [u8; 32],
    before_revision: u64,
    header: Header,
    rows: Vec<RowChange>,
}

impl Edit {
    /// Resolve this edit's already-projected value before consulting its
    /// original snapshot. Several facts may affect one peer/group counter.
    fn projected<T: serde::de::DeserializeOwned>(
        &self,
        table: TableDefinition<'static, &'static [u8], &'static [u8]>,
        key: &[u8],
        original: &ReadOnlyTable<&'static [u8], &'static [u8]>,
    ) -> Result<Option<T>, EnrollmentError> {
        if let Some(row) = self
            .rows
            .iter()
            .find(|row| row.table.name() == table.name() && row.key == key)
        {
            return row
                .after
                .as_ref()
                .map(|value| rows::decode_row(value, self.budget.row_bytes))
                .transpose();
        }
        original
            .get(key)
            .map_err(db_error)?
            .as_ref()
            .map(|value| rows::decode_row(value.value(), self.budget.row_bytes))
            .transpose()
    }

    pub(in crate::enrollment) fn new(read: &Read) -> Result<Self, EnrollmentError> {
        let budget = read.header.limits.transaction()?;
        if budget.rows == 0
            || budget.bytes == 0
            || budget.row_bytes == 0
            || budget.bytes > isize::MAX as usize
            || budget.row_bytes > budget.bytes
        {
            return Err(EnrollmentError::Invalid);
        }
        Ok(Self {
            before: read.header.fingerprint()?,
            before_revision: read.header.revision,
            header: read.header.clone(),
            rows: Vec::new(),
            budget,
            allocated: 0,
        })
    }

    fn encode<T: Serialize>(&self, value: &T) -> Result<Zeroizing<Vec<u8>>, EnrollmentError> {
        // One candidate row is separate bounded encoding scratch. replace()
        // charges its actual capacity, refunding the old projected value first;
        // coalescing a full-budget counter must not require a second full lane.
        Ok(Zeroizing::new(encode_with_limit(
            value,
            self.budget.row_bytes,
        )?))
    }

    fn replace(
        &mut self,
        table: TableDefinition<'static, &'static [u8], &'static [u8]>,
        key: &[u8],
        before: Option<&[u8]>,
        after: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<(), EnrollmentError> {
        if key.is_empty() {
            return Err(EnrollmentError::Corrupt);
        }
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| row.table.name() == table.name() && row.key == key)
        {
            let row = self.rows.get(index).ok_or(EnrollmentError::Corrupt)?;
            if row.before != before.map(hash) {
                return Err(EnrollmentError::Conflict);
            }
            if row.after.as_ref().map(|v| v.as_slice()) == after.as_ref().map(|v| v.as_slice()) {
                return Ok(());
            }
            let old_bytes = row
                .after
                .as_ref()
                .map_or(Ok(0), |value| logical_bytes(key, value))?;
            let new_bytes = after
                .as_ref()
                .map_or(Ok(0), |value| logical_bytes(key, value))?;
            let allocated = self
                .allocated
                .checked_sub(row.after.as_ref().map_or(0, |v| v.capacity()))
                .and_then(|n| n.checked_add(after.as_ref().map_or(0, |v| v.capacity())))
                .ok_or(EnrollmentError::Capacity)?;
            let slots = self
                .rows
                .capacity()
                .checked_mul(row_slot_bytes())
                .ok_or(EnrollmentError::Capacity)?;
            if allocated
                .checked_add(slots)
                .is_none_or(|n| n > self.budget.bytes)
            {
                return Err(EnrollmentError::Capacity);
            }
            self.header.bytes = self
                .header
                .bytes
                .checked_sub(old_bytes)
                .and_then(|n| n.checked_add(new_bytes))
                .ok_or(EnrollmentError::Corrupt)?;
            self.rows
                .get_mut(index)
                .ok_or(EnrollmentError::Corrupt)?
                .after = after;
            self.allocated = allocated;
            return Ok(());
        }
        if before == after.as_ref().map(|value| value.as_slice()) {
            return Ok(());
        }
        if self.rows.len() >= self.budget.rows {
            return Err(EnrollmentError::Capacity);
        }
        let old_bytes = before.map_or(Ok(0), |value| logical_bytes(key, value))?;
        let new_bytes = after
            .as_ref()
            .map_or(Ok(0), |value| logical_bytes(key, value))?;
        let live = self
            .header
            .bytes
            .checked_sub(old_bytes)
            .and_then(|n| n.checked_add(new_bytes))
            .ok_or(EnrollmentError::Corrupt)?;

        let mut owned_key = Vec::new();
        owned_key
            .try_reserve_exact(key.len())
            .map_err(|_| EnrollmentError::Capacity)?;
        owned_key.extend_from_slice(key);
        let owned = owned_key
            .capacity()
            .checked_add(after.as_ref().map_or(0, |bytes| bytes.capacity()))
            .ok_or(EnrollmentError::Capacity)?;
        let allocated = self
            .allocated
            .checked_add(owned)
            .ok_or(EnrollmentError::Capacity)?;
        let next_rows = self
            .rows
            .len()
            .checked_add(1)
            .ok_or(EnrollmentError::Capacity)?;
        let minimum_slots = next_rows
            .checked_mul(std::mem::size_of::<RowChange>())
            .ok_or(EnrollmentError::Capacity)?;
        if allocated
            .checked_add(minimum_slots)
            .is_none_or(|n| n > self.budget.bytes)
        {
            return Err(EnrollmentError::Capacity);
        }
        if next_rows > self.rows.capacity() {
            self.rows
                .try_reserve_exact(1)
                .map_err(|_| EnrollmentError::Capacity)?;
        }
        let actual_slots = self
            .rows
            .capacity()
            .checked_mul(std::mem::size_of::<RowChange>())
            .ok_or(EnrollmentError::Capacity)?;
        if allocated
            .checked_add(actual_slots)
            .is_none_or(|n| n > self.budget.bytes)
        {
            return Err(EnrollmentError::Capacity);
        }
        self.rows.push(RowChange {
            table,
            key: owned_key,
            before: before.map(hash),
            after,
        });
        self.allocated = allocated;
        self.header.bytes = live;
        Ok(())
    }

    pub(in crate::enrollment) fn put_current(
        mut self,
        read: &Read,
        group: [u8; 32],
        current: &rows::Current,
    ) -> Result<Self, EnrollmentError> {
        let prior = read.current.get(group.as_slice()).map_err(db_error)?;
        let old: Option<rows::Current> = self.projected(CURRENT, &group, &read.current)?;
        let existed = old.is_some();
        if current.invitations_deleted_through > self.header.revision
            || old.as_ref().is_some_and(|old| {
                current.generation < old.generation
                    || current.invitations_deleted_through < old.invitations_deleted_through
                    || current.purge_through < old.purge_through
            })
        {
            return Err(EnrollmentError::Conflict);
        }
        let old_reserve = old
            .as_ref()
            .map(|row| obligations::current_reserve(row, self.budget.row_bytes))
            .transpose()?
            .unwrap_or(0);
        let reserve = obligations::current_reserve(current, self.budget.row_bytes)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(old_reserve)
            .and_then(|n| n.checked_add(reserve))
            .ok_or(EnrollmentError::Corrupt)?;
        let old_sync = old
            .as_ref()
            .and_then(|row| super::work::sync_id(self.header.local, row));
        let next_sync = super::work::sync_id(self.header.local, current);
        if old_sync != next_sync {
            if let Some(id) = old_sync {
                self = self.remove_work(read, id, super::work::WorkSource::Sync(group))?;
            }
            if let Some(id) = next_sync {
                self = self.index_work(read, id, super::work::WorkSource::Sync(group))?;
            }
        }
        let value = self.encode(current)?;
        self.replace(
            CURRENT,
            &group,
            prior.as_ref().map(|row| row.value()),
            Some(value),
        )?;
        if !existed {
            self.header.counts.groups = self
                .header
                .counts
                .groups
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    /// Added/accepted membership owns this exact invitation association even
    /// after its presentation row is dismissed or its namespace is purged.
    pub(in crate::enrollment) fn bind_invitation(
        mut self,
        read: &Read,
        id: [u8; 32],
        binding: &rows::InvitationBinding,
    ) -> Result<Self, EnrollmentError> {
        let prior = read
            .invitation_bindings
            .get(id.as_slice())
            .map_err(db_error)?;
        let old: Option<rows::InvitationBinding> =
            self.projected(INVITATION_BINDINGS, &id, &read.invitation_bindings)?;
        if let Some(old) = &old {
            if old != binding {
                return Err(EnrollmentError::Conflict);
            }
        }
        if old.is_none() {
            self = self.consume_outgoing_reservation(read, id)?;
            self.fund_remove(read, id, binding)?;
        }
        let value = self.encode(binding)?;
        self.replace(
            INVITATION_BINDINGS,
            &id,
            prior.as_ref().map(|r| r.value()),
            Some(value),
        )?;
        if old.is_none() {
            self.header.counts.invitation_bindings = self
                .header
                .counts
                .invitation_bindings
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn admit_audience(
        self,
        read: &Read,
        group: [u8; 32],
        recipient: [u8; 16],
    ) -> Result<Self, EnrollmentError> {
        let mut audience = self
            .projected::<rows::Audience>(
                AUDIENCE,
                &keys::audience(group, recipient),
                &read.audience,
            )?
            .unwrap_or(rows::Audience {
                admitted: false,
                offers: 0,
                close_notice_funded: true,
            });
        audience.admitted = true;
        self.put_audience(read, group, recipient, Some(audience))
    }

    /// Called exactly when an outgoing offer enters/leaves the retained offer
    /// table, not on each delivery retry or mutable presentation-state change.
    pub(in crate::enrollment) fn offer_audience(
        self,
        read: &Read,
        group: [u8; 32],
        recipient: [u8; 16],
        added: bool,
    ) -> Result<Self, EnrollmentError> {
        let mut audience = self
            .projected::<rows::Audience>(
                AUDIENCE,
                &keys::audience(group, recipient),
                &read.audience,
            )?
            .unwrap_or(rows::Audience {
                admitted: false,
                offers: 0,
                close_notice_funded: true,
            });
        audience.offers = if added {
            audience
                .offers
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?
        } else {
            audience
                .offers
                .checked_sub(1)
                .ok_or(EnrollmentError::Corrupt)?
        };
        let keep = audience.admitted || audience.offers != 0;
        self.put_audience(read, group, recipient, keep.then_some(audience))
    }

    fn put_audience(
        mut self,
        read: &Read,
        group: [u8; 32],
        recipient: [u8; 16],
        next: Option<rows::Audience>,
    ) -> Result<Self, EnrollmentError> {
        if recipient == self.header.local {
            return Err(EnrollmentError::Invalid);
        }
        let key = keys::audience(group, recipient);
        let prior = read.audience.get(key.as_slice()).map_err(db_error)?;
        let old: Option<rows::Audience> = self.projected(AUDIENCE, &key, &read.audience)?;
        let before_reserve = obligations::audience_reserve(old.as_ref(), self.budget.row_bytes)?;
        let after_reserve = obligations::audience_reserve(next.as_ref(), self.budget.row_bytes)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(before_reserve)
            .and_then(|n| n.checked_add(after_reserve))
            .ok_or(EnrollmentError::Corrupt)?;
        let value = next.as_ref().map(|row| self.encode(row)).transpose()?;
        self.replace(AUDIENCE, &key, prior.as_ref().map(|r| r.value()), value)?;
        match (old.is_some(), next.is_some()) {
            (false, true) => {
                self.header.counts.audience = self
                    .header
                    .counts
                    .audience
                    .checked_add(1)
                    .ok_or(EnrollmentError::Capacity)?
            }
            (true, false) => {
                self.header.counts.audience = self
                    .header
                    .counts
                    .audience
                    .checked_sub(1)
                    .ok_or(EnrollmentError::Corrupt)?
            }
            _ => {}
        }
        Ok(self)
    }

    /// A bounded fan-out step consumes the previously funded Close obligation
    /// into one exact outbox row. It does not need to rewrite the full audience.
    pub(in crate::enrollment) fn materialize_close_notice(
        mut self,
        read: &Read,
        proof: &Terminal,
        recipient: [u8; 16],
    ) -> Result<Self, EnrollmentError> {
        if !matches!(proof.statement.kind, TerminalKind::Closed)
            || proof.statement.owner.rns != self.header.local
        {
            return Err(EnrollmentError::NotOwner);
        }
        let group = proof.statement.group;
        let stored = read
            .terminal(proof.id()?)?
            .ok_or(EnrollmentError::Missing)?;
        if stored.proof != *proof {
            return Err(EnrollmentError::Corrupt);
        }
        let mut audience = read
            .audience(group, recipient)?
            .ok_or(EnrollmentError::Missing)?;
        let notification = Notification {
            proof: proof.clone(),
            recipient,
            acknowledged: false,
        };
        let id = Enrollment::notice_id(&notification)?;
        let prior = read.notices.get(id.as_slice()).map_err(db_error)?;
        if let Some(prior) = &prior {
            let notice: rows::Notice = rows::decode_row(prior.value(), self.budget.row_bytes)?;
            if notice.terminal != proof.id()?
                || notice.recipient != recipient
                || audience.close_notice_funded
            {
                return Err(EnrollmentError::Corrupt);
            }
            return Ok(self);
        }
        if !audience.close_notice_funded {
            return Err(EnrollmentError::Corrupt);
        }
        let row = rows::Notice {
            terminal: proof.id()?,
            recipient,
            acknowledged: false,
        };
        let value = self.encode(&row)?;
        self.replace(NOTICES, &id, None, Some(value))?;
        self.header.counts.notices = self
            .header
            .counts
            .notices
            .checked_add(1)
            .ok_or(EnrollmentError::Capacity)?;
        audience.close_notice_funded = false;
        self.put_audience(read, group, recipient, Some(audience))?
            .index_work(read, id, super::work::WorkSource::Notice(id))
    }

    pub(in crate::enrollment) fn put_terminal(
        mut self,
        read: &Read,
        row: &rows::TerminalRow,
    ) -> Result<Self, EnrollmentError> {
        let id = row.proof.id()?;
        let group = row.proof.statement.group;
        let prior = read.terminals.get(id.as_slice()).map_err(db_error)?;
        let old: Option<rows::TerminalRow> = self.projected(TERMINALS, &id, &read.terminals)?;
        if old.is_none() {
            self.consume_terminal_obligation(read, &row.proof)?;
        }
        if let Some(old) = &old {
            if old.proof != row.proof || (old.complete && !row.complete) || row.after < old.after {
                return Err(EnrollmentError::Conflict);
            }
        }
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(obligations::terminal_reserve(
                old.as_ref(),
                self.budget.row_bytes,
            )?)
            .and_then(|n| {
                n.checked_add(obligations::terminal_reserve(Some(row), self.budget.row_bytes).ok()?)
            })
            .ok_or(EnrollmentError::Corrupt)?;
        let value = self.encode(row)?;
        self.replace(
            TERMINALS,
            &id,
            prior.as_ref().map(|r| r.value()),
            Some(value),
        )?;
        if prior.is_none() {
            let key = keys::group_item(group, id);
            let subject = keys::terminal_subject(group, &row.proof.statement.kind);
            if read
                .terminal_groups
                .get(key.as_slice())
                .map_err(db_error)?
                .is_some()
            {
                return Err(EnrollmentError::Corrupt);
            }
            if read
                .terminal_subjects
                .get(subject.as_slice())
                .map_err(db_error)?
                .is_some()
            {
                return Err(EnrollmentError::Conflict);
            }
            self.replace(
                TERMINAL_GROUPS,
                &key,
                None,
                Some(Zeroizing::new(id.to_vec())),
            )?;
            self.replace(
                TERMINAL_SUBJECTS,
                &subject,
                None,
                Some(Zeroizing::new(id.to_vec())),
            )?;
            self.header.counts.terminals = self
                .header
                .counts
                .terminals
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn acknowledge_notice(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let Some(mut notice) = read.notice(id)? else {
            return Ok(self);
        };
        notice.acknowledged = true;
        let prior = read
            .notices
            .get(id.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        let value = self.encode(&notice)?;
        self.replace(NOTICES, &id, Some(prior.value()), Some(value))?;
        Ok(self)
    }

    pub(in crate::enrollment) fn fanout_close(
        mut self,
        read: &Read,
        terminal: [u8; 32],
        limits: crate::PageLimits,
    ) -> Result<Self, EnrollmentError> {
        let mut row = read.terminal(terminal)?.ok_or(EnrollmentError::Missing)?;
        if row.complete {
            return Ok(self);
        }
        if !matches!(row.proof.statement.kind, TerminalKind::Closed) {
            return Err(EnrollmentError::Invalid);
        }
        let available = self
            .budget
            .rows
            .saturating_sub(self.rows.len())
            .saturating_sub(1)
            / 3;
        let max_rows = usize::try_from(self.header.limits.max_maintenance_rows)
            .map_err(|_| EnrollmentError::Invalid)?
            .min(available.max(1))
            .min(usize::try_from(limits.max_rows).map_err(|_| EnrollmentError::Invalid)?);
        if max_rows == 0 {
            return Err(EnrollmentError::Capacity);
        }
        let page = read.audience_page(
            row.proof.statement.group,
            row.after,
            crate::PageLimits {
                max_rows: u32::try_from(max_rows).map_err(|_| EnrollmentError::Invalid)?,
                max_bytes: limits.max_bytes,
            },
        )?;
        let mut processed = 0usize;
        for (recipient, audience) in &page.rows {
            // Keep room for the final continuation before adding a recipient.
            // The same serializer and actual Vec capacities used by replace()
            // determine the byte fit; no caller retry with a magic smaller page.
            if !self.close_step_fits(&row, *recipient, audience)? {
                if processed == 0 {
                    return Err(EnrollmentError::Capacity);
                }
                break;
            }
            self = self.materialize_close_notice(read, &row.proof, *recipient)?;
            row.after = Some(*recipient);
            processed += 1;
        }
        row.complete = page.next.is_none() && processed == page.rows.len();
        self.put_terminal(read, &row)
    }

    fn close_step_fits(
        &self,
        terminal: &rows::TerminalRow,
        recipient: [u8; 16],
        audience: &rows::Audience,
    ) -> Result<bool, EnrollmentError> {
        let maximum_cursor = rows::TerminalRow {
            proof: terminal.proof.clone(),
            after: Some([u8::MAX; 16]),
            complete: false,
        };
        let cursor = match self.encode(&maximum_cursor) {
            Ok(value) => value,
            Err(EnrollmentError::Capacity) => return Ok(false),
            Err(error) => return Err(error),
        };
        let mut added = 32usize
            .checked_add(cursor.capacity())
            .ok_or(EnrollmentError::Capacity)?;
        let mut rows = 1usize;
        if audience.close_notice_funded {
            let notice = rows::Notice {
                terminal: terminal.proof.id()?,
                recipient,
                acknowledged: false,
            };
            let mut next_audience = audience.clone();
            next_audience.close_notice_funded = false;
            let notification = Notification {
                proof: terminal.proof.clone(),
                recipient,
                acknowledged: false,
            };
            let id = Enrollment::notice_id(&notification)?;
            for (key_bytes, encoded) in [
                (32usize, self.encode(&notice)),
                (48usize, self.encode(&next_audience)),
                (32usize, self.encode(&super::work::WorkSource::Notice(id))),
            ] {
                let encoded = match encoded {
                    Ok(value) => value,
                    Err(EnrollmentError::Capacity) => return Ok(false),
                    Err(error) => return Err(error),
                };
                added = added
                    .checked_add(key_bytes)
                    .and_then(|n| n.checked_add(encoded.capacity()))
                    .ok_or(EnrollmentError::Capacity)?;
            }
            rows += 3;
        }
        let count = self
            .rows
            .len()
            .checked_add(rows)
            .ok_or(EnrollmentError::Capacity)?;
        let slots = count
            .max(self.rows.capacity())
            .checked_mul(row_slot_bytes())
            .ok_or(EnrollmentError::Capacity)?;
        Ok(count <= self.budget.rows
            && self
                .allocated
                .checked_add(added)
                .and_then(|n| n.checked_add(slots))
                .is_some_and(|n| n <= self.budget.bytes))
    }

    pub(in crate::enrollment) fn put_incarnation(
        mut self,
        read: &Read,
        key: keys::IncarnationKey,
        incarnation: &rows::Incarnation,
    ) -> Result<Self, EnrollmentError> {
        incarnation.validate_key(key, IdentityHash::new(self.header.local))?;
        let prior = read.incarnations.get(key.as_slice()).map_err(db_error)?;
        let old: Option<rows::Incarnation> =
            self.projected(INCARNATIONS, &key, &read.incarnations)?;
        let old_reserve = old
            .as_ref()
            .map(|row| obligations::incarnation_reserve(row, self.budget.row_bytes))
            .transpose()?
            .unwrap_or(0);
        let reserve = obligations::incarnation_reserve(incarnation, self.budget.row_bytes)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(old_reserve)
            .and_then(|n| n.checked_add(reserve))
            .ok_or(EnrollmentError::Corrupt)?;
        let namespace = incarnation.namespace();
        let id = incarnation.store_incarnation();
        if old
            .as_ref()
            .is_some_and(|old| old.namespace() != namespace || old.store_incarnation() != id)
        {
            return Err(EnrollmentError::Conflict);
        }
        let value = self.encode(incarnation)?;
        self.replace(
            INCARNATIONS,
            &key,
            prior.as_ref().map(|row| row.value()),
            Some(value),
        )?;
        if old.is_none() {
            self.fund_close(read, key, incarnation)?;
            for (table, existing, index) in [
                (NAMESPACES, &read.namespaces, namespace.as_slice()),
                (INCARNATION_IDS, &read.incarnation_ids, id.as_slice()),
            ] {
                if existing.get(index).map_err(db_error)?.is_some() {
                    return Err(EnrollmentError::Conflict);
                }
                self.replace(table, index, None, Some(Zeroizing::new(key.to_vec())))?;
            }
        }
        if let Some(old) = &old {
            let count = membership_count(&mut self.header.counts, old);
            *count = count.checked_sub(1).ok_or(EnrollmentError::Corrupt)?;
        }
        let count = membership_count(&mut self.header.counts, incarnation);
        *count = count.checked_add(1).ok_or(EnrollmentError::Capacity)?;
        Ok(self)
    }

    pub(in crate::enrollment) fn put_receipt(
        mut self,
        read: &Read,
        receipt: &rows::Receipt,
    ) -> Result<Self, EnrollmentError> {
        let id = receipt.request.id;
        if id == [0; 32] {
            return Err(EnrollmentError::Invalid);
        }
        let prior = read.requests.get(id.as_slice()).map_err(db_error)?;
        let prior_receipt = prior
            .as_ref()
            .map(|bytes| rows::decode_row::<rows::Receipt>(bytes.value(), self.budget.row_bytes))
            .transpose()?;
        if let Some(old) = &prior_receipt {
            if old.request.id != id {
                return Err(EnrollmentError::Corrupt);
            }
            if old.request.fingerprint != receipt.request.fingerprint
                || old.request.result != receipt.request.result
                || (old.namespace.is_some() && old.namespace != receipt.namespace)
            {
                return Err(EnrollmentError::RequestConflict);
            }
        }
        let value = self.encode(receipt)?;
        self.replace(
            REQUESTS,
            &id,
            prior.as_ref().map(|row| row.value()),
            Some(value),
        )?;
        let key = receipt.retirement_key()?;
        let old_key = prior_receipt
            .as_ref()
            .map(rows::Receipt::retirement_key)
            .transpose()?;
        if old_key.as_ref() != Some(&key) {
            if let Some(old_key) = old_key {
                let prior_index = read
                    .request_expiry
                    .get(old_key.as_slice())
                    .map_err(db_error)?
                    .ok_or(EnrollmentError::Corrupt)?;
                if prior_index.value() != id {
                    return Err(EnrollmentError::Corrupt);
                }
                self.replace(REQUEST_EXPIRY, &old_key, Some(prior_index.value()), None)?;
            }
            if read
                .request_expiry
                .get(key.as_slice())
                .map_err(db_error)?
                .is_some()
            {
                return Err(EnrollmentError::Corrupt);
            }
            self.replace(
                REQUEST_EXPIRY,
                &key,
                None,
                Some(Zeroizing::new(id.to_vec())),
            )?;
        }
        if prior.is_none() {
            self.header.counts.requests = self
                .header
                .counts
                .requests
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn finish(mut self) -> Result<Write, EnrollmentError> {
        if self.header.counts.live > self.header.counts.groups {
            return Err(EnrollmentError::Corrupt);
        }
        if self
            .header
            .limits
            .max_groups
            .is_some_and(|limit| self.header.counts.live > limit)
        {
            return Err(EnrollmentError::Capacity);
        }
        if self
            .header
            .bytes
            .checked_add(self.header.reserved)
            .is_none_or(|n| n > self.header.limits.max_catalog_bytes)
        {
            return Err(EnrollmentError::Capacity);
        }
        self.rows.sort_by(|left, right| {
            left.table
                .name()
                .cmp(right.table.name())
                .then_with(|| left.key.cmp(&right.key))
        });
        if !self.rows.is_empty() || self.header.fingerprint()? != self.before {
            self.header.revision = self
                .header
                .revision
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
            let mut digest = Sha256::new();
            digest.update(b"prns.groups.catalog.commit.v5");
            digest.update(self.before);
            // Bind changed floors, counts, liabilities and policy as well as
            // payloads. The header still contains its preceding row witness at
            // this point; replacing it below does not create a circular hash.
            digest.update(encode_with_limit(&self.header, Header::MAX_ENCODED_BYTES)?);
            for row in &self.rows {
                let name = row.table.name().as_bytes();
                digest.update((name.len() as u64).to_be_bytes());
                digest.update(name);
                digest.update((row.key.len() as u64).to_be_bytes());
                digest.update(&row.key);
                digest.update([u8::from(row.after.is_some())]);
                if let Some(bytes) = &row.after {
                    digest.update((bytes.len() as u64).to_be_bytes());
                    digest.update(bytes.as_slice());
                }
            }
            self.header.witness = digest.finalize().into();
        }
        Ok(Write {
            before: self.before,
            after: self.header.fingerprint()?,
            before_revision: self.before_revision,
            header: self.header,
            rows: self.rows,
        })
    }
}

impl Write {
    /// Stage only; callers own the transaction and must abort on any error.
    /// Both ordinary catalog commits and the existing namespace participant use
    /// this exact check, so an error cannot partially publish a row or index.
    pub(in crate::enrollment) fn stage(
        &self,
        tx: &WriteTransaction,
        local: IdentityHash,
    ) -> Result<(), EnrollmentError> {
        let mut header = tx.open_table(HEADER).map_err(db_error)?;
        if header.len().map_err(db_error)? != 1 {
            return Err(EnrollmentError::Corrupt);
        }
        let actual = Header::decode(
            header
                .get(0)
                .map_err(db_error)?
                .ok_or(EnrollmentError::Corrupt)?
                .value(),
            local,
        )?;
        if actual.revision != self.before_revision || actual.fingerprint()? != self.before {
            return Err(EnrollmentError::Conflict);
        }
        for change in &self.rows {
            let mut table = tx.open_table(change.table).map_err(db_error)?;
            if table
                .get(b"".as_slice())
                .map_err(db_error)?
                .as_ref()
                .map(|row| row.value())
                != Some(MARKER)
            {
                return Err(EnrollmentError::Corrupt);
            }
            let current = table
                .get(change.key.as_slice())
                .map_err(db_error)?
                .as_ref()
                .map(|row| hash(row.value()));
            if current != change.before {
                return Err(EnrollmentError::Conflict);
            }
            match &change.after {
                Some(bytes) => {
                    table
                        .insert(change.key.as_slice(), bytes.as_slice())
                        .map_err(db_error)?;
                }
                None => {
                    table.remove(change.key.as_slice()).map_err(db_error)?;
                }
            }
        }
        header
            .insert(
                0,
                encode_with_limit(&self.header, Header::MAX_ENCODED_BYTES)?.as_slice(),
            )
            .map_err(db_error)?;
        Ok(())
    }
}

fn logical_bytes(key: &[u8], value: &[u8]) -> Result<u64, EnrollmentError> {
    u64::try_from(key.len())
        .ok()
        .and_then(|key| {
            u64::try_from(value.len())
                .ok()
                .and_then(|value| key.checked_add(value))
        })
        .ok_or(EnrollmentError::Capacity)
}

fn membership_count<'a>(counts: &'a mut Counts, row: &rows::Incarnation) -> &'a mut u64 {
    match row {
        rows::Incarnation::Live(_) => &mut counts.live,
        rows::Incarnation::Archived(_) => &mut counts.archived,
        rows::Incarnation::Retired(_) => &mut counts.retired,
    }
}
