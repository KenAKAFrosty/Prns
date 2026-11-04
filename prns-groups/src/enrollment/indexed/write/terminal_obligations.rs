use super::*;

impl Edit {
    fn fund_terminal(
        &mut self,
        read: &Read,
        source: [u8; 32],
        statement: &TerminalStatement,
    ) -> Result<(), EnrollmentError> {
        let key = keys::terminal_subject(statement.group, &statement.kind);
        if read
            .terminal_subject(statement.group, &statement.kind)?
            .is_some()
        {
            return Err(EnrollmentError::Conflict);
        }
        let original = read
            .terminal_obligations
            .get(key.as_slice())
            .map_err(db_error)?;
        let previous = self
            .rows
            .iter()
            .find(|row| row.table.name() == TERMINAL_OBLIGATIONS.name() && row.key == key)
            .map(|row| row.after.as_deref().map(Vec::as_slice));
        let value = previous.unwrap_or_else(|| original.as_ref().map(|row| row.value()));
        if let Some(value) = value {
            if value == source {
                return Ok(());
            }
            // Incoming offers may fund Close before a membership exists. The
            // first accepted incarnation transfers that same obligation to its
            // stable group key; it must preserve immutable owner/title facts.
            if !matches!(statement.kind, TerminalKind::Closed) {
                return Err(EnrollmentError::Conflict);
            }
            let id: [u8; 32] = value.try_into().map_err(|_| EnrollmentError::Corrupt)?;
            let invite: Invite = self
                .projected(INVITATIONS, &id, &read.invitations)?
                .ok_or(EnrollmentError::Corrupt)?;
            if invite.offer.group != statement.group
                || invite.offer.owner != statement.owner
                || invite.offer.title != statement.title
            {
                return Err(EnrollmentError::Authentication);
            }
            // Another offer shares the existing admission; only an accepted
            // group transfers the durable reference away from that offer.
            if source == statement.group {
                self.replace(
                    TERMINAL_OBLIGATIONS,
                    &key,
                    original.as_ref().map(|row| row.value()),
                    Some(Zeroizing::new(source.to_vec())),
                )?;
            }
            return Ok(());
        }
        let reserve =
            obligations::terminal_bytes(statement, self.header.local, self.budget.row_bytes)?;
        self.replace(
            TERMINAL_OBLIGATIONS,
            &key,
            original.as_ref().map(|row| row.value()),
            Some(Zeroizing::new(source.to_vec())),
        )?;
        self.header.reserved = self
            .header
            .reserved
            .checked_add(reserve)
            .ok_or(EnrollmentError::Capacity)?;
        self.header.counts.terminal_obligations = self
            .header
            .counts
            .terminal_obligations
            .checked_add(1)
            .ok_or(EnrollmentError::Capacity)?;
        Ok(())
    }

    pub(super) fn fund_close(
        &mut self,
        read: &Read,
        key: keys::IncarnationKey,
        incarnation: &rows::Incarnation,
    ) -> Result<(), EnrollmentError> {
        let (group, _) = keys::split_incarnation(&key)?;
        let current: rows::Current = self
            .projected(CURRENT, &group, &read.current)?
            .ok_or(EnrollmentError::Corrupt)?;
        let statement = TerminalStatement {
            version: WIRE_VERSION,
            group,
            title: current.title,
            owner: incarnation.owner().clone(),
            head: [u8::MAX; 32],
            revision: u64::MAX,
            kind: TerminalKind::Closed,
        };
        self.fund_terminal(read, group, &statement)
    }

    pub(super) fn fund_remove(
        &mut self,
        read: &Read,
        invitation: [u8; 32],
        binding: &rows::InvitationBinding,
    ) -> Result<(), EnrollmentError> {
        let key = keys::incarnation(binding.namespace.group, binding.namespace.generation);
        let incarnation: rows::Incarnation = self
            .projected(INCARNATIONS, &key, &read.incarnations)?
            .ok_or(EnrollmentError::Corrupt)?;
        if incarnation.store_incarnation() != binding.namespace.incarnation {
            return Err(EnrollmentError::Corrupt);
        }
        if incarnation.owner().rns != self.header.local && binding.member.rns != self.header.local {
            return Ok(());
        }
        if incarnation.owner().rns == self.header.local && binding.member.rns == self.header.local {
            return Err(EnrollmentError::Corrupt);
        }
        let current: rows::Current = self
            .projected(CURRENT, &binding.namespace.group, &read.current)?
            .ok_or(EnrollmentError::Corrupt)?;
        let statement = TerminalStatement {
            version: WIRE_VERSION,
            group: binding.namespace.group,
            title: current.title,
            owner: incarnation.owner().clone(),
            head: [u8::MAX; 32],
            revision: u64::MAX,
            kind: TerminalKind::Removed(binding.member.clone()),
        };
        self.fund_terminal(read, invitation, &statement)
    }

    pub(super) fn consume_terminal_obligation(
        &mut self,
        read: &Read,
        proof: &Terminal,
    ) -> Result<(), EnrollmentError> {
        let key = keys::terminal_subject(proof.statement.group, &proof.statement.kind);
        let original = read
            .terminal_obligations
            .get(key.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        let source = original
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let expected = read.obligation_statement(key, source)?;
        if expected.group != proof.statement.group
            || expected.owner != proof.statement.owner
            || expected.title != proof.statement.title
            || expected.kind != proof.statement.kind
        {
            return Err(EnrollmentError::Corrupt);
        }
        self.retire_terminal_obligation(read, key, &expected)
    }

    pub(in crate::enrollment::indexed) fn retire_terminal_obligation(
        &mut self,
        read: &Read,
        key: keys::TerminalSubjectKey,
        statement: &TerminalStatement,
    ) -> Result<(), EnrollmentError> {
        let original = read
            .terminal_obligations
            .get(key.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        self.replace(TERMINAL_OBLIGATIONS, &key, Some(original.value()), None)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(obligations::terminal_bytes(
                statement,
                self.header.local,
                self.budget.row_bytes,
            )?)
            .ok_or(EnrollmentError::Corrupt)?;
        self.header.counts.terminal_obligations = self
            .header
            .counts
            .terminal_obligations
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        Ok(())
    }
    pub(super) fn fund_offer_terminal(
        &mut self,
        read: &Read,
        invite: &Invite,
    ) -> Result<(), EnrollmentError> {
        if invite.offer.owner.rns == self.header.local
            || self
                .projected::<rows::Current>(CURRENT, &invite.offer.group, &read.current)?
                .is_some()
            || read
                .terminal_subject(invite.offer.group, &TerminalKind::Closed)?
                .is_some()
        {
            return Ok(());
        }
        let statement = TerminalStatement {
            version: WIRE_VERSION,
            group: invite.offer.group,
            title: invite.offer.title.clone(),
            owner: invite.offer.owner.clone(),
            head: [u8::MAX; 32],
            revision: u64::MAX,
            kind: TerminalKind::Closed,
        };
        self.fund_terminal(read, invite.offer.id, &statement)
    }

    pub(super) fn release_offer_terminal(
        &mut self,
        read: &Read,
        invite: &Invite,
    ) -> Result<(), EnrollmentError> {
        if invite.offer.owner.rns == self.header.local
            || self
                .projected::<rows::Current>(CURRENT, &invite.offer.group, &read.current)?
                .is_some()
        {
            return Ok(());
        }
        let key = keys::terminal_subject(invite.offer.group, &TerminalKind::Closed);
        let original = read
            .terminal_obligations
            .get(key.as_slice())
            .map_err(db_error)?;
        let projected = self
            .rows
            .iter()
            .find(|row| row.table.name() == TERMINAL_OBLIGATIONS.name() && row.key == key)
            .map(|row| row.after.as_deref().map(Vec::as_slice));
        let value = projected.unwrap_or_else(|| original.as_ref().map(|row| row.value()));
        if value != Some(invite.offer.id.as_slice()) {
            return Ok(());
        }
        let mut replacement = None;
        // At most one candidate for each already-touched invitation plus one
        // surviving candidate. This bound is selected transaction work, not a
        // scan over retained history.
        for row in read
            .invitation_groups
            .range(
                keys::group_item(invite.offer.group, [0; 32]).as_slice()
                    ..=keys::group_item(invite.offer.group, [255; 32]).as_slice(),
            )
            .map_err(db_error)?
            .take(self.budget.rows.saturating_add(1))
        {
            let (_, value) = row.map_err(db_error)?;
            let id: [u8; 32] = value
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            if id == invite.offer.id {
                continue;
            }
            if self
                .projected::<Invite>(INVITATIONS, &id, &read.invitations)?
                .is_some()
            {
                replacement = Some(id);
                break;
            }
        }
        // Newly admitted same-edit offers are absent from the original index.
        if replacement.is_none() {
            for row in &self.rows {
                if row.table.name() == INVITATIONS.name() && row.key != invite.offer.id {
                    if let Some(value) = &row.after {
                        let candidate: Invite = rows::decode_row(value, self.budget.row_bytes)?;
                        if candidate.offer.group == invite.offer.group {
                            replacement = Some(candidate.offer.id);
                            break;
                        }
                    }
                }
            }
        }
        self.replace(
            TERMINAL_OBLIGATIONS,
            &key,
            original.as_ref().map(|row| row.value()),
            replacement.map(|id| Zeroizing::new(id.to_vec())),
        )?;
        if replacement.is_none() {
            let statement = TerminalStatement {
                version: WIRE_VERSION,
                group: invite.offer.group,
                title: invite.offer.title.clone(),
                owner: invite.offer.owner.clone(),
                head: [u8::MAX; 32],
                revision: u64::MAX,
                kind: TerminalKind::Closed,
            };
            self.header.reserved = self
                .header
                .reserved
                .checked_sub(obligations::terminal_bytes(
                    &statement,
                    self.header.local,
                    self.budget.row_bytes,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
            self.header.counts.terminal_obligations = self
                .header
                .counts
                .terminal_obligations
                .checked_sub(1)
                .ok_or(EnrollmentError::Corrupt)?;
        }
        Ok(())
    }
}
