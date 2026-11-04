use super::*;
impl<'db> Enrollment<'db> {
    pub fn handle(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        bytes: &[u8],
        now: u64,
    ) -> Result<Vec<u8>, EnrollmentError> {
        let request = self.prepare_handle(bytes)?;
        self.handle_prepared(owner, requester, request, now)
    }
    pub(super) fn prepare_handle(&self, bytes: &[u8]) -> Result<Frame, EnrollmentError> {
        let request = self.decode(bytes)?;
        self.check_frame_profile(&request, bytes.len())?;
        Ok(request)
    }
    fn check_frame_profile(&self, request: &Frame, bytes: usize) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let selected = match request {
            Frame::Offer(offer) => offer.profile.validate().ok().map(|()| offer.profile),
            Frame::Join { invitation, .. }
            | Frame::Joined { invitation, .. }
            | Frame::Decline { invitation } => {
                if let Some(invite) = catalog.invitation(*invitation)? {
                    Some(invite.offer.profile)
                } else if let Some(binding) = catalog.invitation_binding(*invitation)? {
                    Some(
                        catalog
                            .incarnation(indexed::keys::incarnation(
                                binding.namespace.group,
                                binding.namespace.generation,
                            ))?
                            .ok_or(EnrollmentError::Corrupt)?
                            .profile(),
                    )
                } else {
                    None
                }
            }
            Frame::Leave(departure) => catalog.group_profile(departure.group)?,
            Frame::Terminal(proof) => catalog.group_profile(proof.statement.group)?,
            Frame::Cancel { group, .. } | Frame::Sync { group, .. } => {
                catalog.group_profile(*group)?
            }
        };
        if selected.is_some_and(|profile| {
            u64::try_from(bytes).map_or(true, |bytes| {
                bytes > u64::from(profile.max_enrollment_frame_bytes)
            })
        }) {
            return Err(EnrollmentError::Capacity);
        }
        Ok(())
    }
    pub(super) fn handle_prepared(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        request: Frame,
        now: u64,
    ) -> Result<Vec<u8>, EnrollmentError> {
        match request {
            Frame::Offer(offer) => self.receive_offer(owner, requester, offer, now),
            Frame::Leave(departure) => self.receive_leave(owner, requester, departure),
            Frame::Terminal(proof) => {
                self.accept_terminal(owner, requester, proof, None)?;
                self.frame(&Reply::Ack)
            }
            Frame::Cancel {
                invitation,
                group,
                expires,
            } => {
                let catalog = self.read()?;
                let now = now.max(catalog.header.invitation_time_floor);
                if expires < now {
                    return self.frame(&Reply::Ack);
                }
                if expires > now.saturating_add(INVITATION_LIFETIME_SECS) {
                    return Err(EnrollmentError::Invalid);
                }
                let mut edit = Edit::new(&catalog)?.put_cancellation(
                    &catalog,
                    &Cancellation {
                        invitation,
                        group,
                        owner: requester.into_bytes(),
                        expires,
                    },
                )?;
                if let Some(mut invite) = catalog.invitation(invitation)? {
                    if invite.offer.owner.rns != requester.into_bytes()
                        || invite.offer.recipient != self.local.into_bytes()
                    {
                        return Err(EnrollmentError::Authentication);
                    }
                    invite.state = InvitationState::Cancelled;
                    invite.acknowledged = true;
                    invite.deferred_join = None;
                    edit = edit.put_invitation(&catalog, &invite)?;
                }
                self.write(edit.finish()?)?;
                self.frame(&Reply::Ack)
            }
            request @ Frame::Join { .. } => self.receive_join(owner, requester, request, now),
            Frame::Joined { invitation, head } => {
                let catalog = self.read()?;
                let (entry, _, added) = self
                    .retained_add(owner, &catalog, invitation, requester)?
                    .ok_or(EnrollmentError::Missing)?;
                self.adapter(owner, &entry, None, |adapter, snapshot| {
                    let signed = if snapshot
                        .current
                        .as_ref()
                        .is_some_and(|s| s.head().ok() == Some(head))
                    {
                        snapshot.current.clone().ok_or(EnrollmentError::Corrupt)?
                    } else {
                        snapshot
                            .control_head(adapter, head)?
                            .ok_or(EnrollmentError::Authentication)?
                            .signed
                    };
                    if signed.manifest.epoch < added.signed.manifest.epoch {
                        return Err(EnrollmentError::Authentication);
                    }
                    Ok(())
                })?;
                if let Some(mut invite) = catalog.invitation(invitation)? {
                    invite.state = InvitationState::Joined;
                    invite.acknowledged = true;
                    invite
                        .terminal_at
                        .get_or_insert(now.max(catalog.header.invitation_time_floor));
                    self.write(
                        Edit::new(&catalog)?
                            .put_invitation(&catalog, &invite)?
                            .finish()?,
                    )?;
                }
                self.frame(&Reply::Ack)
            }
            Frame::Decline { invitation } => {
                let catalog = self.read()?;
                let Some(mut invite) = catalog.invitation(invitation)? else {
                    return self.frame(&Reply::Ack);
                };
                if invite.offer.owner.rns != self.local.into_bytes()
                    || invite.offer.recipient != requester.into_bytes()
                {
                    return Err(EnrollmentError::Authentication);
                }
                if catalog.invitation_binding(invitation)?.is_some() {
                    return Err(EnrollmentError::Conflict);
                }
                invite.state = InvitationState::Declined;
                invite.acknowledged = true;
                self.write(
                    Edit::new(&catalog)?
                        .put_invitation(&catalog, &invite)?
                        .finish()?,
                )?;
                self.frame(&Reply::Ack)
            }
            Frame::Sync {
                group,
                head,
                member,
            } => {
                let catalog = self.read()?;
                if member.rns != requester.into_bytes() {
                    return Err(EnrollmentError::Authentication);
                }
                if let Some(proof) = self.terminal_for(owner, &catalog, group, &member)? {
                    return self.frame(&Reply::Terminal(proof));
                }
                let entry = Self::find(&catalog, group)?;
                self.adapter(owner, &entry, None, |adapter, snapshot| {
                    let current = snapshot
                        .current
                        .as_ref()
                        .ok_or(EnrollmentError::NotJoined)?;
                    if !current.manifest.members.iter().any(|m| m == &member) {
                        return Err(EnrollmentError::Authentication);
                    }
                    if head == current.head()? {
                        return self.frame(&Reply::Control(None));
                    }
                    let next = snapshot
                        .control_after(adapter, head)?
                        .ok_or(EnrollmentError::Conflict)?;
                    self.frame(&Reply::Control(Some(next.clone())))
                })
            }
        }
    }
    fn receive_offer(
        &self,
        _owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        offer: Offer,
        now: u64,
    ) -> Result<Vec<u8>, EnrollmentError> {
        if requester.into_bytes() != offer.owner.rns
            || offer.recipient != self.local.into_bytes()
            || requester == self.local
            || offer.owner.incarnation == 0
            || offer.title.trim().is_empty()
            || offer.title.len() > profile::MAX_TITLE_BYTES
        {
            return Err(EnrollmentError::Authentication);
        }
        match self.require_profile(offer.profile) {
            Ok(()) => (),
            Err(EnrollmentError::UnsupportedProfile) => {
                return self.frame(&Reply::UnsupportedProfile)
            }
            Err(error) => return Err(error),
        }
        let catalog = self.read()?;
        if catalog
            .terminal_subject(offer.group, &TerminalKind::Closed)?
            .is_some()
        {
            return Err(EnrollmentError::GroupClosed);
        }
        if let Some(cancel) = catalog.cancellation(offer.id)? {
            if cancel.group != offer.group
                || cancel.owner != offer.owner.rns
                || cancel.expires != offer.expires
            {
                return Err(EnrollmentError::Authentication);
            }
            return self.frame(&Reply::Ack);
        }
        if let Some(prior) = catalog.invitation(offer.id)? {
            return if prior.offer == offer {
                self.frame(&Reply::Ack)
            } else {
                Err(EnrollmentError::Conflict)
            };
        }
        if catalog.invitation_binding(offer.id)?.is_some() {
            return Err(EnrollmentError::Conflict);
        }
        require_time(&offer, now.max(catalog.header.invitation_time_floor))?;
        if let Some(current) = catalog.current(offer.group)? {
            let prior = catalog
                .incarnation(indexed::keys::incarnation(offer.group, current.generation))?
                .ok_or(EnrollmentError::Corrupt)?;
            if prior.owner() != &offer.owner
                || prior.profile() != offer.profile
                || current.title != offer.title
            {
                return Err(EnrollmentError::Authentication);
            }
            if let indexed::rows::Incarnation::Live(entry) = prior {
                let terminal_unjoined = current.head.is_none()
                    && entry
                        .invitation
                        .map(|id| catalog.invitation(id))
                        .transpose()?
                        .flatten()
                        .is_some_and(|invite| {
                            matches!(
                                invite.state,
                                InvitationState::Cancelled | InvitationState::Expired
                            ) || now.max(catalog.header.invitation_time_floor)
                                >= invite.offer.expires
                        });
                if current.lifecycle != GroupLifecycle::Removed && !terminal_unjoined {
                    return Err(EnrollmentError::Conflict);
                }
            }
        } else if let Some(prior) = catalog.group_offer(offer.group)? {
            if prior.offer.owner != offer.owner
                || prior.offer.profile != offer.profile
                || prior.offer.title != offer.title
            {
                return Err(EnrollmentError::Authentication);
            }
        }
        let invite = Invite {
            admission_revision: 0,
            offer,
            state: InvitationState::Pending,
            acknowledged: true,
            terminal_at: None,
            deferred_join: None,
            dismissed: false,
        };
        self.write(
            Edit::new(&catalog)?
                .put_invitation(&catalog, &invite)?
                .finish()?,
        )?;
        self.frame(&Reply::Ack)
    }
    fn receive_join(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        requester: IdentityHash,
        request: Frame,
        now: u64,
    ) -> Result<Vec<u8>, EnrollmentError> {
        let Frame::Join {
            invitation,
            profile: offered_profile,
            member,
            package,
        } = request
        else {
            return Err(EnrollmentError::Invalid);
        };
        if member.rns != requester.into_bytes() || member.incarnation == 0 {
            return Err(EnrollmentError::Authentication);
        }
        let catalog = self.read()?;
        let invitation_group = self.invitation_group(invitation)?.map(|group| group.0);
        if let Some(group) = invitation_group {
            if let Some(proof) = self.terminal_for(owner, &catalog, group, &member)? {
                return self.frame(&Reply::Terminal(proof));
            }
        }

        let request_hash = hash(&encode(&Frame::Join {
            invitation,
            profile: offered_profile,
            member: member.clone(),
            package: package.clone(),
        })?);
        if let Some((_, snapshot, added)) =
            self.retained_add(owner, &catalog, invitation, requester)?
        {
            if added.request_hash != request_hash {
                return Err(EnrollmentError::Conflict);
            }
            return self.frame(&Reply::Welcome {
                added,
                tip: Box::new(snapshot.current.clone().ok_or(EnrollmentError::Corrupt)?),
            });
        }
        let Some(invite) = catalog.invitation(invitation)? else {
            return self.frame(&Reply::Expired);
        };
        if invite.offer.owner.rns != self.local.into_bytes()
            || invite.offer.recipient != requester.into_bytes()
            || offered_profile != profile_digest(invite.offer.profile)?
            || package.len()
                > usize::try_from(invite.offer.profile.max_wire_bytes)
                    .map_err(|_| EnrollmentError::Corrupt)?
        {
            return Err(EnrollmentError::Authentication);
        }
        let entry = Self::find(&catalog, invite.offer.group)?;
        let (prior, mut roster) = self.adapter(owner, &entry, None, |_, snapshot| {
            Self::require_active(&snapshot)?;
            let prior = snapshot.current.clone().ok_or(EnrollmentError::NotJoined)?;
            Ok((prior.clone(), prior.manifest.members.clone()))
        })?;
        let now = now.max(catalog.header.invitation_time_floor);
        if now >= invite.offer.expires {
            return self.frame(&Reply::Expired);
        }
        require_time(&invite.offer, now)?;
        if invite.state == InvitationState::Cancelled {
            return self.frame(&Reply::Cancelled);
        }
        if invite.state != InvitationState::Pending {
            return Err(EnrollmentError::Conflict);
        }
        if roster.len()
            >= usize::try_from(entry.profile.max_members).map_err(|_| EnrollmentError::Corrupt)?
        {
            return Err(EnrollmentError::Capacity);
        }
        if roster
            .iter()
            .any(|m| m.endpoint == member.endpoint || m.rns == member.rns || m.key == member.key)
        {
            return Err(EnrollmentError::Conflict);
        }
        if invite
            .deferred_join
            .is_some_and(|prior| prior != request_hash)
        {
            return Err(EnrollmentError::Conflict);
        }
        // This lane exclusively owns both service publications/receipts and Add.
        // Check at the actual transition, including sends admitted after Offer.
        // Never weaken the old-era fence to make a delayed join succeed.
        let waiting = self.owned_adapter(owner, &entry, None, |adapter, snapshot| {
            if snapshot.current.as_ref() != Some(&prior) {
                return Err(EnrollmentError::Conflict);
            }
            if !adapter
                .key_package_matches(&package, &member.binding())
                .map_err(|_| EnrollmentError::Authentication)?
            {
                return Err(EnrollmentError::Authentication);
            }
            let readers = prior
                .manifest
                .members
                .iter()
                .map(Member::endpoint)
                .collect::<Result<Vec<_>, _>>()?;
            let head = prns_groups_core::ControlId(prior.head()?);
            let service = prns_groups_service::GroupService::new_managed(
                adapter,
                prns_groups_service::Descriptor {
                    group: GroupId(entry.group),
                    control: prns_groups_service::ControlReference {
                        head,
                        era: prns_groups_core::AuthorizationEra(head),
                    },
                    readers: &readers,
                },
                entry
                    .profile
                    .service_limits()
                    .map_err(|_| EnrollmentError::Corrupt)?,
            )
            .map_err(|_| EnrollmentError::Corrupt)?;
            let clock = prns_groups_core::ClockReading::Trusted(
                prns_groups_core::ClockInterval::new(Timestamp(now), Timestamp(now))
                    .map_err(|_| EnrollmentError::Invalid)?,
            );
            service
                .has_pending_authored_deliveries(clock)
                .map_err(|_| EnrollmentError::Corrupt)
        })?;
        if waiting {
            self.defer_join(invitation, request_hash)?;
            return self.frame(&Reply::AwaitingDelivery);
        }
        roster.push(member);
        roster.sort_by_key(|m| m.endpoint);
        let expected = roster.iter().map(Member::binding).collect::<Vec<_>>();
        let added = self.adapter(owner, &entry, Some(&roster), |adapter, mut snapshot| {
            if snapshot.current.as_ref() != Some(&prior) {
                return Err(EnrollmentError::Conflict);
            }
            let candidate = roster
                .iter()
                .find(|m| m.rns == requester.into_bytes())
                .ok_or(EnrollmentError::Authentication)?;
            if !adapter
                .key_package_matches(&package, &candidate.binding())
                .map_err(|_| EnrollmentError::Authentication)?
            {
                return Err(EnrollmentError::Authentication);
            }
            let identity = entry.identity()?;
            let mut added = None;
            adapter
                .compose_with_roster(
                    self.next_protocol(adapter, &entry)?,
                    Command::Add {
                        group: GroupId(entry.group),
                        packages: &[&package],
                        now_secs: now,
                    },
                    Some(&expected),
                    |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                    |tx, output| {
                        if output.messages.len() != 2 {
                            return Err(EnrollmentError::Corrupt);
                        }
                        let commit = output.messages[0].clone();
                        let welcome = output.messages[1].clone();
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
                            kind: ControlKind::Add,
                            members: roster.clone(),
                        };
                        let signed = Signed {
                            signature: identity
                                .sign_enrollment(&encode(&manifest)?)
                                .map_err(|_| EnrollmentError::Crypto)?,
                            manifest,
                        };
                        Self::transition(&entry, &prior, &signed, &commit)?;
                        let result = Added {
                            member: candidate.clone(),
                            invitation,
                            request_hash,
                            signed: signed.clone(),
                            welcome,
                        };
                        self.frame(&Reply::Welcome {
                            tip: Box::new(signed.clone()),
                            added: result.clone(),
                        })?;
                        // Profile admission bounds every Welcome with any two
                        // allowed rosters; older exact Welcome rows need no rescan.
                        let control = Control {
                            signed: signed.clone(),
                            commit,
                        };
                        self.frame(&Reply::Control(Some(control.clone())))?;
                        snapshot.current = Some(signed);
                        snapshot.stage_control(tx, &control)?;
                        snapshot.stage_added(tx, &result)?;
                        Self::stage_reserved(tx, &snapshot)?;
                        added = Some(result);
                        Ok(())
                    },
                )
                .map_err(composition_error)?;
            added.ok_or(EnrollmentError::Corrupt)
        })?;
        self.frame(&Reply::Welcome {
            tip: Box::new(added.signed.clone()),
            added,
        })
    }
    /// Complete only a response obtained from the Work's authenticated recipient.
    /// A dropped response can always be retried against the exact original work.
    pub fn complete(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        response: &[u8],
        now: u64,
    ) -> Result<CompletionActivity, EnrollmentError> {
        let Some((work, reply)) = self.prepare_complete(owner, id, response, now)? else {
            return Ok(CompletionActivity::Idle);
        };
        self.complete_prepared(owner, work, reply, now)
    }
    pub(super) fn prepare_complete(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        response: &[u8],
        now: u64,
    ) -> Result<Option<(Work, Reply)>, EnrollmentError> {
        let reply: Reply = self.decode(response)?;
        // Correlate this exact response without advancing unrelated maintenance
        // cursors. The runner and explicit pending-work traversal own cleanup;
        // an idle or duplicate response must not write the catalog by itself.
        let pending = self.pending_work(owner, &self.read()?, id, now)?;
        if let Some(work) = &pending {
            self.check_frame_profile(&self.decode::<Frame>(&work.frame)?, response.len())?;
        }
        Ok(pending.map(|work| (work, reply)))
    }
    pub(super) fn complete_prepared(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        work: Work,
        reply: Reply,
        now: u64,
    ) -> Result<CompletionActivity, EnrollmentError> {
        match (self.decode::<Frame>(&work.frame)?, reply) {
            (
                request @ (Frame::Leave(_) | Frame::Sync { .. } | Frame::Join { .. }),
                Reply::Terminal(proof),
            ) => {
                let (group, member) = match request {
                    Frame::Leave(departure) => (departure.group, departure.member),
                    Frame::Sync { group, member, .. } => (group, member),
                    Frame::Join {
                        invitation, member, ..
                    } => {
                        let catalog = self.read()?;
                        let group = catalog
                            .invitation(invitation)?
                            .ok_or(EnrollmentError::Missing)?
                            .offer
                            .group;
                        (group, member)
                    }
                    _ => return Err(EnrollmentError::Invalid),
                };
                if proof.statement.group != group
                    || matches!(&proof.statement.kind, TerminalKind::Removed(target) if target != &member)
                {
                    return Err(EnrollmentError::Authentication);
                }
                let catalog = self.read()?;
                let departure = match catalog.work_source(work.id)? {
                    Some(indexed::work::WorkSource::Departure(id)) => Some(id),
                    _ => None,
                };
                // The exact old outbox and terminal share the one existing
                // carrier transaction, even after a later local rejoin.
                self.accept_terminal(owner, work.recipient, proof, departure)?;
                Ok(CompletionActivity::Progress)
            }
            (Frame::Terminal(_), Reply::Ack) => {
                self.acknowledge_notice(owner, work.id)?;
                Ok(CompletionActivity::Progress)
            }
            (Frame::Cancel { invitation, .. }, Reply::Ack) => self
                .acknowledge(invitation)
                .map(|()| CompletionActivity::Progress),
            (Frame::Offer(offer), Reply::Ack) => self
                .acknowledge(offer.id)
                .map(|()| CompletionActivity::Progress),
            (Frame::Offer(offer), Reply::UnsupportedProfile) => {
                let catalog = self.read()?;
                let mut invite = catalog
                    .invitation(offer.id)?
                    .ok_or(EnrollmentError::Missing)?;
                if invite.offer != offer
                    || invite.offer.recipient != work.recipient.into_bytes()
                    || invite.state != InvitationState::Pending
                {
                    return Err(EnrollmentError::Conflict);
                }
                invite.state = InvitationState::UnsupportedProfile;
                invite.acknowledged = true;
                invite
                    .terminal_at
                    .get_or_insert(now.max(catalog.header.invitation_time_floor));
                self.write(
                    Edit::new(&catalog)?
                        .put_invitation(&catalog, &invite)?
                        .finish()?,
                )
                .map(|()| CompletionActivity::Progress)
            }
            (Frame::Decline { invitation }, Reply::Ack) => self
                .acknowledge(invitation)
                .map(|()| CompletionActivity::Progress),
            (Frame::Joined { invitation, .. }, Reply::Ack) => {
                let catalog = self.read()?;
                let mut invite = catalog
                    .invitation(invitation)?
                    .ok_or(EnrollmentError::Missing)?;
                invite.state = InvitationState::Joined;
                invite.acknowledged = true;
                invite
                    .terminal_at
                    .get_or_insert(now.max(catalog.header.invitation_time_floor));
                self.write(
                    Edit::new(&catalog)?
                        .put_invitation(&catalog, &invite)?
                        .finish()?,
                )
                .map(|()| CompletionActivity::Progress)
            }
            (join @ Frame::Join { .. }, Reply::AwaitingDelivery) => {
                let Frame::Join { invitation, .. } = &join else {
                    return Err(EnrollmentError::Invalid);
                };
                self.defer_join(*invitation, hash(&encode(&join)?))?;
                // A deferred Join keeps the exact durable KeyPackage and normal
                // retry schedule; it is not fresh membership progress.
                Ok(CompletionActivity::Idle)
            }
            (
                Frame::Join {
                    invitation,
                    profile,
                    member,
                    package,
                },
                Reply::Welcome { added, tip },
            ) => {
                if added.invitation != invitation
                    || added.request_hash
                        != hash(&encode(&Frame::Join {
                            invitation,
                            profile,
                            member,
                            package,
                        })?)
                {
                    return Err(EnrollmentError::Authentication);
                }
                let catalog = self.read()?;
                let invite = catalog
                    .invitation(invitation)?
                    .ok_or(EnrollmentError::Missing)?;
                let entry = Self::find(&catalog, invite.offer.group)?;
                added
                    .signed
                    .verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
                tip.verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
                if tip.manifest.epoch < added.signed.manifest.epoch
                    || !tip.manifest.members.contains(&entry.local)
                    || (tip.manifest.epoch == added.signed.manifest.epoch && *tip != added.signed)
                {
                    return Err(EnrollmentError::Authentication);
                }
                if !added.signed.manifest.members.contains(&entry.local) {
                    return Err(EnrollmentError::Authentication);
                }
                let roster = &added.signed.manifest.members;
                let expected = roster.iter().map(Member::binding).collect::<Vec<_>>();
                self.adapter(owner, &entry, Some(roster), |adapter, mut snapshot| {
                    if snapshot.current.is_some() {
                        return Ok(CompletionActivity::Idle);
                    }
                    adapter
                        .compose_with_roster(
                            self.next_protocol(adapter, &entry)?,
                            Command::Join {
                                group: GroupId(entry.group),
                                welcome: &added.welcome,
                                now_secs: now,
                            },
                            Some(&expected),
                            |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                            |tx, output| {
                                if output.context.ok_or(EnrollmentError::Corrupt)?.epoch
                                    != added.signed.manifest.epoch
                                {
                                    return Err(EnrollmentError::Authentication);
                                }
                                snapshot.required_tip = Some((*tip).clone());
                                snapshot.current = Some(added.signed.clone());
                                snapshot.package = None;
                                Self::stage(tx, &snapshot)
                            },
                        )
                        .map_err(composition_error)?;
                    Ok(CompletionActivity::Progress)
                })
            }
            (Frame::Join { invitation, .. }, terminal @ (Reply::Expired | Reply::Cancelled)) => {
                let catalog = self.read()?;
                let mut invite = catalog
                    .invitation(invitation)?
                    .ok_or(EnrollmentError::Missing)?;
                invite.state = if matches!(terminal, Reply::Cancelled) {
                    InvitationState::Cancelled
                } else {
                    InvitationState::Expired
                };
                invite.acknowledged = true;
                invite
                    .terminal_at
                    .get_or_insert(now.max(catalog.header.invitation_time_floor));
                self.write(
                    Edit::new(&catalog)?
                        .put_invitation(&catalog, &invite)?
                        .finish()?,
                )
                .map(|()| CompletionActivity::Progress)
            }
            (Frame::Sync { group, head, .. }, Reply::Control(control)) => {
                let Some(control) = control else {
                    return Ok(CompletionActivity::Idle);
                };
                let catalog = self.read()?;
                let entry = Self::find(&catalog, group)?;
                let prior = self.adapter(owner, &entry, None, |_, s| {
                    s.current.clone().ok_or(EnrollmentError::NotJoined)
                })?;
                if prior.head()? != head {
                    return Err(EnrollmentError::Conflict);
                }
                Self::transition(&entry, &prior, &control.signed, &control.commit)?;
                let roster = &control.signed.manifest.members;
                let expected = roster.iter().map(Member::binding).collect::<Vec<_>>();
                let mut authorized = prior.manifest.members.clone();
                for member in roster {
                    if !authorized.contains(member) {
                        authorized.push(member.clone());
                    }
                }
                self.management_adapter(
                    owner,
                    &entry,
                    Some(&authorized),
                    |adapter, mut snapshot| {
                        adapter
                            .compose_with_roster(
                                Self::next_maintenance(adapter, &entry)?,
                                Command::Receive {
                                    group: GroupId(group),
                                    bytes: &control.commit,
                                },
                                Some(&expected),
                                |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                                |tx, output| {
                                    if output.event != prns_groups_mls::Event::Control
                                        || output.context.ok_or(EnrollmentError::Corrupt)?.epoch
                                            != control.signed.manifest.epoch
                                    {
                                        return Err(EnrollmentError::Authentication);
                                    }
                                    snapshot.current = Some(control.signed.clone());
                                    snapshot.stage_control(tx, &control)?;
                                    // Crossing the required join checkpoint must match
                                    // the signed tip exactly before publication.
                                    snapshot.ready()?;
                                    Self::stage(tx, &snapshot)
                                },
                            )
                            .map_err(composition_error)?;
                        Ok(CompletionActivity::Progress)
                    },
                )
            }
            _ => Err(EnrollmentError::Invalid),
        }
    }
    fn defer_join(
        &self,
        invitation: [u8; 32],
        request_hash: [u8; 32],
    ) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let mut invite = catalog
            .invitation(invitation)?
            .ok_or(EnrollmentError::Missing)?;
        if let Some(prior) = invite.deferred_join {
            return if prior == request_hash {
                Ok(())
            } else {
                Err(EnrollmentError::Conflict)
            };
        }
        invite.deferred_join = Some(request_hash);
        self.write(
            Edit::new(&catalog)?
                .put_invitation(&catalog, &invite)?
                .finish()?,
        )
    }
    fn acknowledge(&self, id: [u8; 32]) -> Result<(), EnrollmentError> {
        let catalog = self.read()?;
        let mut invite = catalog.invitation(id)?.ok_or(EnrollmentError::Missing)?;
        invite.acknowledged = true;
        self.write(
            Edit::new(&catalog)?
                .put_invitation(&catalog, &invite)?
                .finish()?,
        )
    }
    pub(super) fn transition(
        entry: &Entry,
        prior: &Signed,
        next: &Signed,
        commit: &[u8],
    ) -> Result<(), EnrollmentError> {
        Self::transition_context(&state::InspectionContext::from(entry), prior, next, commit)
    }

    pub(super) fn transition_context(
        entry: &state::InspectionContext<'_>,
        prior: &Signed,
        next: &Signed,
        commit: &[u8],
    ) -> Result<(), EnrollmentError> {
        next.verify(entry.group, entry.owner, entry.title, entry.profile)?;
        if next.manifest.revision
            != prior
                .manifest
                .revision
                .checked_add(1)
                .ok_or(EnrollmentError::Capacity)?
            || next.manifest.commit_digest != hash(commit)
        {
            return Err(EnrollmentError::Authentication);
        }
        let old = prior
            .manifest
            .members
            .iter()
            .map(Member::endpoint)
            .collect::<Result<Vec<_>, _>>()?;
        let new = next
            .manifest
            .members
            .iter()
            .map(Member::endpoint)
            .collect::<Result<Vec<_>, _>>()?;
        match &next.manifest.kind {
            ControlKind::Add => {
                if prior
                    .manifest
                    .members
                    .iter()
                    .any(|m| !next.manifest.members.contains(m))
                {
                    return Err(EnrollmentError::Authentication);
                }
                prns_groups_core::enrollment::validate_add_transition(
                    GroupId(entry.group),
                    prior.head()?,
                    prior.manifest.epoch,
                    &old,
                    GroupId(next.manifest.group),
                    next.manifest.parent,
                    next.manifest.epoch,
                    &new,
                    usize::try_from(entry.profile.max_members)
                        .map_err(|_| EnrollmentError::Corrupt)?,
                )
                .map_err(|_| EnrollmentError::Authentication)?;
            }
            ControlKind::Remove(target) => {
                let removed = prns_groups_core::enrollment::validate_remove_transition(
                    GroupId(entry.group),
                    prior.head()?,
                    prior.manifest.epoch,
                    &old,
                    GroupId(next.manifest.group),
                    next.manifest.parent,
                    next.manifest.epoch,
                    &new,
                    entry.owner.endpoint()?,
                    usize::try_from(entry.profile.max_members)
                        .map_err(|_| EnrollmentError::Corrupt)?,
                )
                .map_err(|_| EnrollmentError::Authentication)?;
                if removed != target.endpoint()?
                    || !prior.manifest.members.contains(target)
                    || next
                        .manifest
                        .members
                        .iter()
                        .any(|m| !prior.manifest.members.contains(m))
                {
                    return Err(EnrollmentError::Authentication);
                }
            }
            ControlKind::Create => return Err(EnrollmentError::Authentication),
        }
        Ok(())
    }
}
