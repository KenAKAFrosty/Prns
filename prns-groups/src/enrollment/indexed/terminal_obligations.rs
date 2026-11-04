//! Exact, admission-funded terminal work. Keys are the already-owned terminal
//! subjects; no supported group/member ceiling is multiplied into a reservation.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Cursor {
    group: [u8; 32],
    member: Option<([u8; 32], u64)>,
}
impl Cursor {
    pub(super) fn from_key(key: &[u8]) -> Result<Self, EnrollmentError> {
        let key: &keys::TerminalSubjectKey =
            key.try_into().map_err(|_| EnrollmentError::Corrupt)?;
        let group = key[..32].try_into().map_err(|_| EnrollmentError::Corrupt)?;
        let member = match key[32] {
            0 if key[33..].iter().all(|byte| *byte == 0) => None,
            1 => Some((
                key[33..65]
                    .try_into()
                    .map_err(|_| EnrollmentError::Corrupt)?,
                u64::from_be_bytes(key[65..].try_into().map_err(|_| EnrollmentError::Corrupt)?),
            )),
            _ => return Err(EnrollmentError::Corrupt),
        };
        Ok(Self { group, member })
    }
    pub(super) fn key(self) -> keys::TerminalSubjectKey {
        let mut key = [0; 73];
        key[..32].copy_from_slice(&self.group);
        if let Some((endpoint, incarnation)) = self.member {
            key[32] = 1;
            key[33..65].copy_from_slice(&endpoint);
            key[65..].copy_from_slice(&incarnation.to_be_bytes());
        }
        key
    }
}

impl Read {
    pub(super) fn obligation_statement(
        &self,
        key: keys::TerminalSubjectKey,
        source: [u8; 32],
    ) -> Result<TerminalStatement, EnrollmentError> {
        let cursor = Cursor::from_key(&key)?;
        let current = self.current(cursor.group)?;
        let (owner, title, kind) = if cursor.member.is_some() {
            let binding = self
                .invitation_binding(source)?
                .ok_or(EnrollmentError::Corrupt)?;
            if binding.namespace.group != cursor.group
                || keys::terminal_subject(
                    cursor.group,
                    &TerminalKind::Removed(binding.member.clone()),
                ) != key
            {
                return Err(EnrollmentError::Corrupt);
            }
            let incarnation = self
                .incarnation(keys::incarnation(
                    cursor.group,
                    binding.namespace.generation,
                ))?
                .ok_or(EnrollmentError::Corrupt)?;
            if incarnation.owner().rns != self.header.local
                && binding.member.rns != self.header.local
            {
                return Err(EnrollmentError::Corrupt);
            }
            (
                incarnation.owner().clone(),
                current.ok_or(EnrollmentError::Corrupt)?.title,
                TerminalKind::Removed(binding.member),
            )
        } else if let Some(current) = current {
            if source != cursor.group {
                return Err(EnrollmentError::Corrupt);
            }
            let incarnation = self
                .incarnation(keys::incarnation(cursor.group, current.generation))?
                .ok_or(EnrollmentError::Corrupt)?;
            if incarnation.owner().rns != current.owner {
                return Err(EnrollmentError::Corrupt);
            }
            (
                incarnation.owner().clone(),
                current.title,
                TerminalKind::Closed,
            )
        } else {
            let invite = self.invitation(source)?.ok_or(EnrollmentError::Corrupt)?;
            if invite.offer.group != cursor.group
                || invite.offer.owner.rns == self.header.local
                || invite.offer.recipient != self.header.local
            {
                return Err(EnrollmentError::Corrupt);
            }
            (invite.offer.owner, invite.offer.title, TerminalKind::Closed)
        };
        Ok(TerminalStatement {
            version: WIRE_VERSION,
            group: cursor.group,
            title,
            owner,
            head: [u8::MAX; 32],
            revision: u64::MAX,
            kind,
        })
    }

    pub(super) fn inspect_obligations(&self) -> Result<u64, EnrollmentError> {
        let mut reserved = 0u64;
        for row in self
            .terminal_obligations
            .range::<&[u8]>((
                std::ops::Bound::Excluded(b"".as_slice()),
                std::ops::Bound::Unbounded,
            ))
            .map_err(db_error)?
        {
            let (key, source) = row.map_err(db_error)?;
            let key = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let source = source
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let statement = self.obligation_statement(key, source)?;
            if self
                .terminal_subject(statement.group, &statement.kind)?
                .is_some()
            {
                return Err(EnrollmentError::Corrupt);
            }
            reserved = reserved
                .checked_add(obligations::terminal_bytes(
                    &statement,
                    self.header.local,
                    self.header.limits.row_bytes()?,
                )?)
                .ok_or(EnrollmentError::Corrupt)?;
        }
        for row in self
            .current
            .range::<&[u8]>((
                std::ops::Bound::Excluded(b"".as_slice()),
                std::ops::Bound::Unbounded,
            ))
            .map_err(db_error)?
        {
            let (key, _) = row.map_err(db_error)?;
            let group = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            self.current(group)?.ok_or(EnrollmentError::Corrupt)?;
            if self
                .terminal_subject(group, &TerminalKind::Closed)?
                .is_none()
            {
                self.require_obligation(
                    keys::terminal_subject(group, &TerminalKind::Closed),
                    group,
                )?;
            }
        }
        for row in self
            .invitation_bindings
            .range::<&[u8]>((
                std::ops::Bound::Excluded(b"".as_slice()),
                std::ops::Bound::Unbounded,
            ))
            .map_err(db_error)?
        {
            let (key, _) = row.map_err(db_error)?;
            let invitation = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let binding = self
                .invitation_binding(invitation)?
                .ok_or(EnrollmentError::Corrupt)?;
            let incarnation = self
                .incarnation(keys::incarnation(
                    binding.namespace.group,
                    binding.namespace.generation,
                ))?
                .ok_or(EnrollmentError::Corrupt)?;
            if incarnation.owner().rns != self.header.local
                && binding.member.rns != self.header.local
            {
                continue;
            }
            let kind = TerminalKind::Removed(binding.member.clone());
            if let Some(terminal) = self.terminal_subject(binding.namespace.group, &kind)? {
                let notification = Notification {
                    proof: terminal.proof,
                    recipient: binding.member.rns,
                    acknowledged: false,
                };
                if incarnation.owner().rns == self.header.local {
                    self.notice(Enrollment::notice_id(&notification)?)?
                        .ok_or(EnrollmentError::Corrupt)?;
                }
            } else if self
                .terminal_subject(binding.namespace.group, &TerminalKind::Closed)?
                .is_none()
            {
                self.require_obligation(
                    keys::terminal_subject(binding.namespace.group, &kind),
                    invitation,
                )?;
            }
        }
        for row in self
            .invitations
            .range::<&[u8]>((
                std::ops::Bound::Excluded(b"".as_slice()),
                std::ops::Bound::Unbounded,
            ))
            .map_err(db_error)?
        {
            let (key, _) = row.map_err(db_error)?;
            let invite = self
                .invitation(
                    key.value()
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?,
                )?
                .ok_or(EnrollmentError::Corrupt)?;
            if invite.offer.owner.rns != self.header.local
                && self.current(invite.offer.group)?.is_none()
                && self
                    .terminal_subject(invite.offer.group, &TerminalKind::Closed)?
                    .is_none()
            {
                let key = keys::terminal_subject(invite.offer.group, &TerminalKind::Closed);
                let source = self
                    .terminal_obligations
                    .get(key.as_slice())
                    .map_err(db_error)?
                    .ok_or(EnrollmentError::Corrupt)?;
                let statement = self.obligation_statement(
                    key,
                    source
                        .value()
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?,
                )?;
                if statement.owner != invite.offer.owner || statement.title != invite.offer.title {
                    return Err(EnrollmentError::Corrupt);
                }
            }
        }
        Ok(reserved)
    }

    fn require_obligation(
        &self,
        key: keys::TerminalSubjectKey,
        source: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        if self
            .terminal_obligations
            .get(key.as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|row| row.value())
            != Some(source.as_slice())
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(())
    }
}
