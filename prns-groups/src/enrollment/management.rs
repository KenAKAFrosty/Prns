use super::*;

// A full wire control plus its independently signed terminal notice and replay
// record. Admission reserves this space for each currently removable member.
const TERMINAL_BYTES: usize = 8192;

impl<'db> Enrollment<'db> {
    pub(super) fn cancel_invitation_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        invitation: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            clock,
        } = admitted;
        if replay.is_some() {
            return Ok(());
        }
        let now = clock
            .ok_or(EnrollmentError::Clock)?
            .lower()
            .0
            .max(catalog.header.invitation_time_floor);
        let mut invite = catalog
            .invitation(invitation)?
            .ok_or(EnrollmentError::Missing)?;
        if invite.offer.owner.rns != self.local.into_bytes() {
            return Err(EnrollmentError::NotOwner);
        }
        if catalog.invitation_binding(invitation)?.is_some() {
            return Err(EnrollmentError::AlreadyJoined);
        }
        let entry = Self::find(&catalog, invite.offer.group)?;
        self.adapter(owner, &entry, None, |_, snapshot| {
            Self::require_active(&snapshot)
        })?;
        if !matches!(
            invite.state,
            InvitationState::Pending | InvitationState::Cancelled
        ) {
            return Err(EnrollmentError::Conflict);
        }
        invite.state = InvitationState::Cancelled;
        invite.acknowledged = false;
        invite.terminal_at = Some(now);
        invite.deferred_join = None;
        let edit = Edit::new(&catalog)?.put_invitation(&catalog, &invite)?;
        self.write(Self::remember(&catalog, edit, request, fingerprint, invitation)?.finish()?)
    }

    pub(super) fn dismiss_invitation_admitted(
        &self,
        _owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        invitation: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            clock,
        } = admitted;
        if replay.is_some() {
            return Ok(());
        }
        let now = clock.ok_or(EnrollmentError::Clock)?.lower().0;
        let mut invite = catalog
            .invitation(invitation)?
            .ok_or(EnrollmentError::Missing)?;
        if !Self::dismissible(catalog.invitation_view(invite.clone(), now)?.state) {
            return Err(EnrollmentError::Conflict);
        }
        invite.dismissed = true;
        let edit = Edit::new(&catalog)?.put_invitation(&catalog, &invite)?;
        self.write(Self::remember(&catalog, edit, request, fingerprint, invitation)?.finish()?)
    }
    pub(super) fn dismissible(state: InvitationState) -> bool {
        matches!(
            state,
            InvitationState::Joined
                | InvitationState::Declined
                | InvitationState::Expired
                | InvitationState::Cancelled
                | InvitationState::UnsupportedProfile
        )
    }
    pub(super) fn require_active(snapshot: &Snapshot) -> Result<(), EnrollmentError> {
        match snapshot.lifecycle {
            GroupLifecycle::Active => Ok(()),
            GroupLifecycle::Closed => Err(EnrollmentError::GroupClosed),
            _ => Err(EnrollmentError::NotJoined),
        }
    }
    pub(super) fn terminal(
        entry: &Entry,
        signed: &Signed,
        kind: TerminalKind,
    ) -> Result<Terminal, EnrollmentError> {
        let statement = TerminalStatement {
            version: WIRE_VERSION,
            group: entry.group,
            title: entry.title.clone(),
            owner: entry.owner.clone(),
            head: signed.head()?,
            revision: signed.manifest.revision,
            kind,
        };
        let signature = entry
            .identity()?
            .sign_enrollment(&encode(&statement)?)
            .map_err(|_| EnrollmentError::Crypto)?;
        let terminal = Terminal {
            statement,
            signature,
        };
        if encode(&terminal)?.len() > TERMINAL_BYTES {
            return Err(EnrollmentError::Capacity);
        }
        Ok(terminal)
    }
    pub(super) fn stage_reserved(
        tx: &prns_groups_mls::ApplicationTransaction<'_, RedbBackend<'_>>,
        snapshot: &Snapshot,
    ) -> Result<(), EnrollmentError> {
        // Historical facts are separate bounded rows. Their full namespace
        // liability is reserved by the ordinary backend before admission;
        // it must not be charged against this single current-header row.
        Self::stage(tx, snapshot)
    }
    fn management_replay(
        adapter: &Adapter<RedbBackend<'_>>,
        snapshot: &Snapshot,
        request: [u8; 32],
        fingerprint: [u8; 32],
    ) -> Result<bool, EnrollmentError> {
        match snapshot.management(adapter, request)? {
            Some(r) if r.fingerprint == fingerprint => Ok(true),
            Some(_) => Err(EnrollmentError::RequestConflict),
            None => Ok(false),
        }
    }
    pub(super) fn remove_member_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        group: GroupId,
        member: Endpoint,
        expected_head: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            ..
        } = admitted;
        if replay.is_some() {
            return Ok(());
        }
        let entry = Self::find(&catalog, group.0)?;
        if entry.owner.rns != self.local.into_bytes() {
            return Err(EnrollmentError::NotOwner);
        }
        self.remove_exact(
            owner,
            &entry,
            member,
            Some(expected_head),
            Some((request, fingerprint)),
        )?;
        // Request, terminal, summary and exact catalog indexes committed in the
        // same namespace transaction; no second receipt write can be lost.
        Ok(())
    }
    fn remove_exact(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        target: Endpoint,
        expected_head: Option<[u8; 32]>,
        request: Option<([u8; 32], [u8; 32])>,
    ) -> Result<Terminal, EnrollmentError> {
        let (prior, member, roster) = self.adapter(owner, entry, None, |_, snapshot| {
            Self::require_active(&snapshot)?;
            let prior = snapshot.current.clone().ok_or(EnrollmentError::NotJoined)?;
            if expected_head.is_some_and(|head| prior.head().ok() != Some(head)) {
                return Err(EnrollmentError::StaleMembership);
            }
            let member = prior
                .manifest
                .members
                .iter()
                .find(|m| m.endpoint().ok() == Some(target))
                .cloned()
                .ok_or(EnrollmentError::MemberNotFound)?;
            if member == entry.owner {
                return Err(EnrollmentError::OwnerMustClose);
            }
            let roster = prior
                .manifest
                .members
                .iter()
                .filter(|m| *m != &member)
                .cloned()
                .collect::<Vec<_>>();
            Ok((prior, member, roster))
        })?;
        let expected = roster.iter().map(Member::binding).collect::<Vec<_>>();
        self.management_adapter(
            owner,
            entry,
            Some(&prior.manifest.members),
            |adapter, mut snapshot| {
                let mut result = None;
                adapter
                    .compose_with_roster(
                        Self::next_maintenance(adapter, entry)?,
                        Command::RemoveMember {
                            group: GroupId(entry.group),
                            member: member.binding(),
                        },
                        Some(&expected),
                        |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                        |tx, output| {
                            if output.event != prns_groups_mls::Event::Control
                                || output.messages.len() != 1
                            {
                                return Err(EnrollmentError::Corrupt);
                            }
                            let commit = output.messages[0].clone();
                            let manifest = Manifest {
                                version: WIRE_VERSION,
                                group: entry.group,
                                owner: entry.owner.rns,
                                title: entry.title.clone(),
                                profile: entry.profile,
                                parent: prior.head()?,
                                revision: prior
                                    .manifest
                                    .revision
                                    .checked_add(1)
                                    .ok_or(EnrollmentError::Capacity)?,
                                epoch: output.context.ok_or(EnrollmentError::Corrupt)?.epoch,
                                commit_digest: hash(&commit),
                                kind: ControlKind::Remove(member.clone()),
                                members: roster.clone(),
                            };
                            let signed = Signed {
                                signature: entry
                                    .identity()?
                                    .sign_enrollment(&encode(&manifest)?)
                                    .map_err(|_| EnrollmentError::Crypto)?,
                                manifest,
                            };
                            Self::transition(entry, &prior, &signed, &commit)?;
                            let terminal = Self::terminal(
                                entry,
                                &signed,
                                TerminalKind::Removed(member.clone()),
                            )?;
                            let control = Control {
                                signed: signed.clone(),
                                commit,
                            };
                            self.frame(&Reply::Control(Some(control.clone())))?;
                            snapshot.current = Some(signed);
                            snapshot.stage_control(tx, &control)?;
                            snapshot.stage_terminal(tx, &terminal)?;
                            if let Some((id, fingerprint)) = request {
                                snapshot.stage_request(
                                    tx,
                                    &Request {
                                        id,
                                        fingerprint,
                                        result: entry.group,
                                    },
                                )?;
                            }
                            Self::stage_reserved(tx, &snapshot)?;
                            result = Some(terminal);
                            Ok(())
                        },
                    )
                    .map_err(composition_error)?;
                result.ok_or(EnrollmentError::Corrupt)
            },
        )
    }

    pub(super) fn leave_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        group: GroupId,
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            ..
        } = admitted;
        if replay.is_some() {
            return self.recover_departure(owner, group.0);
        }
        let entry = Self::find(&catalog, group.0)?;
        if entry.owner.rns == self.local.into_bytes() {
            return Err(EnrollmentError::OwnerMustClose);
        }
        let known_head = self.adapter(owner, &entry, None, |_, snapshot| {
            if matches!(
                snapshot.lifecycle,
                GroupLifecycle::Removed | GroupLifecycle::Closed
            ) {
                return Err(EnrollmentError::NotJoined);
            }
            snapshot
                .current
                .as_ref()
                .ok_or(EnrollmentError::NotJoined)?
                .head()
        })?;
        let current = catalog.current(group.0)?.ok_or(EnrollmentError::Corrupt)?;
        let mut edit = Edit::new(&catalog)?;
        if current.departure.is_none() {
            let departure = Departure {
                id: random()?,
                group: group.0,
                owner: entry.owner.clone(),
                member: entry.local.clone(),
                known_head,
            };
            edit = edit.put_departure(&catalog, &departure)?;
        }
        self.write(Self::remember(&catalog, edit, request, fingerprint, group.0)?.finish()?)?;
        self.recover_departure(owner, group.0)
    }

    pub(super) fn recover_departure(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let Some(current) = catalog.current(group)? else {
            return Ok(());
        };
        let Some(id) = current.departure else {
            return Ok(());
        };
        let departure = catalog.departure(id)?.ok_or(EnrollmentError::Corrupt)?;
        let entry = Self::find(&catalog, group)?;
        if entry.local != departure.member {
            return Err(EnrollmentError::Corrupt);
        }
        self.management_adapter(owner, &entry, None, |adapter, mut snapshot| {
            if snapshot.lifecycle != GroupLifecycle::Active {
                return Ok(());
            }
            snapshot.lifecycle = GroupLifecycle::Leaving;
            adapter
                .maintain(Self::next_maintenance(adapter, &entry)?, &[], |tx| {
                    Self::stage(tx, &snapshot)
                })
                .map_err(composition_error)?;
            Ok(())
        })
    }
    pub(super) fn receive_leave(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        departure: Departure,
    ) -> Result<Vec<u8>, EnrollmentError> {
        if departure.member.rns != requester.into_bytes()
            || departure.owner.rns != self.local.into_bytes()
        {
            return Err(EnrollmentError::Authentication);
        }
        let catalog = self.read()?;
        if let Some(proof) =
            self.terminal_for(owner, &catalog, departure.group, &departure.member)?
        {
            return self.frame(&Reply::Terminal(proof));
        }
        let entry = Self::find(&catalog, departure.group)?;
        if entry.owner != departure.owner {
            return Err(EnrollmentError::Authentication);
        }
        self.adapter(owner, &entry, None, |_, snapshot| {
            if !snapshot
                .current
                .as_ref()
                .is_some_and(|s| s.manifest.members.contains(&departure.member))
            {
                return Err(EnrollmentError::Authentication);
            }
            Ok(())
        })?;
        let proof = self.remove_exact(owner, &entry, departure.member.endpoint()?, None, None)?;
        self.frame(&Reply::Terminal(proof))
    }
    pub(super) fn close_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        group: GroupId,
        expected_head: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            ..
        } = admitted;
        if replay.is_some() {
            return Ok(());
        }
        let entry = Self::find(&catalog, group.0)?;
        if entry.owner.rns != self.local.into_bytes() {
            return Err(EnrollmentError::NotOwner);
        }
        self.management_adapter(owner, &entry, None, |adapter, mut snapshot| {
            if Self::management_replay(adapter, &snapshot, request, fingerprint)? {
                return Ok(());
            }
            Self::require_active(&snapshot)?;
            let signed = snapshot
                .current
                .as_ref()
                .ok_or(EnrollmentError::NotJoined)?;
            if signed.head()? != expected_head {
                return Err(EnrollmentError::StaleMembership);
            }
            let proof = Self::terminal(&entry, signed, TerminalKind::Closed)?;
            adapter
                .compose(
                    Self::next_maintenance(adapter, &entry)?,
                    Command::Retire { group },
                    |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                    |tx, _| {
                        snapshot.lifecycle = GroupLifecycle::Closed;
                        snapshot.stage_terminal(tx, &proof)?;
                        snapshot.stage_request(
                            tx,
                            &Request {
                                id: request,
                                fingerprint,
                                result: group.0,
                            },
                        )?;
                        Self::stage(tx, &snapshot)
                    },
                )
                .map_err(composition_error)?;
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn terminal_for(
        &self,
        _owner: &mut RedbStoreOwner<'db>,
        catalog: &Catalog,
        group: [u8; 32],
        member: &Member,
    ) -> Result<Option<Terminal>, EnrollmentError> {
        if let Some(closed) = catalog.terminal_subject(group, &TerminalKind::Closed)? {
            if closed.proof.statement.owner.rns == self.local.into_bytes()
                && catalog.audience(group, member.rns)?.is_some()
            {
                return Ok(Some(closed.proof));
            }
        }
        Ok(catalog
            .terminal_subject(group, &TerminalKind::Removed(member.clone()))?
            .map(|row| row.proof))
    }

    pub(super) fn acknowledge_notice(
        &self,
        _owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        self.write(
            Edit::new(&catalog)?
                .acknowledge_notice(&catalog, id)?
                .finish()?,
        )
    }
    pub(super) fn notice_id(notice: &Notification) -> Result<[u8; 32], EnrollmentError> {
        Ok(hash(&encode(&(
            "terminal-notice",
            notice.proof.id()?,
            notice.recipient,
        ))?))
    }
    pub(super) fn accept_terminal(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        proof: Terminal,
        departure_ack: Option<[u8; 32]>,
    ) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let group = proof.statement.group;
        if requester.into_bytes() != proof.statement.owner.rns {
            return Err(EnrollmentError::Authentication);
        }
        let current = catalog.current(group)?;
        let retained = current
            .as_ref()
            .map(|current| {
                catalog.incarnation(indexed::keys::incarnation(group, current.generation))
            })
            .transpose()?
            .flatten();
        let (known, title) = if let Some(row) = &retained {
            (
                row.owner().clone(),
                current
                    .as_ref()
                    .ok_or(EnrollmentError::Corrupt)?
                    .title
                    .clone(),
            )
        } else if let Some(row) = catalog.terminal_subject(group, &TerminalKind::Closed)? {
            (row.proof.statement.owner, row.proof.statement.title)
        } else {
            let offer = catalog
                .group_offer(group)?
                .ok_or(EnrollmentError::Missing)?
                .offer;
            (offer.owner, offer.title)
        };
        proof.verify(group, &known)?;
        if proof.statement.title != title {
            return Err(EnrollmentError::Authentication);
        }
        if let Some(id) = departure_ack {
            let departure = catalog.departure(id)?.ok_or(EnrollmentError::Missing)?;
            if departure.group != group
                || departure.owner != known
                || matches!(&proof.statement.kind, TerminalKind::Removed(member) if member != &departure.member)
            {
                return Err(EnrollmentError::Authentication);
            }
        }
        let entry = catalog.entry(group)?;
        if let Some(entry) = entry.as_ref().filter(|entry|
            matches!(proof.statement.kind, TerminalKind::Closed)
                || matches!(&proof.statement.kind, TerminalKind::Removed(member) if member == &entry.local)) {
            let scoped = self.with_departure_ack(departure_ack);
            let committed = scoped.management_adapter(owner, entry, None, |adapter, mut snapshot| {
                if snapshot.terminal(adapter, proof.id()?)?.is_some() { return Ok(false); }
                if snapshot.lifecycle == GroupLifecycle::Closed { return Ok(false); }
                if snapshot.current.as_ref().is_some_and(|signed| signed.manifest.revision > proof.statement.revision) {
                    return Err(EnrollmentError::StaleMembership);
                }
                snapshot.lifecycle = match proof.statement.kind {
                    TerminalKind::Closed => GroupLifecycle::Closed, TerminalKind::Removed(_) => GroupLifecycle::Removed,
                };
                if snapshot.current.is_none() || adapter.is_retired(GroupId(group)).map_err(|_| EnrollmentError::Crypto)? {
                    adapter.maintain(Self::next_maintenance(adapter, entry)?, &[], |tx| {
                        snapshot.stage_terminal(tx, &proof)?;
                        Self::stage(tx, &snapshot)
                    }).map_err(composition_error)?;
                } else {
                    adapter.compose(Self::next_maintenance(adapter, entry)?, Command::Retire { group: GroupId(group) },
                        |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                        |tx, _| { snapshot.stage_terminal(tx, &proof)?; Self::stage(tx, &snapshot) },
                    ).map_err(composition_error)?;
                }
                // The existing participant publishes terminal and lifecycle in
                // the same physical commit as the namespace retirement.
                Ok(true)
            })?;
            if committed { return Ok(()); }
        }
        let row = catalog
            .terminal(proof.id()?)?
            .unwrap_or(indexed::rows::TerminalRow {
                proof: proof.clone(),
                after: None,
                complete: true,
            });
        let mut edit = Edit::new(&catalog)?.put_terminal(&catalog, &row)?;
        if let Some(id) = departure_ack {
            edit = edit.clear_departure_id(&catalog, id)?;
        }
        if let Some(current) = catalog.current(group)? {
            if let Some(id) = current.departure {
                let departure = catalog.departure(id)?.ok_or(EnrollmentError::Corrupt)?;
                if matches!(proof.statement.kind, TerminalKind::Closed)
                    || matches!(&proof.statement.kind, TerminalKind::Removed(member) if member == &departure.member)
                {
                    edit = edit.clear_departure_id(&catalog, id)?;
                }
            }
        }
        self.write(edit.finish()?)
    }

    pub(super) fn delete_local_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        group: GroupId,
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            ..
        } = admitted;
        if replay.is_some() {
            return self.purge_retired(owner);
        }
        let entry = Self::find(&catalog, group.0)?;
        let current = catalog.current(group.0)?.ok_or(EnrollmentError::Corrupt)?;
        if entry.owner.rns == self.local.into_bytes() && current.lifecycle != GroupLifecycle::Closed
        {
            return Err(EnrollmentError::OwnerMustClose);
        }
        // One fence covers every older generation before physical cleanup.
        // Keyed terminal, audience and departure obligations remain retained.
        let edit = Edit::new(&catalog)?
            .fence_delete(&catalog, group.0)?
            .put_incarnation(
                &catalog,
                indexed::keys::incarnation(group.0, entry.generation),
                &indexed::rows::Incarnation::Retired(
                    indexed::rows::RetiredIncarnation::from_entry(&entry, false),
                ),
            )?;
        self.write(Self::remember(&catalog, edit, request, fingerprint, group.0)?.finish()?)?;
        self.purge_retired(owner)
    }

    pub(super) fn purge_retired(
        &self,
        owner: &mut RedbStoreOwner<'db>,
    ) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let plan = catalog.purge_plan()?;
        let Some((key, mut retired)) = plan.target else {
            if catalog.header.purge_after != plan.after {
                self.write(Edit::new(&catalog)?.purge_progress(plan.after).finish()?)?;
            }
            return Ok(());
        };
        // Persist the nonsecret exact retirement row before deleting anything.
        // A lost completion leaves that same fenced namespace retryable.
        self.write(
            Edit::new(&catalog)?
                .put_incarnation(
                    &catalog,
                    key,
                    &indexed::rows::Incarnation::Retired(retired.clone()),
                )?
                .finish()?,
        )?;
        crate::archive::purge_retired(
            self.database,
            owner,
            self.local,
            GroupId(retired.group),
            prns_groups_store_redb::CommitContext {
                namespace: StoreNamespace(retired.namespace),
                incarnation: StoreIncarnation::new(retired.incarnation)
                    .map_err(|_| EnrollmentError::Corrupt)?,
            },
            self.selection.namespace,
        )?;
        crate::session::delete_cursors(
            self.database,
            retired.namespace,
            self.change_witness.as_ref(),
            self.change_stamp,
        )
        .map_err(|_| EnrollmentError::Corrupt)?;
        retired.purged = true;
        let refreshed = self.read()?;
        self.write(
            Edit::new(&refreshed)?
                .put_incarnation(
                    &refreshed,
                    key,
                    &indexed::rows::Incarnation::Retired(retired),
                )?
                .purge_progress(plan.after)
                .finish()?,
        )
    }
}
