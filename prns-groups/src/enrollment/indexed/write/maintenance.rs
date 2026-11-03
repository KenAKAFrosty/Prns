use super::super::maintenance::{pending, Incoming};
use super::*;

impl Edit {
    fn outgoing_reserved(&self, read: &Read, invite: &Invite) -> Result<bool, EnrollmentError> {
        Ok(invite.offer.owner.rns == self.header.local
            && invite.state == InvitationState::Pending
            && self
                .projected::<rows::InvitationBinding>(
                    INVITATION_BINDINGS,
                    &invite.offer.id,
                    &read.invitation_bindings,
                )?
                .is_none())
    }

    fn pending_offer_index(
        &mut self,
        read: &Read,
        invite: &Invite,
        add: bool,
    ) -> Result<(), EnrollmentError> {
        let key = keys::audience(invite.offer.group, invite.offer.recipient);
        let original = read
            .pending_recipients
            .get(key.as_slice())
            .map_err(db_error)?;
        let pending = if let Some(row) = self
            .rows
            .iter()
            .find(|row| row.table.name() == PENDING_RECIPIENTS.name() && row.key == key)
        {
            row.after.as_ref().map(|value| value.as_slice())
        } else {
            original.as_ref().map(|row| row.value())
        };
        if add && pending.is_some() {
            return Err(EnrollmentError::Conflict);
        }
        if !add && pending != Some(invite.offer.id.as_slice()) {
            return Err(EnrollmentError::Corrupt);
        }
        let mut current: rows::Current = self
            .projected(CURRENT, &invite.offer.group, &read.current)?
            .ok_or(EnrollmentError::Missing)?;
        if current.owner != self.header.local || invite.offer.recipient == self.header.local {
            return Err(EnrollmentError::Invalid);
        }
        if add {
            current.pending_offers = current
                .pending_offers
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
            self.header.counts.pending_recipients = self
                .header
                .counts
                .pending_recipients
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        } else {
            current.pending_offers = current
                .pending_offers
                .checked_sub(1)
                .ok_or(EnrollmentError::Corrupt)?;
            self.header.counts.pending_recipients = self
                .header
                .counts
                .pending_recipients
                .checked_sub(1)
                .ok_or(EnrollmentError::Corrupt)?;
        }
        let prior_current = read
            .current
            .get(invite.offer.group.as_slice())
            .map_err(db_error)?;
        let value = self.encode(&current)?;
        self.replace(
            CURRENT,
            &invite.offer.group,
            prior_current.as_ref().map(|row| row.value()),
            Some(value),
        )?;
        self.replace(
            PENDING_RECIPIENTS,
            &key,
            original.as_ref().map(|row| row.value()),
            add.then(|| Zeroizing::new(invite.offer.id.to_vec())),
        )
    }

    /// The first Added binding consumes the exact unaccepted offer reservation;
    /// it does not erase the retained invitation or audience delivery evidence.
    pub(in crate::enrollment) fn consume_outgoing_reservation(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        if let Some(invite) = self.projected::<Invite>(INVITATIONS, &id, &read.invitations)? {
            if self.outgoing_reserved(read, &invite)? {
                self.pending_offer_index(read, &invite, false)?;
            }
        }
        Ok(self)
    }
    fn incoming_change(
        &mut self,
        read: &Read,
        peer: [u8; 16],
        invitation: bool,
        add: bool,
    ) -> Result<(), EnrollmentError> {
        let original = read.incoming_peers.get(peer.as_slice()).map_err(db_error)?;
        let old: Incoming = self
            .projected(INCOMING_PEERS, &peer, &read.incoming_peers)?
            .unwrap_or_default();
        let mut next = old;
        let (count, global) = if invitation {
            (
                &mut next.invitations,
                &mut self.header.counts.pending_invitations,
            )
        } else {
            (
                &mut next.cancellations,
                &mut self.header.counts.unsolicited_cancellations,
            )
        };
        if add {
            *count = count.checked_add(1).ok_or(EnrollmentError::Capacity)?;
            *global = global.checked_add(1).ok_or(EnrollmentError::Capacity)?;
            if *count > u64::from(self.header.limits.max_incoming_per_peer)
                || *global > u64::from(self.header.limits.max_incoming_invitations)
            {
                return Err(EnrollmentError::Capacity);
            }
        } else {
            *count = count.checked_sub(1).ok_or(EnrollmentError::Corrupt)?;
            *global = global.checked_sub(1).ok_or(EnrollmentError::Corrupt)?;
        }
        let old_empty = old == Incoming::default();
        let next_empty = next == Incoming::default();
        if old_empty && !next_empty {
            self.header.counts.incoming_peers = self
                .header
                .counts
                .incoming_peers
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        } else if !old_empty && next_empty {
            self.header.counts.incoming_peers = self
                .header
                .counts
                .incoming_peers
                .checked_sub(1)
                .ok_or(EnrollmentError::Corrupt)?;
        }
        let encoded = if next_empty {
            None
        } else {
            Some(self.encode(&next)?)
        };
        self.replace(
            INCOMING_PEERS,
            &peer,
            original.as_ref().map(|r| r.value()),
            encoded,
        )
    }

    fn incoming_index(
        &mut self,
        read: &Read,
        peer: [u8; 16],
        id: [u8; 32],
        invitation: bool,
        add: bool,
    ) -> Result<(), EnrollmentError> {
        let key = keys::peer_item(peer, id);
        let (definition, table) = if invitation {
            (PENDING_INVITATIONS, &read.pending_invitations)
        } else {
            (UNSOLICITED_CANCELLATIONS, &read.unsolicited_cancellations)
        };
        let prior = table.get(key.as_slice()).map_err(db_error)?;
        if prior.as_ref().is_some_and(|r| r.value() != id) {
            return Err(EnrollmentError::Corrupt);
        }
        self.incoming_change(read, peer, invitation, add)?;
        self.replace(
            definition,
            &key,
            prior.as_ref().map(|r| r.value()),
            add.then(|| Zeroizing::new(id.to_vec())),
        )
    }

    pub(in crate::enrollment) fn put_invitation(
        mut self,
        read: &Read,
        invite: &Invite,
    ) -> Result<Self, EnrollmentError> {
        let id = invite.offer.id;
        let prior = read.invitations.get(id.as_slice()).map_err(db_error)?;
        if prior.is_some() {
            read.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        let old: Option<Invite> = self.projected(INVITATIONS, &id, &read.invitations)?;
        if old.as_ref().is_some_and(|old| old.offer != invite.offer) {
            return Err(EnrollmentError::Conflict);
        }
        // Only the catalog writer assigns this immutable admission fact. A
        // caller's presentation update cannot move an old offer past Delete.
        let mut normalized = invite.clone();
        normalized.admission_revision = match old.as_ref() {
            Some(old) if old.admission_revision != 0 => old.admission_revision,
            Some(_) => return Err(EnrollmentError::Corrupt),
            None => self
                .before_revision
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?,
        };
        let invite = &normalized;
        let previous_reserve = old
            .as_ref()
            .map(|old| obligations::invitation_reserve(old, self.budget.row_bytes))
            .transpose()?
            .unwrap_or(0);
        let next_reserve = obligations::invitation_reserve(invite, self.budget.row_bytes)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(previous_reserve)
            .and_then(|n| n.checked_add(next_reserve))
            .ok_or(EnrollmentError::Corrupt)?;
        let cancellation: Option<Cancellation> =
            self.projected(CANCELLATIONS, &id, &read.cancellations)?;
        if cancellation.as_ref().is_some_and(|c| {
            c.owner != invite.offer.owner.rns
                || c.group != invite.offer.group
                || c.expires != invite.offer.expires
        }) {
            return Err(EnrollmentError::Authentication);
        }
        let was_reserved = old
            .as_ref()
            .map(|old| self.outgoing_reserved(read, old))
            .transpose()?
            .unwrap_or(false);
        let now_reserved = self.outgoing_reserved(read, invite)?;
        if was_reserved != now_reserved {
            self.pending_offer_index(read, invite, now_reserved)?;
        }
        let was_pending = old
            .as_ref()
            .is_some_and(|old| pending(self.header.local, old));
        let now_pending = pending(self.header.local, invite);
        if was_pending != now_pending {
            self.incoming_index(read, invite.offer.owner.rns, id, true, now_pending)?;
        }
        let value = self.encode(invite)?;
        self.replace(
            INVITATIONS,
            &id,
            prior.as_ref().map(|r| r.value()),
            Some(value),
        )?;
        if old.is_none() {
            self.fund_offer_terminal(read, invite)?;
            let group = keys::group_item(invite.offer.group, id);
            let peer = keys::peer_item(invite.offer.owner.rns, id);
            let expires = keys::time_item(invite.offer.expires, id);
            for (table, existing, key) in [
                (INVITATION_GROUPS, &read.invitation_groups, group.as_slice()),
                (INVITATION_PEERS, &read.invitation_peers, peer.as_slice()),
                (
                    INVITATION_EXPIRY,
                    &read.invitation_expiry,
                    expires.as_slice(),
                ),
            ] {
                if existing.get(key).map_err(db_error)?.is_some() {
                    return Err(EnrollmentError::Corrupt);
                }
                self.replace(table, key, None, Some(Zeroizing::new(id.to_vec())))?;
            }
            self = self.index_invitation_work(read, id, true)?;
            if invite.offer.owner.rns == self.header.local {
                self =
                    self.offer_audience(read, invite.offer.group, invite.offer.recipient, true)?;
            } else if cancellation.is_some() {
                self.incoming_index(read, invite.offer.owner.rns, id, false, false)?;
            }
            self.header.counts.invitations = self
                .header
                .counts
                .invitations
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn remove_invitation(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let invite: Invite = self
            .projected(INVITATIONS, &id, &read.invitations)?
            .ok_or(EnrollmentError::Missing)?;
        let prior = read.invitations.get(id.as_slice()).map_err(db_error)?;
        if prior.is_some() {
            read.invitation(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(obligations::invitation_reserve(
                &invite,
                self.budget.row_bytes,
            )?)
            .ok_or(EnrollmentError::Corrupt)?;
        if self.outgoing_reserved(read, &invite)? {
            self.pending_offer_index(read, &invite, false)?;
        }
        if pending(self.header.local, &invite) {
            self.incoming_index(read, invite.offer.owner.rns, id, true, false)?;
        }
        if invite.offer.owner.rns == self.header.local {
            self = self.offer_audience(read, invite.offer.group, invite.offer.recipient, false)?;
        } else if self
            .projected::<Cancellation>(CANCELLATIONS, &id, &read.cancellations)?
            .is_some()
        {
            self.incoming_index(read, invite.offer.owner.rns, id, false, true)?;
        }
        self.release_offer_terminal(read, &invite)?;
        self.replace(INVITATIONS, &id, prior.as_ref().map(|r| r.value()), None)?;
        let group = keys::group_item(invite.offer.group, id);
        let peer = keys::peer_item(invite.offer.owner.rns, id);
        let expires = keys::time_item(invite.offer.expires, id);
        for (table, original, key) in [
            (INVITATION_GROUPS, &read.invitation_groups, group.as_slice()),
            (INVITATION_PEERS, &read.invitation_peers, peer.as_slice()),
            (
                INVITATION_EXPIRY,
                &read.invitation_expiry,
                expires.as_slice(),
            ),
        ] {
            let prior = original.get(key).map_err(db_error)?;
            self.replace(table, key, prior.as_ref().map(|row| row.value()), None)?;
        }
        self = self.index_invitation_work(read, id, false)?;
        self.header.counts.invitations = self
            .header
            .counts
            .invitations
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        Ok(self)
    }

    pub(in crate::enrollment) fn put_cancellation(
        mut self,
        read: &Read,
        cancellation: &Cancellation,
    ) -> Result<Self, EnrollmentError> {
        let id = cancellation.invitation;
        if cancellation.owner == self.header.local {
            return Err(EnrollmentError::Invalid);
        }
        let prior = read.cancellations.get(id.as_slice()).map_err(db_error)?;
        if prior.is_some() {
            read.cancellation(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        let old: Option<Cancellation> = self.projected(CANCELLATIONS, &id, &read.cancellations)?;
        if let Some(old) = &old {
            if old.group != cancellation.group
                || old.owner != cancellation.owner
                || old.expires != cancellation.expires
            {
                return Err(EnrollmentError::Authentication);
            }
        }
        let invite: Option<Invite> = self.projected(INVITATIONS, &id, &read.invitations)?;
        if invite.as_ref().is_some_and(|i| {
            i.offer.owner.rns != cancellation.owner
                || i.offer.group != cancellation.group
                || i.offer.expires != cancellation.expires
        }) {
            return Err(EnrollmentError::Authentication);
        }
        let value = self.encode(cancellation)?;
        self.replace(
            CANCELLATIONS,
            &id,
            prior.as_ref().map(|r| r.value()),
            Some(value),
        )?;
        if old.is_none() {
            let key = keys::time_item(cancellation.expires, id);
            if read
                .cancellation_expiry
                .get(key.as_slice())
                .map_err(db_error)?
                .is_some()
            {
                return Err(EnrollmentError::Corrupt);
            }
            self.replace(
                CANCELLATION_EXPIRY,
                &key,
                None,
                Some(Zeroizing::new(id.to_vec())),
            )?;
            if invite.is_none() {
                self.incoming_index(read, cancellation.owner, id, false, true)?;
            }
            self.header.counts.cancellations = self
                .header
                .counts
                .cancellations
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?;
        }
        Ok(self)
    }

    pub(in crate::enrollment) fn remove_cancellation(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let row: Cancellation = self
            .projected(CANCELLATIONS, &id, &read.cancellations)?
            .ok_or(EnrollmentError::Missing)?;
        let prior = read.cancellations.get(id.as_slice()).map_err(db_error)?;
        if prior.is_some() {
            read.cancellation(id)?.ok_or(EnrollmentError::Corrupt)?;
        }
        if self
            .projected::<Invite>(INVITATIONS, &id, &read.invitations)?
            .is_none()
        {
            self.incoming_index(read, row.owner, id, false, false)?;
        }
        self.replace(CANCELLATIONS, &id, prior.as_ref().map(|r| r.value()), None)?;
        let key = keys::time_item(row.expires, id);
        let prior_expiry = read
            .cancellation_expiry
            .get(key.as_slice())
            .map_err(db_error)?;
        self.replace(
            CANCELLATION_EXPIRY,
            &key,
            prior_expiry.as_ref().map(|row| row.value()),
            None,
        )?;
        self.header.counts.cancellations = self
            .header
            .counts
            .cancellations
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        Ok(self)
    }

    pub(in crate::enrollment::indexed) fn retire_receipt(
        mut self,
        read: &Read,
        id: [u8; 32],
    ) -> Result<Self, EnrollmentError> {
        let prior = read
            .requests
            .get(id.as_slice())
            .map_err(db_error)?
            .ok_or(EnrollmentError::Missing)?;
        let row: rows::Receipt = rows::decode_row(prior.value(), self.budget.row_bytes)?;
        if row.request.id != id || row.namespace.is_some() {
            return Err(EnrollmentError::Conflict);
        }
        let key = row.retirement_key()?;
        self.replace(REQUESTS, &id, Some(prior.value()), None)?;
        self.replace(REQUEST_EXPIRY, &key, Some(&id), None)?;
        self.header.counts.requests = self
            .header
            .counts
            .requests
            .checked_sub(1)
            .ok_or(EnrollmentError::Corrupt)?;
        Ok(self)
    }
}

impl Edit {
    /// The planner reserves the exact maximum key/slot shape of one candidate
    /// before consuming the edit. Value bounds below come from the same owned
    /// row serializer. A tight selection reports Capacity before any commit;
    /// it never silently returns a permanently zero-progress successful step.
    fn maintenance_fits(
        &self,
        rows: usize,
        keys: usize,
        values: usize,
    ) -> Result<bool, EnrollmentError> {
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
                .checked_add(keys)
                .and_then(|n| n.checked_add(values))
                .and_then(|n| n.checked_add(slots))
                .is_some_and(|n| n <= self.budget.bytes))
    }

    fn incoming_scratch(&self) -> Result<usize, EnrollmentError> {
        Ok(self
            .encode(&Incoming {
                invitations: u64::MAX,
                cancellations: u64::MAX,
            })?
            .capacity())
    }

    fn cancellation_fits(&self, unknown: bool) -> Result<bool, EnrollmentError> {
        let (rows, keys, values) = if unknown {
            (4, 32 + 40 + 48 + 16, self.incoming_scratch()?)
        } else {
            (2, 32 + 40, 0)
        };
        self.maintenance_fits(rows, keys, values)
    }

    fn pending_offer_scratch(
        &self,
        read: &Read,
        group: [u8; 32],
    ) -> Result<usize, EnrollmentError> {
        let mut current: rows::Current = self
            .projected(CURRENT, &group, &read.current)?
            .ok_or(EnrollmentError::Corrupt)?;
        current.pending_offers = u32::MAX;
        Ok(self.encode(&current)?.capacity())
    }

    fn invitation_removal_fits(
        &self,
        read: &Read,
        invite: &Invite,
        was_pending: bool,
        was_reserved: bool,
        cancellation: bool,
    ) -> Result<bool, EnrollmentError> {
        let mut rows = 4usize;
        let mut keys = 32usize + 64 + 48 + 40;
        let mut values = 0usize;
        let (work_rows, work_keys) = super::super::work::invitation_work_removal_cost();
        rows = rows
            .checked_add(work_rows)
            .ok_or(EnrollmentError::Capacity)?;
        keys = keys
            .checked_add(work_keys)
            .ok_or(EnrollmentError::Capacity)?;
        if invite.offer.owner.rns == self.header.local {
            rows += 1;
            keys += 48;
            // An admitted audience remains; an offer-only audience can vanish.
            // This is the maximum serialized counter, not a copied inventory.
            values = self
                .encode(&rows::Audience {
                    admitted: false,
                    offers: u64::MAX,
                    close_notice_funded: false,
                })?
                .capacity();
        } else if was_pending {
            rows += 2;
            keys += 48 + 16;
            values = self.incoming_scratch()?;
        }
        if invite.offer.owner.rns != self.header.local {
            // Last offer refunds its Close proof, or transfers the exact
            // obligation reference to another retained offer of this group.
            rows += 1;
            keys += 73;
            values = values.checked_add(32).ok_or(EnrollmentError::Capacity)?;
        }
        if was_reserved {
            rows += 2;
            keys += 48 + 32;
            values = values
                .checked_add(self.pending_offer_scratch(read, invite.offer.group)?)
                .ok_or(EnrollmentError::Capacity)?;
        }
        if cancellation {
            // The matching cancellation expires with the offer. Delete it
            // first in this edit so removing the offer never transiently
            // consumes another unsolicited-cancellation admission slot.
            rows += 2;
            keys += 32 + 40;
        }
        self.maintenance_fits(rows, keys, values)
    }

    fn invitation_update_fits(
        &self,
        read: &Read,
        was_pending: bool,
        was_reserved: bool,
        next: &Invite,
    ) -> Result<bool, EnrollmentError> {
        let mut rows = 1;
        let mut keys = 32;
        let mut values = self.encode(next)?.capacity();
        if was_pending != pending(self.header.local, next) {
            rows += 2;
            keys += 48 + 16;
            values = values
                .checked_add(self.incoming_scratch()?)
                .ok_or(EnrollmentError::Capacity)?;
        }
        if was_reserved != self.outgoing_reserved(read, next)? {
            rows += 2;
            keys += 48 + 32;
            values = values
                .checked_add(self.pending_offer_scratch(read, next.offer.group)?)
                .ok_or(EnrollmentError::Capacity)?;
        }
        self.maintenance_fits(rows, keys, values)
    }

    pub(in crate::enrollment::indexed) fn maintain(
        mut self,
        read: &Read,
        now: u64,
    ) -> Result<super::super::maintenance::Plan, EnrollmentError> {
        use super::super::maintenance::{lower, start, Cursor, Phase, Plan};
        use std::ops::Bound;
        if now < 3600 {
            return Err(EnrollmentError::Invalid);
        }
        let now = now.max(read.header.invitation_time_floor);
        // The caller's trusted maintenance clock is a durable admission fence,
        // even when this bounded turn visits another category or no expiry row.
        self.header.invitation_time_floor = now;
        let maximum = usize::try_from(read.header.limits.max_maintenance_rows)
            .map_err(|_| EnrollmentError::Corrupt)?;
        let mut result = crate::enrollment::Maintenance::default();
        let mut output = None;
        match read.header.maintenance_phase {
            Phase::Requests => {
                let floor = crate::enrollment::maintenance::RETRY_POLICY
                    .retirement_floor(now, read.header.request_retired_before);
                let end = keys::time_item(floor.min(u64::MAX - 1), [u8::MAX; 32]);
                for row in read
                    .request_expiry
                    .range::<&[u8]>((lower(None), Bound::Included(end.as_slice())))
                    .map_err(db_error)?
                    .take(maximum)
                {
                    let (key, value) = row.map_err(db_error)?;
                    let cursor = Cursor::decode(key.value())?;
                    if value.value() != cursor.id {
                        return Err(EnrollmentError::Corrupt);
                    }
                    if !self.maintenance_fits(2, 32 + 40, 0)? {
                        if result.retired_requests == 0 {
                            return Err(EnrollmentError::Capacity);
                        }
                        break;
                    }
                    self = self.retire_receipt(read, cursor.id)?;
                    // Exact retained lookup precedes the fence, so another
                    // request at this second can still replay until its turn.
                    self.header.request_retired_before =
                        self.header.request_retired_before.max(cursor.time);
                    result.retired_requests += 1;
                }
            }
            Phase::Cancellations => {
                let end = keys::time_item(now, [u8::MAX; 32]);
                for (removed, row) in read
                    .cancellation_expiry
                    .range::<&[u8]>((lower(None), Bound::Included(end.as_slice())))
                    .map_err(db_error)?
                    .take(maximum)
                    .enumerate()
                {
                    let (key, value) = row.map_err(db_error)?;
                    let cursor = Cursor::decode(key.value())?;
                    if value.value() != cursor.id {
                        return Err(EnrollmentError::Corrupt);
                    }
                    let unknown = read.invitation(cursor.id)?.is_none();
                    if !self.cancellation_fits(unknown)? {
                        if removed == 0 {
                            return Err(EnrollmentError::Capacity);
                        }
                        break;
                    }
                    self = self.remove_cancellation(read, cursor.id)?;
                    self.header.invitation_time_floor =
                        self.header.invitation_time_floor.max(cursor.time);
                }
            }
            Phase::Invitations => {
                let after = start(read.header.invitation_maintenance_after);
                let mut range = read
                    .invitation_expiry
                    .range::<&[u8]>((
                        lower(after.as_ref().map(|key| key.as_slice())),
                        Bound::Unbounded,
                    ))
                    .map_err(db_error)?;
                for inspected in 0..maximum {
                    let Some(row) = range.next() else {
                        self.header.invitation_maintenance_after = None;
                        break;
                    };
                    let (key, value) = row.map_err(db_error)?;
                    let cursor = Cursor::decode(key.value())?;
                    if value.value() != cursor.id {
                        return Err(EnrollmentError::Corrupt);
                    }
                    let mut invite = read
                        .invitation(cursor.id)?
                        .ok_or(EnrollmentError::Corrupt)?;
                    let was_pending = pending(self.header.local, &invite);
                    let was_reserved = self.outgoing_reserved(read, &invite)?;
                    let action =
                        super::super::maintenance::invitation_action(read, &mut invite, now)?;
                    match action {
                        super::super::maintenance::Action::Keep => {}
                        super::super::maintenance::Action::Update => {
                            if !self.invitation_update_fits(
                                read,
                                was_pending,
                                was_reserved,
                                &invite,
                            )? {
                                if inspected == 0 {
                                    return Err(EnrollmentError::Capacity);
                                }
                                break;
                            }
                            self = self.put_invitation(read, &invite)?;
                        }
                        super::super::maintenance::Action::Remove => {
                            let cancellation = read.cancellation(cursor.id)?;
                            if cancellation.as_ref().is_some_and(|c| c.expires > now) {
                                return Err(EnrollmentError::Corrupt);
                            }
                            if !self.invitation_removal_fits(
                                read,
                                &invite,
                                was_pending,
                                was_reserved,
                                cancellation.is_some(),
                            )? {
                                // Two individually valid rows can need more
                                // projection slots together than were selected.
                                // Retire the expired cancellation first, retain
                                // this cursor, then revisit the invitation.
                                if cancellation.is_some() && self.cancellation_fits(false)? {
                                    self = self.remove_cancellation(read, cursor.id)?;
                                    self.header.invitation_time_floor =
                                        self.header.invitation_time_floor.max(invite.offer.expires);
                                    break;
                                }
                                if inspected == 0 {
                                    return Err(EnrollmentError::Capacity);
                                }
                                break;
                            }
                            if cancellation.is_some() {
                                self = self.remove_cancellation(read, cursor.id)?;
                            }
                            self = self.remove_invitation(read, cursor.id)?;
                            self.header.invitation_time_floor =
                                self.header.invitation_time_floor.max(invite.offer.expires);
                            result.retired_invitations += 1;
                        }
                    }
                    self.header.invitation_maintenance_after = Some(cursor);
                }
            }
            Phase::Terminals => {
                let after = read.header.terminal_after;
                let mut range = read
                    .terminals
                    .range::<&[u8]>((
                        lower(after.as_ref().map(|key| key.as_slice())),
                        Bound::Unbounded,
                    ))
                    .map_err(db_error)?;
                for _ in 0..maximum {
                    let Some(row) = range.next() else {
                        self.header.terminal_after = None;
                        break;
                    };
                    let (key, _) = row.map_err(db_error)?;
                    let id = key
                        .value()
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?;
                    let terminal = read.terminal(id)?.ok_or(EnrollmentError::Corrupt)?;
                    self.header.terminal_after = Some(id);
                    if terminal.proof.statement.owner.rns == self.header.local
                        && matches!(terminal.proof.statement.kind, TerminalKind::Closed)
                        && !terminal.complete
                    {
                        let limit = crate::PageLimits {
                            max_rows: self.header.limits.max_maintenance_rows,
                            max_bytes: self.header.limits.max_transaction_bytes,
                        };
                        self = self.fanout_close(read, id, limit)?;
                        break;
                    }
                }
            }
            Phase::Obligations => {
                let after = read
                    .header
                    .obligation_after
                    .map(super::super::terminal_obligations::Cursor::key);
                let mut range = read
                    .terminal_obligations
                    .range::<&[u8]>((
                        lower(after.as_ref().map(|key| key.as_slice())),
                        Bound::Unbounded,
                    ))
                    .map_err(db_error)?;
                for inspected in 0..maximum {
                    let Some(row) = range.next() else {
                        self.header.obligation_after = None;
                        break;
                    };
                    let (key, value) = row.map_err(db_error)?;
                    let cursor = super::super::terminal_obligations::Cursor::from_key(key.value())?;
                    let source = value
                        .value()
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?;
                    let statement = read.obligation_statement(cursor.key(), source)?;
                    if matches!(statement.kind, TerminalKind::Removed(_))
                        && read
                            .terminal_subject(statement.group, &TerminalKind::Closed)?
                            .is_some()
                    {
                        if !self.maintenance_fits(1, 73, 0)? {
                            if inspected == 0 {
                                return Err(EnrollmentError::Capacity);
                            }
                            break;
                        }
                        self.retire_terminal_obligation(read, cursor.key(), &statement)?;
                    }
                    self.header.obligation_after = Some(cursor);
                }
            }
            Phase::Outputs => {
                let after = read.header.output_maintenance_after;
                let mut range = read
                    .current
                    .range::<&[u8]>((
                        lower(after.as_ref().map(|key| key.as_slice())),
                        Bound::Unbounded,
                    ))
                    .map_err(db_error)?;
                for _ in 0..maximum {
                    let Some(row) = range.next() else {
                        self.header.output_maintenance_after = None;
                        break;
                    };
                    let (key, _) = row.map_err(db_error)?;
                    let group = key
                        .value()
                        .try_into()
                        .map_err(|_| EnrollmentError::Corrupt)?;
                    let current = read.current(group)?.ok_or(EnrollmentError::Corrupt)?;
                    self.header.output_maintenance_after = Some(group);
                    let membership = read
                        .incarnation(keys::incarnation(group, current.generation))?
                        .ok_or(EnrollmentError::Corrupt)?;
                    if let rows::Incarnation::Live(entry) = membership {
                        output = Some(entry);
                        break;
                    }
                }
            }
        }
        self.header.maintenance_phase = read.header.maintenance_phase.next();
        result.request_retired_before = self.header.request_retired_before;
        Ok(Plan {
            write: self.finish()?,
            result,
            output,
        })
    }
}
