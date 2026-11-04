use super::*;

/// Cold-only namespace visitor. Borrowed native keys never escape this object.
/// Final counts include secondary keys, so orphan rows cannot hide behind a
/// smaller header. Reciprocal point checks below establish their exact mapping.
pub(in crate::enrollment) struct Inspection {
    counts: [u64; 17],
    has_values: bool,
    header: Option<Header>,
}
impl Inspection {
    pub(in crate::enrollment) fn new() -> Self {
        Self {
            counts: [0; 17],
            has_values: false,
            header: None,
        }
    }
    pub(in crate::enrollment) fn observe(
        &mut self,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), RedbError> {
        self.has_values = true;
        let Some(application) = prns_groups_mls::application_key(key) else {
            return Ok(());
        };
        if application == STATE_KEY {
            if self.header.is_some() {
                return Err(RedbError::Corrupt);
            }
            let header: Header = serde_json::from_slice(value).map_err(|_| RedbError::Corrupt)?;
            if header.version != VERSION {
                return Err(RedbError::Corrupt);
            }
            self.header = Some(header);
        } else if let Some(suffix) = application.strip_prefix(PREFIX) {
            let (&kind, id) = suffix.split_first().ok_or(RedbError::Corrupt)?;
            let length = match kind {
                CONTROL | CONTROL_ORDER | ADDED_EPOCH | ADDED_ORDER | TERMINAL_ORDER
                | ACK_ORDER | REQUEST_ORDER | PAST_MEMBER_ORDER => 8,
                CONTROL_HEAD | CONTROL_PARENT | ADDED | TERMINAL | ACK | REQUEST => 32,
                ADDED_MEMBER | PAST_MEMBER => 40,
                TERMINAL_SUBJECT => {
                    if id.first() == Some(&0) {
                        1
                    } else if id.first() == Some(&1) {
                        41
                    } else {
                        return Err(RedbError::Corrupt);
                    }
                }
                _ => return Err(RedbError::Corrupt),
            };
            if id.len() != length {
                return Err(RedbError::Corrupt);
            }
            let count = self
                .counts
                .get_mut(usize::from(kind) - 1)
                .ok_or(RedbError::Corrupt)?;
            *count = count.checked_add(1).ok_or(RedbError::CounterExhausted)?;
        } else if application.starts_with(b"enrollment/") {
            return Err(RedbError::Corrupt);
        }
        Ok(())
    }
    pub(in crate::enrollment) fn finish(self) -> Result<(), RedbError> {
        let Some(h) = self.header else {
            return if !self.has_values && self.counts.iter().all(|n| *n == 0) {
                Ok(())
            } else {
                Err(RedbError::Corrupt)
            };
        };
        let expected = [
            h.control_count,
            h.control_count,
            h.control_count,
            h.added_count,
            h.added_count,
            h.added_member_count,
            h.added_count,
            h.terminal_count,
            h.terminal_subject_count,
            h.terminal_count,
            h.ack_count,
            h.ack_count,
            h.management_count,
            h.management_count,
            h.past_member_count,
            h.past_member_count,
            h.control_count,
        ];
        if self.counts != expected {
            return Err(RedbError::Corrupt);
        }
        Ok(())
    }
}

/// Borrowed immutable group facts shared by live, archived and purge-pending
/// namespaces. This contains neither local secret nor duplicated durable state.
pub(in crate::enrollment) struct InspectionContext<'a> {
    pub group: [u8; 32],
    pub owner: &'a Member,
    pub title: &'a str,
    pub profile: crate::GroupProfile,
}
impl<'a> From<&'a Entry> for InspectionContext<'a> {
    fn from(entry: &'a Entry) -> Self {
        Self {
            group: entry.group,
            owner: &entry.owner,
            title: &entry.title,
            profile: entry.profile,
        }
    }
}

impl Snapshot {
    /// Strict startup validation, never called by a routine lexical adapter view.
    /// Each iteration holds only bounded rows and one current signed roster.
    #[cfg(test)]
    pub(in crate::enrollment) fn inspect(
        &self,
        adapter: &impl Facts,
        entry: &Entry,
    ) -> Result<(), EnrollmentError> {
        self.inspect_context(adapter, &InspectionContext::from(entry))
    }

    pub(in crate::enrollment) fn inspect_context(
        &self,
        adapter: &impl Facts,
        entry: &InspectionContext<'_>,
    ) -> Result<(), EnrollmentError> {
        self.validate_counts()?;
        if let Some(current) = &self.current {
            current.verify(entry.group, entry.owner, entry.title, entry.profile)?;
        }
        if let Some(tip) = &self.required_tip {
            tip.verify(entry.group, entry.owner, entry.title, entry.profile)?;
        }
        self.ready()?;
        let mut previous: Option<Signed> = None;
        for ordinal in 0..self.control_count {
            let id = required(read::<Vec<u8>>(
                adapter,
                &key(CONTROL_ORDER, &ordinal.to_be_bytes()),
            )?)?;
            let epoch = u64::from_be_bytes(id.try_into().map_err(|_| EnrollmentError::Corrupt)?);
            let control = required(self.control(adapter, epoch)?)?;
            control
                .signed
                .verify(entry.group, entry.owner, entry.title, entry.profile)?;
            if hash(&control.commit) != control.signed.manifest.commit_digest {
                return Err(EnrollmentError::Corrupt);
            }
            if required(read::<u64>(
                adapter,
                &key(CONTROL_HEAD, &control.signed.head()?),
            )?)? != epoch
                || required(read::<u64>(
                    adapter,
                    &key(CONTROL_PARENT, &control.signed.manifest.parent),
                )?)? != epoch
            {
                return Err(EnrollmentError::Corrupt);
            }
            if let Some(prior) = &previous {
                Enrollment::transition_context(entry, prior, &control.signed, &control.commit)?;
            }
            if let ControlKind::Remove(member) = &control.signed.manifest.kind {
                let past: PastMember = required(super::rows::read_row(
                    adapter,
                    PAST_MEMBER,
                    PAST_MEMBER_ORDER,
                    &member_key(member),
                    self.past_member_count,
                )?)?;
                if &past.member != member {
                    return Err(EnrollmentError::Corrupt);
                }
            }
            previous = Some(control.signed);
        }
        if let Some(last) = &previous {
            if self.current.as_ref() != Some(last) {
                return Err(EnrollmentError::Corrupt);
            }
        }
        for ordinal in 0..self.added_count {
            let added = self.added_at(adapter, ordinal)?;
            added
                .signed
                .verify(entry.group, entry.owner, entry.title, entry.profile)?;
            if !added.signed.manifest.members.contains(&added.member) {
                return Err(EnrollmentError::Corrupt);
            }
            let epoch = required(self.added_epoch(adapter, added.signed.manifest.epoch)?)?;
            let member = required(self.added_member(adapter, &added.member)?)?;
            if epoch.invitation != added.invitation
                || member.signed.manifest.epoch < added.signed.manifest.epoch
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        for ordinal in 0..self.terminal_count {
            let terminal = self.terminal_at(adapter, ordinal)?;
            terminal.verify(entry.group, entry.owner)?;
            let latest = required(self.terminal_subject(adapter, &terminal.statement.kind)?)?;
            if latest.statement.revision < terminal.statement.revision {
                return Err(EnrollmentError::Corrupt);
            }
        }
        for ordinal in 0..self.ack_count {
            let id = required(read::<Vec<u8>>(
                adapter,
                &key(ACK_ORDER, &ordinal.to_be_bytes()),
            )?)?;
            if !self.acknowledged(
                adapter,
                id.try_into().map_err(|_| EnrollmentError::Corrupt)?,
            )? {
                return Err(EnrollmentError::Corrupt);
            }
        }
        for ordinal in 0..self.management_count {
            let id = required(read::<Vec<u8>>(
                adapter,
                &key(REQUEST_ORDER, &ordinal.to_be_bytes()),
            )?)?;
            required(self.management(
                adapter,
                id.try_into().map_err(|_| EnrollmentError::Corrupt)?,
            )?)?;
        }
        for ordinal in 0..self.past_member_count {
            let past = self.past_record_at(adapter, ordinal)?;
            let control = required(self.control(adapter, past.removed_epoch)?)?;
            if !matches!(&control.signed.manifest.kind,ControlKind::Remove(member) if member==&past.member)
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        Ok(())
    }
}

impl Snapshot {
    pub(in crate::enrollment) fn inspect_catalog_requests(
        &self,
        facts: &impl Facts,
        catalog: &indexed::Read,
        group: [u8; 32],
        generation: u32,
        incarnation: [u8; 24],
    ) -> Result<(), EnrollmentError> {
        for ordinal in 0..self.management_count {
            let id = required(read::<Vec<u8>>(
                facts,
                &key(REQUEST_ORDER, &ordinal.to_be_bytes()),
            )?)?;
            let request = required(
                self.management(facts, id.try_into().map_err(|_| EnrollmentError::Corrupt)?)?,
            )?;
            let receipt = catalog
                .receipt(request.id, request.fingerprint)?
                .ok_or(EnrollmentError::Corrupt)?;
            if receipt.request.result != request.result
                || receipt.namespace.as_ref().is_none_or(|binding| {
                    binding.group != group
                        || binding.generation != generation
                        || binding.incarnation != incarnation
                })
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        Ok(())
    }
}
