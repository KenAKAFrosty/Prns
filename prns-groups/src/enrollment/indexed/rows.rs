//! Catalog-owned rows. The authoritative MLS control/Welcome bytes remain in
//! their namespace; catalog rows hold only the routing/provenance needed for
//! exact access, summary pages and durable local deletion.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Current {
    /// Last assigned generation, including after local deletion. A later
    /// invitation cannot reuse a namespace targeted by an older purge.
    pub generation: u32,
    pub title: String,
    pub owner: [u8; 16],
    pub lifecycle: GroupLifecycle,
    pub ready: bool,
    pub member_count: u32,
    /// Pending outgoing offers reserve admission only until Added or terminal.
    pub pending_offers: u32,
    pub head: Option<[u8; 32]>,
    /// The exact local Leave outbox row. Namespace control updates cannot
    /// clear this admission fence before its terminal proof is committed.
    pub departure: Option<[u8; 32]>,
    /// Hidden immediately when the delete fence commits. Physical purging is a
    /// bounded continuation over this group's exact incarnation keys.
    pub visible: bool,
    pub purge_through: Option<u32>,
    pub invitations_deleted_through: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) enum Incarnation {
    Live(Entry),
    Archived(Entry),
    Retired(RetiredIncarnation),
}

/// No secret, copied terminal proof, audience, or invitation vector is needed in
/// this fence. Those exact associations have their own keyed catalog rows.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct RetiredIncarnation {
    pub group: [u8; 32],
    pub profile: crate::GroupProfile,
    pub owner: Member,
    pub local: Member,
    pub namespace: [u8; 16],
    pub incarnation: [u8; 24],
    pub purged: bool,
}

impl RetiredIncarnation {
    pub(in crate::enrollment) fn from_entry(entry: &Entry, purged: bool) -> Self {
        Self {
            group: entry.group,
            profile: entry.profile,
            owner: entry.owner.clone(),
            local: entry.local.clone(),
            namespace: entry.namespace,
            incarnation: entry.incarnation,
            purged,
        }
    }
}

impl Current {
    pub(in crate::enrollment) fn pending(entry: &Entry) -> Self {
        Self {
            generation: entry.generation,
            title: entry.title.clone(),
            owner: entry.owner.rns,
            lifecycle: GroupLifecycle::Active,
            ready: false,
            member_count: 0,
            head: None,
            departure: None,
            pending_offers: 0,
            visible: true,
            purge_through: None,
            invitations_deleted_through: 0,
        }
    }
}

impl Incarnation {
    pub(in crate::enrollment) fn profile(&self) -> crate::GroupProfile {
        match self {
            Self::Live(entry) | Self::Archived(entry) => entry.profile,
            Self::Retired(entry) => entry.profile,
        }
    }
    pub(in crate::enrollment) fn owner(&self) -> &Member {
        match self {
            Self::Live(entry) | Self::Archived(entry) => &entry.owner,
            Self::Retired(entry) => &entry.owner,
        }
    }
    pub(in crate::enrollment) fn store_incarnation(&self) -> [u8; 24] {
        match self {
            Self::Live(entry) | Self::Archived(entry) => entry.incarnation,
            Self::Retired(entry) => entry.incarnation,
        }
    }

    pub(in crate::enrollment) fn namespace(&self) -> [u8; 16] {
        match self {
            Self::Live(entry) | Self::Archived(entry) => entry.namespace,
            Self::Retired(entry) => entry.namespace,
        }
    }

    pub(in crate::enrollment) fn validate_key(
        &self,
        key: keys::IncarnationKey,
        local: IdentityHash,
    ) -> Result<(), EnrollmentError> {
        let (group, generation) = keys::split_incarnation(&key)?;
        let (actual_group, actual_local, incarnation, profile) = match self {
            Self::Live(entry) | Self::Archived(entry) => {
                if entry.generation != generation
                    || entry.title.trim().is_empty()
                    || entry.title.len() > profile::MAX_TITLE_BYTES
                {
                    return Err(EnrollmentError::Corrupt);
                }
                (
                    entry.group,
                    entry.local.rns,
                    entry.incarnation,
                    entry.profile,
                )
            }
            Self::Retired(entry) => (
                entry.group,
                entry.local.rns,
                entry.incarnation,
                entry.profile,
            ),
        };
        if actual_group != group || actual_local != local.into_bytes() {
            return Err(EnrollmentError::Corrupt);
        }
        StoreIncarnation::new(incarnation).map_err(|_| EnrollmentError::Corrupt)?;
        profile.validate().map_err(|_| EnrollmentError::Corrupt)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Notice {
    pub terminal: [u8; 32],
    pub recipient: [u8; 16],
    pub acknowledged: bool,
}

/// Close retains its audience across Remove and re-Join. Multiple outstanding
/// offers to one peer share one future Close notice; an admitted peer remains
/// in the audience after its final offer is pruned.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Audience {
    pub admitted: bool,
    pub offers: u64,
    pub close_notice_funded: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct TerminalRow {
    pub proof: Terminal,
    /// Bounded fan-out continues after this recipient, independently of which
    /// delivery waiters remain alive. A completed cursor is never restarted.
    pub after: Option<[u8; 16]>,
    pub complete: bool,
}

/// Exact request results committed by a namespace are indexed in that same
/// physical commit. Their lookup must not fall back to scanning every group.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct Receipt {
    pub request: Request,
    pub namespace: Option<NamespaceReceipt>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct NamespaceReceipt {
    pub group: [u8; 32],
    pub generation: u32,
    pub incarnation: [u8; 24],
}

/// The wire Leave stays unchanged; its local durable binding survives a new
/// current generation without consulting that generation's membership.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct DepartureRow {
    pub departure: Departure,
    pub namespace: NamespaceReceipt,
}

/// The admitted member is retained with the namespace association so a later
/// Remove proof can resolve this invitation without scanning control history.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::enrollment) struct InvitationBinding {
    pub namespace: NamespaceReceipt,
    pub member: Member,
}

impl Receipt {
    pub(in crate::enrollment) fn retirement_key(
        &self,
    ) -> Result<keys::TimeItemKey, EnrollmentError> {
        let token = prns_groups_core::retry::RequestToken::parse(self.request.id)
            .map_err(|_| EnrollmentError::Corrupt)?;
        let issued = if self.namespace.is_some() {
            // A namespace still owns this exact proof. Ordinary catalog receipt
            // retirement cannot remove its lookup and force a namespace scan.
            u64::MAX
        } else {
            token.issued_secs()
        };
        Ok(keys::time_item(issued, self.request.id))
    }
}

impl Read {
    pub(in crate::enrollment) fn group_profile(
        &self,
        group: [u8; 32],
    ) -> Result<Option<crate::GroupProfile>, EnrollmentError> {
        let Some(current) = self.current(group)? else {
            return Ok(None);
        };
        Ok(Some(
            self.incarnation(keys::incarnation(group, current.generation))?
                .ok_or(EnrollmentError::Corrupt)?
                .profile(),
        ))
    }

    pub(in crate::enrollment) fn group_offer(
        &self,
        group: [u8; 32],
    ) -> Result<Option<Invite>, EnrollmentError> {
        let first = keys::group_item(group, [0; 32]);
        let last = keys::group_item(group, [u8::MAX; 32]);
        let mut range = self
            .invitation_groups
            .range::<&[u8]>(first.as_slice()..=last.as_slice())
            .map_err(db_error)?;
        let Some(row) = range.next() else {
            return Ok(None);
        };
        let (_, value) = row.map_err(db_error)?;
        let id = value
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        self.invitation(id)
    }

    pub(in crate::enrollment) fn entry(
        &self,
        group: [u8; 32],
    ) -> Result<Option<Entry>, EnrollmentError> {
        let Some(current) = self.current(group)?.filter(|row| row.visible) else {
            return Ok(None);
        };
        match self
            .incarnation(keys::incarnation(group, current.generation))?
            .ok_or(EnrollmentError::Corrupt)?
        {
            Incarnation::Live(entry) => Ok(Some(entry)),
            _ => Err(EnrollmentError::Corrupt),
        }
    }

    pub(in crate::enrollment) fn departure(
        &self,
        id: [u8; 32],
    ) -> Result<Option<Departure>, EnrollmentError> {
        let Some(bytes) = self.departures.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let stored: DepartureRow = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        let row = stored.departure;
        if row.id != id || row.member.rns != self.header.local || row.owner.rns == self.header.local
        {
            return Err(EnrollmentError::Corrupt);
        }
        row.member
            .endpoint()
            .map_err(|_| EnrollmentError::Corrupt)?;
        row.owner.endpoint().map_err(|_| EnrollmentError::Corrupt)?;
        let current = self.current(row.group)?.ok_or(EnrollmentError::Corrupt)?;
        if current.owner != row.owner.rns || stored.namespace.group != row.group {
            return Err(EnrollmentError::Corrupt);
        }
        let incarnation = self
            .incarnation(keys::incarnation(row.group, stored.namespace.generation))?
            .ok_or(EnrollmentError::Corrupt)?;
        if incarnation.store_incarnation() != stored.namespace.incarnation {
            return Err(EnrollmentError::Corrupt);
        }
        match incarnation {
            Incarnation::Live(entry) | Incarnation::Archived(entry)
                if entry.owner == row.owner && entry.local == row.member => {}
            Incarnation::Retired(entry)
                if entry.owner == row.owner && entry.local == row.member => {}
            _ => return Err(EnrollmentError::Corrupt),
        }
        Ok(Some(row))
    }

    pub(in crate::enrollment) fn cancellation(
        &self,
        id: [u8; 32],
    ) -> Result<Option<Cancellation>, EnrollmentError> {
        let Some(bytes) = self.cancellations.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let row: Cancellation = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        if row.invitation != id || row.owner == self.header.local {
            return Err(EnrollmentError::Corrupt);
        }
        let invite = self.invitation(id)?;
        if let Some(invite) = &invite {
            if row.group != invite.offer.group
                || row.owner != invite.offer.owner.rns
                || row.expires != invite.offer.expires
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        let expiry = keys::time_item(row.expires, id);
        if self
            .cancellation_expiry
            .get(expiry.as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|r| r.value())
            != Some(id.as_slice())
        {
            return Err(EnrollmentError::Corrupt);
        }
        maintenance::check_incoming_index(self, row.owner, id, false, invite.is_none())?;
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn terminal(
        &self,
        id: [u8; 32],
    ) -> Result<Option<TerminalRow>, EnrollmentError> {
        let Some(bytes) = self.terminals.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let row: TerminalRow = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        if row.proof.id()? != id {
            return Err(EnrollmentError::Corrupt);
        }
        let key = keys::group_item(row.proof.statement.group, id);
        if self
            .terminal_groups
            .get(key.as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|r| r.value())
            != Some(id.as_slice())
        {
            return Err(EnrollmentError::Corrupt);
        }
        let subject = keys::terminal_subject(row.proof.statement.group, &row.proof.statement.kind);
        if self
            .terminal_subjects
            .get(subject.as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|r| r.value())
            != Some(id.as_slice())
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }

    pub(in crate::enrollment) fn terminal_subject(
        &self,
        group: [u8; 32],
        kind: &TerminalKind,
    ) -> Result<Option<TerminalRow>, EnrollmentError> {
        let subject = keys::terminal_subject(group, kind);
        let Some(value) = self
            .terminal_subjects
            .get(subject.as_slice())
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let id = value
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let row = self.terminal(id)?.ok_or(EnrollmentError::Corrupt)?;
        if row.proof.statement.group != group || &row.proof.statement.kind != kind {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }

    pub(in crate::enrollment) fn notice(
        &self,
        id: [u8; 32],
    ) -> Result<Option<Notice>, EnrollmentError> {
        let Some(bytes) = self.notices.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let row: Notice = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        let proof = self
            .terminal(row.terminal)?
            .ok_or(EnrollmentError::Corrupt)?
            .proof;
        let notice = Notification {
            proof,
            recipient: row.recipient,
            acknowledged: row.acknowledged,
        };
        if Enrollment::notice_id(&notice)? != id {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn invitation_binding(
        &self,
        id: [u8; 32],
    ) -> Result<Option<InvitationBinding>, EnrollmentError> {
        let Some(bytes) = self
            .invitation_bindings
            .get(id.as_slice())
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let binding: InvitationBinding =
            decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        let incarnation = self
            .incarnation(keys::incarnation(
                binding.namespace.group,
                binding.namespace.generation,
            ))?
            .ok_or(EnrollmentError::Corrupt)?;
        if binding.namespace.incarnation != incarnation.store_incarnation() {
            return Err(EnrollmentError::Corrupt);
        }
        binding.member.endpoint()?;
        if let Some(invite) = self.invitation(id)? {
            if invite.offer.group != binding.namespace.group
                || invite.offer.recipient != binding.member.rns
                || &invite.offer.owner != incarnation.owner()
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        Ok(Some(binding))
    }

    pub(in crate::enrollment) fn audience(
        &self,
        group: [u8; 32],
        recipient: [u8; 16],
    ) -> Result<Option<Audience>, EnrollmentError> {
        let key = keys::audience(group, recipient);
        let Some(bytes) = self.audience.get(key.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let row: Audience = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        if !row.admitted && row.offers == 0 {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(row))
    }
    pub(in crate::enrollment) fn current(
        &self,
        group: [u8; 32],
    ) -> Result<Option<Current>, EnrollmentError> {
        let Some(bytes) = self.current.get(group.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        decode_current(bytes.value(), self.header.limits.row_bytes()?).map(Some)
    }

    pub(in crate::enrollment) fn incarnation(
        &self,
        key: keys::IncarnationKey,
    ) -> Result<Option<Incarnation>, EnrollmentError> {
        let Some(bytes) = self.incarnations.get(key.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let incarnation: Incarnation = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        incarnation.validate_key(key, IdentityHash::new(self.header.local))?;
        let namespace = incarnation.namespace();
        let bound = self
            .namespaces
            .get(namespace.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        if bound.value() != key {
            return Err(EnrollmentError::Corrupt);
        }
        let id = incarnation.store_incarnation();
        let bound = self
            .incarnation_ids
            .get(id.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?;
        if bound.value() != key {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(incarnation))
    }

    pub(in crate::enrollment) fn invitation(
        &self,
        id: [u8; 32],
    ) -> Result<Option<Invite>, EnrollmentError> {
        let Some(bytes) = self.invitations.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let invite: Invite = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        if invite.offer.id != id
            || invite.admission_revision == 0
            || invite.admission_revision > self.header.revision
        {
            return Err(EnrollmentError::Corrupt);
        }
        let group_key = keys::group_item(invite.offer.group, id);
        let peer_key = keys::peer_item(invite.offer.owner.rns, id);
        let expiry_key = keys::time_item(invite.offer.expires, id);
        for (table, key) in [
            (&self.invitation_groups, group_key.as_slice()),
            (&self.invitation_peers, peer_key.as_slice()),
            (&self.invitation_expiry, expiry_key.as_slice()),
        ] {
            if table
                .get(key)
                .map_err(db_error)?
                .as_ref()
                .map(|v| v.value())
                != Some(id.as_slice())
            {
                return Err(EnrollmentError::Corrupt);
            }
        }
        maintenance::check_incoming_index(
            self,
            invite.offer.owner.rns,
            id,
            true,
            maintenance::pending(self.header.local, &invite),
        )?;
        maintenance::check_outgoing_index(self, &invite)?;
        Ok(Some(invite))
    }

    pub(in crate::enrollment) fn receipt(
        &self,
        id: [u8; 32],
        fingerprint: [u8; 32],
    ) -> Result<Option<Receipt>, EnrollmentError> {
        let Some(bytes) = self.requests.get(id.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let receipt: Receipt = decode_row(bytes.value(), self.header.limits.row_bytes()?)?;
        if receipt.request.id != id {
            return Err(EnrollmentError::Corrupt);
        }
        if receipt.request.fingerprint != fingerprint {
            return Err(EnrollmentError::RequestConflict);
        }
        let key = receipt.retirement_key()?;
        if self
            .request_expiry
            .get(key.as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|row| row.value())
            != Some(id.as_slice())
        {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some(receipt))
    }
}

pub(in crate::enrollment) fn decode_current(
    bytes: &[u8],
    limit: usize,
) -> Result<Current, EnrollmentError> {
    let mut current: Current = decode_row(bytes, limit)?;
    if current.title.trim().is_empty()
        || current.title.len() > profile::MAX_TITLE_BYTES
        || current
            .purge_through
            .is_some_and(|generation| generation > current.generation)
    {
        return Err(EnrollmentError::Corrupt);
    }
    // Decoding scratch is independently row-bounded. The retained title owns
    // exactly its UTF-8 length, so the page's maximum valid row is knowable.
    current.title = current.title.into_boxed_str().into_string();
    Ok(current)
}

pub(in crate::enrollment) fn decode_row<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    max_row_bytes: usize,
) -> Result<T, EnrollmentError> {
    #[cfg(test)]
    work::decoded();
    if bytes.len() > max_row_bytes {
        return Err(EnrollmentError::Capacity);
    }
    serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Corrupt)
}
