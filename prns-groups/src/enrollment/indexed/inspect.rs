//! Strict cold validation streams the selected tables with one bounded row of
//! scratch. Ordinary open, point reads, pages and mutation staging never invoke
//! it and never enumerate the database's unrelated namespace tables.
use super::*;

impl Read {
    pub(in crate::enrollment) fn inspect(&self) -> Result<(), EnrollmentError> {
        self.inspect_purge_cursor()?;
        let mut bytes = 0u64;
        for (table, width) in [
            (&self.current, 32),
            (&self.incarnations, 36),
            (&self.namespaces, 16),
            (&self.incarnation_ids, 24),
            (&self.invitations, 32),
            (&self.invitation_groups, 64),
            (&self.invitation_peers, 48),
            (&self.invitation_expiry, 40),
            (&self.invitation_bindings, 32),
            (&self.audience, 48),
            (&self.requests, 32),
            (&self.request_expiry, 40),
            (&self.departures, 32),
            (&self.notices, 32),
            (&self.terminals, 32),
            (&self.terminal_groups, 64),
            (&self.terminal_subjects, 73),
            (&self.cancellations, 32),
            (&self.cancellation_expiry, 40),
            (&self.pending_invitations, 48),
            (&self.unsolicited_cancellations, 48),
            (&self.incoming_peers, 16),
            (&self.pending_recipients, 48),
            (&self.work_ids, 32),
            (&self.terminal_obligations, 73),
        ] {
            for row in table
                .range::<&[u8]>((
                    std::ops::Bound::Excluded(b"".as_slice()),
                    std::ops::Bound::Unbounded,
                ))
                .map_err(db_error)?
            {
                let (key, value) = row.map_err(db_error)?;
                if key.value().len() != width
                    || value.value().len() > self.header.limits.row_bytes()?
                {
                    return Err(EnrollmentError::Corrupt);
                }
                let row_bytes = u64::try_from(key.value().len())
                    .ok()
                    .and_then(|n| n.checked_add(u64::try_from(value.value().len()).ok()?))
                    .ok_or(EnrollmentError::Corrupt)?;
                bytes = bytes
                    .checked_add(row_bytes)
                    .ok_or(EnrollmentError::Corrupt)?;
            }
        }
        if bytes != self.header.bytes {
            return Err(EnrollmentError::Corrupt);
        }

        let mut reserved = self.inspect_obligations()?;
        let mut live = 0u64;
        let mut archived = 0u64;
        let mut retired = 0u64;
        for row in self.incarnations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let key: keys::IncarnationKey = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let (group, generation) = keys::split_incarnation(&key)?;
            let incarnation = self.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
            reserved = reserved
                .checked_add(obligations::incarnation_reserve(
                    &incarnation,
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
            let current = self.current(group)?.ok_or(EnrollmentError::Corrupt)?;
            if current.invitations_deleted_through > self.header.revision {
                return Err(EnrollmentError::Corrupt);
            }
            if generation > current.generation {
                return Err(EnrollmentError::Corrupt);
            }
            match incarnation {
                rows::Incarnation::Live(entry) => {
                    if generation != current.generation
                        || !current.visible
                        || entry.title != current.title
                        || entry.owner.rns != current.owner
                    {
                        return Err(EnrollmentError::Corrupt);
                    }
                    entry.identity()?;
                    live = live.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                }
                rows::Incarnation::Archived(entry) => {
                    if generation >= current.generation {
                        return Err(EnrollmentError::Corrupt);
                    }
                    entry.identity()?;
                    archived = archived.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                }
                rows::Incarnation::Retired(_) => {
                    retired = retired.checked_add(1).ok_or(EnrollmentError::Corrupt)?;
                }
            }
        }
        if (live, archived, retired)
            != (
                self.header.counts.live,
                self.header.counts.archived,
                self.header.counts.retired,
            )
        {
            return Err(EnrollmentError::Corrupt);
        }
        for row in self.current.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let group = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let current = self.current(group)?.ok_or(EnrollmentError::Corrupt)?;
            reserved = reserved
                .checked_add(obligations::current_reserve(
                    &current,
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
            let incarnation = self
                .incarnation(keys::incarnation(group, current.generation))?
                .ok_or(EnrollmentError::Corrupt)?;
            if current.visible != matches!(incarnation, rows::Incarnation::Live(_)) {
                return Err(EnrollmentError::Corrupt);
            }
            if let Some(id) = current.departure {
                let departure = self.departure(id)?.ok_or(EnrollmentError::Corrupt)?;
                let value = self
                    .departures
                    .get(id.as_slice())
                    .map_err(db_error)?
                    .ok_or(EnrollmentError::Corrupt)?;
                let stored: rows::DepartureRow =
                    rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
                if departure.group != group || stored.namespace.generation != current.generation {
                    return Err(EnrollmentError::Corrupt);
                }
            }
        }
        for row in self.invitations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let invite = self.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
            reserved = reserved
                .checked_add(obligations::invitation_reserve(
                    &invite,
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
            invite
                .offer
                .profile
                .validate()
                .map_err(|_| EnrollmentError::Corrupt)?;
            if invite.offer.title.trim().is_empty()
                || invite.offer.title.len() > profile::MAX_TITLE_BYTES
                || invite.offer.expires < invite.offer.created
                || invite.offer.recipient == invite.offer.owner.rns
                || (invite.offer.owner.rns != self.header.local
                    && invite.offer.recipient != self.header.local)
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        for row in self.requests.iter().map_err(db_error)? {
            let (key, value) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let receipt: rows::Receipt =
                rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
            let receipt = self
                .receipt(id, receipt.request.fingerprint)?
                .ok_or(EnrollmentError::Corrupt)?;
            if let Some(binding) = receipt.namespace {
                let row = self
                    .incarnation(keys::incarnation(binding.group, binding.generation))?
                    .ok_or(EnrollmentError::Corrupt)?;
                if row.store_incarnation() != binding.incarnation {
                    return Err(EnrollmentError::Corrupt);
                }
            }
        }
        for row in self.invitation_bindings.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            self.invitation_binding(id)?
                .ok_or(EnrollmentError::Corrupt)?;
        }
        for row in self.audience.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let group = key
                .value()
                .get(..32)
                .ok_or(EnrollmentError::Corrupt)?
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let recipient = key
                .value()
                .get(32..)
                .ok_or(EnrollmentError::Corrupt)?
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let audience = self
                .audience(group, recipient)?
                .ok_or(EnrollmentError::Corrupt)?;
            let current = self.current(group)?.ok_or(EnrollmentError::Corrupt)?;
            if current.owner != self.header.local || recipient == self.header.local {
                return Err(EnrollmentError::Corrupt);
            }
            reserved = reserved
                .checked_add(obligations::audience_reserve(
                    Some(&audience),
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
        }
        for row in self.terminals.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let terminal = self.terminal(id)?.ok_or(EnrollmentError::Corrupt)?;
            terminal.proof.verify(
                terminal.proof.statement.group,
                &terminal.proof.statement.owner,
            )?;
            if let Some(current) = self.current(terminal.proof.statement.group)? {
                let incarnation = self
                    .incarnation(keys::incarnation(
                        terminal.proof.statement.group,
                        current.generation,
                    ))?
                    .ok_or(EnrollmentError::Corrupt)?;
                if incarnation.owner() != &terminal.proof.statement.owner
                    || current.title != terminal.proof.statement.title
                {
                    return Err(EnrollmentError::Corrupt);
                }
            }
            reserved = reserved
                .checked_add(obligations::terminal_reserve(
                    Some(&terminal),
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
            if matches!(terminal.proof.statement.kind, TerminalKind::Closed)
                && terminal.proof.statement.owner.rns == self.header.local
            {
                let group = terminal.proof.statement.group;
                let first = keys::audience(group, [0; 16]);
                let last = keys::audience(group, [u8::MAX; 16]);
                for row in self
                    .audience
                    .range::<&[u8]>(first.as_slice()..=last.as_slice())
                    .map_err(db_error)?
                {
                    let (key, value) = row.map_err(db_error)?;
                    let peer: [u8; 16] = key
                        .value()
                        .get(32..)
                        .ok_or(EnrollmentError::Corrupt)?
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?;
                    let audience: rows::Audience =
                        rows::decode_row(value.value(), self.header.limits.row_bytes()?)?;
                    if audience.close_notice_funded {
                        if terminal.complete || terminal.after.is_some_and(|cursor| peer <= cursor)
                        {
                            return Err(EnrollmentError::Corrupt);
                        }
                    } else {
                        let notice = Notification {
                            proof: terminal.proof.clone(),
                            recipient: peer,
                            acknowledged: false,
                        };
                        self.notice(Enrollment::notice_id(&notice)?)?
                            .ok_or(EnrollmentError::Corrupt)?;
                    }
                }
            }
        }
        if reserved != self.header.reserved {
            return Err(EnrollmentError::Corrupt);
        }
        for row in self.notices.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            self.notice(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        for row in self.departures.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            self.departure(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        for row in self.cancellations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            self.cancellation(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        maintenance::inspect_incoming(self)?;
        maintenance::inspect_outgoing(self)?;
        self.inspect_work()?;
        Ok(())
    }
}
