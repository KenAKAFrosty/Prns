use super::*;

impl<B: AtomicBackend + Send> Adapter<B> {
    pub fn create(
        &self,
        operation: OperationId,
        group: GroupId,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| self.prepare_create(provider, group))
    }

    pub(super) fn prepare_create(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<Output, Error<B::Error>> {
        self.owner()?;
        self.reserve_group(provider, group)?;
        let client = self.client(provider.clone());
        let mut state = client
            .group_builder()?
            .with_group_id(group.0.to_vec())
            .build()?;
        self.validate_group(&state)?;
        state.write_to_storage()?;
        provider.put(&key(b'B', &group.0), &state.current_epoch().to_be_bytes())?;
        Ok(Self::context(
            Output::empty(Event::Created),
            group,
            state.current_epoch(),
        ))
    }
    pub fn key_package(
        &self,
        operation: OperationId,
        now_secs: u64,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_key_package(provider, now_secs)
        })
    }

    pub(super) fn prepare_key_package(
        &self,
        provider: &Providers<B>,
        now_secs: u64,
    ) -> Result<Output, Error<B::Error>> {
        if now_secs < 3600 {
            return Err(Error::TimeUncertain);
        }
        if provider
            .index(b"key-packages", 32, self.limits.max_key_packages)?
            .len()
            / 32
            >= self.limits.max_key_packages
        {
            return Err(Limit::KeyPackages.into());
        }
        let client = self.client(provider.clone());
        let message = client.generate_key_package_message(
            Default::default(),
            Default::default(),
            Some(now_secs.into()),
        )?;
        Ok(Output::one(Event::KeyPackage, self.encode(message)?))
    }
    pub fn join(
        &self,
        operation: OperationId,
        group: GroupId,
        welcome: &[u8],
        now_secs: u64,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_join(provider, group, welcome, now_secs)
        })
    }

    pub(super) fn prepare_join(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        welcome: &[u8],
        now_secs: u64,
    ) -> Result<Output, Error<B::Error>> {
        self.reserve_group(provider, group)?;
        let message = self.decode(welcome)?;
        if message.wire_format() != WireFormat::Welcome {
            return Err(Error::UnexpectedMessage);
        }
        let client = self.client(provider.clone());
        let (mut state, info) = client.join_group(None, &message, Some(now_secs.into()))?;
        let sender = state.roster().member_with_index(info.sender)?;
        if !self.policy.owner.matches(&sender.signing_identity) {
            return Err(PolicyError::Owner.into());
        }
        if state.group_id() != group.0 {
            return Err(Error::UnexpectedMessage);
        }
        self.validate_group(&state)?;
        state.write_to_storage()?;
        provider.put(&key(b'B', &group.0), &state.current_epoch().to_be_bytes())?;
        Ok(Self::context(
            Output::empty(Event::Joined),
            group,
            state.current_epoch(),
        ))
    }
    pub fn add(
        &self,
        operation: OperationId,
        group: GroupId,
        packages: &[&[u8]],
        now_secs: u64,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_add(provider, group, packages, now_secs)
        })
    }

    pub(super) fn prepare_add(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        packages: &[&[u8]],
        now_secs: u64,
    ) -> Result<Output, Error<B::Error>> {
        self.owner()?;
        let mut state = self.load(provider, group)?;
        if packages.is_empty()
            || packages.len() > self.limits.max_pending_proposals
            || state
                .roster()
                .members_iter()
                .count()
                .checked_add(packages.len())
                .is_none_or(|n| n > self.limits.max_members)
        {
            return Err(Limit::Members.into());
        }
        let mut builder = state.commit_builder().commit_time(now_secs.into());
        for bytes in packages {
            builder = builder.add_member(self.decode(bytes)?)?;
        }
        let output = builder.build()?;
        state.apply_pending_commit()?;
        self.validate_group(&state)?;
        state.write_to_storage()?;
        provider.purge_epochs(&group.0)?;
        provider.put(&key(b'B', &group.0), &state.current_epoch().to_be_bytes())?;
        provider.put(&key(b'P', &group.0), &0u32.to_be_bytes())?;
        let mut messages = vec![self.encode(output.commit_message)?];
        for welcome in output.welcome_messages {
            messages.push(self.encode(welcome)?);
        }
        Ok(Output {
            messages,
            event: Event::Control,
            sender: None,
            context: Some(OutputContext {
                group,
                epoch: state.current_epoch(),
            }),
        })
    }
    pub fn rekey(
        &self,
        operation: OperationId,
        group: GroupId,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| self.prepare_rekey(provider, group))
    }

    pub(super) fn prepare_rekey(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<Output, Error<B::Error>> {
        self.owner()?;
        let mut state = self.load(provider, group)?;
        let output = state.commit_builder().build()?;
        state.apply_pending_commit()?;
        self.validate_group(&state)?;
        state.write_to_storage()?;
        provider.put(&key(b'P', &group.0), &0u32.to_be_bytes())?;
        Ok(Self::context(
            Output::one(Event::Control, self.encode(output.commit_message)?),
            group,
            state.current_epoch(),
        ))
    }
    pub fn remove(
        &self,
        operation: OperationId,
        group: GroupId,
        leaf: u32,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_remove(provider, group, leaf)
        })
    }

    pub(super) fn prepare_remove(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        leaf: u32,
    ) -> Result<Output, Error<B::Error>> {
        self.owner()?;
        let mut state = self.load(provider, group)?;
        if leaf == state.current_member_index() {
            return Err(PolicyError::Owner.into());
        }
        let output = state.commit_builder().remove_member(leaf)?.build()?;
        state.apply_pending_commit()?;
        self.validate_group(&state)?;
        state.write_to_storage()?;
        provider.purge_epochs(&group.0)?;
        provider.put(&key(b'B', &group.0), &state.current_epoch().to_be_bytes())?;
        provider.put(&key(b'P', &group.0), &0u32.to_be_bytes())?;
        Ok(Self::context(
            Output::one(Event::Control, self.encode(output.commit_message)?),
            group,
            state.current_epoch(),
        ))
    }
    pub fn propose_update(
        &self,
        operation: OperationId,
        group: GroupId,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_propose_update(provider, group)
        })
    }

    pub(super) fn prepare_propose_update(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<Output, Error<B::Error>> {
        self.charge_proposal(provider, group)?;
        let mut state = self.load(provider, group)?;
        let message = state.propose_update(Vec::new())?;
        state.write_to_storage()?;
        Ok(Self::context(
            Output::one(Event::Proposal, self.encode(message)?),
            group,
            state.current_epoch(),
        ))
    }
    pub fn send(
        &self,
        operation: OperationId,
        group: GroupId,
        body: &[u8],
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_send(provider, group, body)
        })
    }

    pub(super) fn prepare_send(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        body: &[u8],
    ) -> Result<Output, Error<B::Error>> {
        if body.len() > self.limits.max_plaintext_bytes {
            return Err(Limit::PlaintextBytes.into());
        }
        let mut state = self.load(provider, group)?;
        let message = state.encrypt_application_message(body, Vec::new())?;
        state.write_to_storage()?;
        Ok(Self::context(
            Output::one(Event::Sent, self.encode(message)?),
            group,
            state.current_epoch(),
        ))
    }
    /// Admission must be pure: apply external effects only after this returns a
    /// committed result. Rejection is persisted with consumed receive keys.
    pub fn receive(
        &self,
        operation: OperationId,
        group: GroupId,
        bytes: &[u8],
        admission: impl FnOnce(&[u8]) -> Admission,
    ) -> Result<Committed, Error<B::Error>> {
        self.receive_with(operation, group, bytes, |_, data| admission(data))
    }
    /// Like `receive`, with the verified endpoint binding available to admission.
    pub fn receive_with(
        &self,
        operation: OperationId,
        group: GroupId,
        bytes: &[u8],
        admission: impl FnOnce(&crate::MemberBinding, &[u8]) -> Admission,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute(operation, |provider| {
            self.prepare_receive(provider, group, bytes, None, |binding, data| {
                Ok::<_, core::convert::Infallible>(admission(binding, data))
            })
            .map_err(CompositionError::into_mls)
        })
    }

    pub(super) fn prepare_receive<A>(
        &self,
        provider: &Providers<B>,
        group: GroupId,
        bytes: &[u8],
        expected: Option<&[crate::MemberBinding]>,
        admission: impl FnOnce(&crate::MemberBinding, &[u8]) -> Result<Admission, A>,
    ) -> Result<Output, CompositionError<B::Error, A>> {
        let message = self.decode(bytes)?;
        if message.wire_format() != WireFormat::PrivateMessage
            || message.group_id() != Some(group.0.as_slice())
        {
            return Err(Error::UnexpectedMessage.into());
        }
        let received_epoch = message.epoch().ok_or(Error::UnexpectedMessage)?;
        let mut state = self.load(provider, group)?;
        let prior_roster = expected.map(|_| state.roster().members());
        let local_index = state.current_member_index();
        let mut sender = None;
        let event = match state.process_incoming_message(message)? {
            ReceivedMessage::ApplicationMessage(data) => {
                let member = state.roster().member_with_index(data.sender_index)?;
                let binding = self
                    .policy
                    .authorized
                    .iter()
                    .find(|binding| binding.matches(&member.signing_identity))
                    .ok_or(PolicyError::Credential)?;
                sender = Some(binding.clone());
                if data.data().len() > self.limits.max_plaintext_bytes {
                    // An authenticated over-limit body is consumed and recorded, too.
                    Event::Rejected(u16::MAX)
                } else {
                    match admission(binding, data.data()).map_err(CompositionError::Application)? {
                        Admission::Accept => {
                            Event::Application(crate::output::copy_bytes(data.data())?)
                        }
                        Admission::Reject(code) => Event::Rejected(code),
                    }
                }
            }
            ReceivedMessage::Proposal(proposal) => {
                if !matches!(proposal.proposal, Proposal::Update(_)) {
                    return Err(PolicyError::UnsupportedControl.into());
                }
                self.charge_proposal(provider, group)?;
                Event::Proposal
            }
            ReceivedMessage::Commit(commit) => {
                let (epoch, removed) = match commit.effect {
                    CommitEffect::NewEpoch(epoch) => (epoch, false),
                    CommitEffect::Removed { new_epoch, .. } => (new_epoch, true),
                    CommitEffect::ReInit(_) => return Err(PolicyError::UnsupportedControl.into()),
                };
                let barrier = epoch
                    .applied_proposals
                    .iter()
                    .any(|p| matches!(p.proposal, Proposal::Add(_) | Proposal::Remove(_)));
                if removed {
                    if let Some(expected) = expected {
                        let prior = prior_roster.as_ref().ok_or(Error::CorruptState)?;
                        let single_local_removal = matches!(
                            epoch.applied_proposals.as_slice(),
                            [proposal] if matches!(&proposal.proposal,
                                Proposal::Remove(remove) if remove.to_remove() == local_index)
                        );
                        if !single_local_removal
                            || expected.len().checked_add(1) != Some(prior.len())
                            || !expected.contains(&self.policy.owner)
                            || prior
                                .iter()
                                .filter(|member| member.index != local_index)
                                .any(|member| {
                                    !expected
                                        .iter()
                                        .any(|binding| binding.matches(&member.signing_identity))
                                })
                            || expected.iter().enumerate().any(|(index, binding)| {
                                expected[..index]
                                    .iter()
                                    .any(|prior| prior.endpoint == binding.endpoint)
                            })
                        {
                            return Err(PolicyError::Credential.into());
                        }
                    }
                    provider.purge_epochs(&group.0)?;
                    provider.delete(&key(b'G', &group.0))?;
                    provider.put(&key(b'R', &group.0), &[1])?;
                    provider.put(&key(b'B', &group.0), &epoch.epoch.to_be_bytes())?;
                    return Ok(Self::context(
                        Output::empty(Event::Removed),
                        group,
                        epoch.epoch,
                    ));
                }
                self.validate_group(&state)?;
                state.write_to_storage()?;
                if barrier {
                    provider.purge_epochs(&group.0)?;
                    provider.put(&key(b'B', &group.0), &state.current_epoch().to_be_bytes())?;
                }
                provider.put(&key(b'P', &group.0), &0u32.to_be_bytes())?;
                return Ok(Self::context(
                    Output::empty(Event::Control),
                    group,
                    state.current_epoch(),
                ));
            }
            ReceivedMessage::GroupInfo(_)
            | ReceivedMessage::Welcome
            | ReceivedMessage::KeyPackage(_) => return Err(Error::UnexpectedMessage.into()),
        };
        state.write_to_storage()?;
        let mut output = Output::empty(event);
        output.sender = sender;
        Ok(Self::context(output, group, received_epoch))
    }
}
