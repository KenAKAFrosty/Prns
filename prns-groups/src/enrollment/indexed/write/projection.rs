//! Project committed namespace facts through the existing transaction
//! participant. All lookups are exact; a projection owns only changed rows.
use super::*;
use prns_groups_store::Mutation;
use prns_groups_store_redb::CommitContext;

impl Edit {
    pub(in crate::enrollment) fn namespace_projection(
        self,
        read: &Read,
        context: CommitContext,
        changes: &[Mutation],
    ) -> Result<Self, EnrollmentError> {
        let key: keys::IncarnationKey = read
            .namespaces
            .get(context.namespace.0.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Missing)?
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let incarnation = read.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
        let (group, generation) = keys::split_incarnation(&key)?;
        if read.generation_deleted(group, generation)? {
            return Err(EnrollmentError::Conflict);
        }
        if incarnation.store_incarnation() != *context.incarnation.as_bytes() {
            return Err(EnrollmentError::Conflict);
        }
        let entry = match incarnation {
            rows::Incarnation::Live(entry) | rows::Incarnation::Archived(entry) => entry,
            rows::Incarnation::Retired(_) => return Err(EnrollmentError::Conflict),
        };
        let binding = rows::NamespaceReceipt {
            group: entry.group,
            generation: entry.generation,
            incarnation: entry.incarnation,
        };
        let mut edit = Some(self);
        state::visit_mutations(changes, |projection| {
            edit = Some(
                edit.take()
                    .ok_or(EnrollmentError::Corrupt)?
                    .namespace_fact(read, &entry, &binding, projection)?,
            );
            Ok(())
        })?;
        edit.ok_or(EnrollmentError::Corrupt)
    }

    fn namespace_fact(
        mut self,
        read: &Read,
        entry: &Entry,
        binding: &rows::NamespaceReceipt,
        projection: state::Projection,
    ) -> Result<Self, EnrollmentError> {
        match projection {
            state::Projection::Header(summary) => {
                let mut current: rows::Current = self
                    .projected(CURRENT, &entry.group, &read.current)?
                    .ok_or(EnrollmentError::Corrupt)?;
                // An older archived namespace retains its own recovery facts;
                // it must never replace the current incarnation's summary.
                if current.generation == entry.generation {
                    if !current.visible {
                        return Err(EnrollmentError::Conflict);
                    }
                    current.lifecycle = summary.lifecycle;
                    current.ready = summary.ready;
                    current.member_count = summary.member_count;
                    current.head = summary.head;
                    self = self.put_current(read, entry.group, &current)?;
                    if summary.head.is_some() {
                        if let Some(invitation) = entry.invitation {
                            self = self.bind_invitation(
                                read,
                                invitation,
                                &rows::InvitationBinding {
                                    namespace: binding.clone(),
                                    member: entry.local.clone(),
                                },
                            )?;
                        }
                    }
                }
                Ok(self)
            }
            state::Projection::Added { invitation, member } => {
                if let Some(invite) = read.invitation(invitation)? {
                    if invite.offer.group != entry.group
                        || invite.offer.owner != entry.owner
                        || invite.offer.recipient != member.rns
                    {
                        return Err(EnrollmentError::Corrupt);
                    }
                }
                self = self.bind_invitation(
                    read,
                    invitation,
                    &rows::InvitationBinding {
                        namespace: binding.clone(),
                        member: member.clone(),
                    },
                )?;
                if entry.owner.rns == self.header.local && member.rns != self.header.local {
                    self = self.admit_audience(read, entry.group, member.rns)?;
                }
                Ok(self)
            }
            state::Projection::Request(request) => self.put_receipt(
                read,
                &rows::Receipt {
                    request,
                    namespace: Some(binding.clone()),
                },
            ),
            state::Projection::Terminal(proof) => {
                proof.verify(entry.group, &entry.owner)?;
                if proof.statement.title != entry.title {
                    return Err(EnrollmentError::Corrupt);
                }
                let existing = read.terminal(proof.id()?)?;
                let row = existing.unwrap_or_else(|| rows::TerminalRow {
                    complete: entry.owner.rns != self.header.local
                        || !matches!(proof.statement.kind, TerminalKind::Closed),
                    proof: proof.clone(),
                    after: None,
                });
                self = self.put_terminal(read, &row)?;
                if entry.owner.rns == self.header.local {
                    if let TerminalKind::Removed(member) = &proof.statement.kind {
                        self = self.removal_notice(read, &proof, member.rns)?;
                    }
                }
                let current: rows::Current = self
                    .projected(CURRENT, &entry.group, &read.current)?
                    .ok_or(EnrollmentError::Corrupt)?;
                if current.generation == entry.generation
                    && (matches!(proof.statement.kind, TerminalKind::Closed)
                        || matches!(&proof.statement.kind, TerminalKind::Removed(member) if member == &entry.local))
                {
                    self = self.clear_departure(read, entry.group)?;
                }
                Ok(self)
            }
        }
    }

    pub(in crate::enrollment::indexed) fn removal_notice(
        mut self,
        read: &Read,
        proof: &Terminal,
        recipient: [u8; 16],
    ) -> Result<Self, EnrollmentError> {
        if recipient == self.header.local {
            return Err(EnrollmentError::Corrupt);
        }
        let notification = Notification {
            proof: proof.clone(),
            recipient,
            acknowledged: false,
        };
        let id = Enrollment::notice_id(&notification)?;
        if let Some(existing) = self.projected::<rows::Notice>(NOTICES, &id, &read.notices)? {
            if existing.terminal != proof.id()? || existing.recipient != recipient {
                return Err(EnrollmentError::Corrupt);
            }
            return Ok(self);
        }
        let notice = rows::Notice {
            terminal: proof.id()?,
            recipient,
            acknowledged: false,
        };
        let encoded = self.encode(&notice)?;
        let prior = read.notices.get(id.as_slice()).map_err(db_error)?;
        self.replace(
            NOTICES,
            &id,
            prior.as_ref().map(|row| row.value()),
            Some(encoded),
        )?;
        self.header.counts.notices = self
            .header
            .counts
            .notices
            .checked_add(1)
            .ok_or(EnrollmentError::Capacity)?;
        self.index_work(read, id, super::super::work::WorkSource::Notice(id))
    }

    /// Leave commits its exact outbox and local admission fence together. The
    /// namespace's Active control remains authoritative until owner removal.
    pub(in crate::enrollment) fn put_departure(
        mut self,
        read: &Read,
        departure: &Departure,
    ) -> Result<Self, EnrollmentError> {
        let mut current: rows::Current = self
            .projected(CURRENT, &departure.group, &read.current)?
            .ok_or(EnrollmentError::Missing)?;
        if !current.visible || current.lifecycle != GroupLifecycle::Active {
            return Err(EnrollmentError::Conflict);
        }
        let key = keys::incarnation(departure.group, current.generation);
        let incarnation = read.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
        let entry = match incarnation {
            rows::Incarnation::Live(entry) => entry,
            _ => return Err(EnrollmentError::Conflict),
        };
        if entry.owner == entry.local
            || departure.owner != entry.owner
            || departure.member != entry.local
        {
            return Err(EnrollmentError::Authentication);
        }
        if current.departure.is_some_and(|id| id != departure.id) {
            return Err(EnrollmentError::Conflict);
        }
        let prior = read
            .departures
            .get(departure.id.as_slice())
            .map_err(db_error)?;
        let old: Option<rows::DepartureRow> =
            self.projected(DEPARTURES, &departure.id, &read.departures)?;
        if old.as_ref().is_some_and(|old| &old.departure != departure) {
            return Err(EnrollmentError::RequestConflict);
        }
        let value = self.encode(&rows::DepartureRow {
            departure: departure.clone(),
            namespace: rows::NamespaceReceipt {
                group: entry.group,
                generation: entry.generation,
                incarnation: entry.incarnation,
            },
        })?;
        self.replace(
            DEPARTURES,
            &departure.id,
            prior.as_ref().map(|r| r.value()),
            Some(value),
        )?;
        if old.is_none() {
            self.header.counts.departures = self
                .header
                .counts
                .departures
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        current.departure = Some(departure.id);
        self.put_current(read, departure.group, &current)?
            .index_work(
                read,
                work_id(b"leave", &departure.id),
                super::super::work::WorkSource::Departure(departure.id),
            )
    }

    pub(in crate::enrollment) fn clear_departure(
        self,
        read: &Read,
        group: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let current: rows::Current = self
            .projected(CURRENT, &group, &read.current)?
            .ok_or(EnrollmentError::Missing)?;
        let Some(id) = current.departure else {
            return Ok(self);
        };
        self.clear_departure_id(read, id)
    }

    pub(in crate::enrollment) fn clear_departure_id(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let Some(old) = self.projected::<rows::DepartureRow>(DEPARTURES, &id, &read.departures)?
        else {
            return Ok(self);
        };
        let group = old.departure.group;
        let mut current: rows::Current = self
            .projected(CURRENT, &group, &read.current)?
            .ok_or(EnrollmentError::Corrupt)?;
        let original = read.departures.get(id.as_slice()).map_err(db_error)?;
        self.replace(DEPARTURES, &id, original.as_ref().map(|r| r.value()), None)?;
        self.header.counts.departures = self
            .header
            .counts
            .departures
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        if current.departure == Some(id) {
            current.departure = None;
            self = self.put_current(read, group, &current)?;
        }
        self.remove_work(
            read,
            work_id(b"leave", &id),
            super::super::work::WorkSource::Departure(id),
        )
    }
}
