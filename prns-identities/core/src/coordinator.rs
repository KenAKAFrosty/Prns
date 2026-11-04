use crate::*;

mod service_configuration;

/// Authoritative, durable storage for the whole bounded catalog.
///
/// `compare_and_store` must atomically compare the persisted revision and replace
/// the catalog, and return success only after durability. A revision mismatch is
/// an error. ANY error may have committed: the coordinator becomes poisoned and
/// must reload before planning further effects. Implementations must preserve a
/// good old or complete new snapshot, not expose a torn write as success.
pub trait Storage<const I: usize, const O: usize, const S: usize> {
    type Error;
    fn load(&mut self, into: &mut Catalog<I, O, S>) -> Result<(), Self::Error>;
    fn compare_and_store(
        &mut self,
        expected_revision: u64,
        next: &Catalog<I, O, S>,
    ) -> Result<(), Self::Error>;
}

/// One exclusive lifecycle owner, backed by two caller-placed catalog buffers.
///
/// Adapters must fence effects using the ticket's session, and must not run two
/// unfenced owners against one runtime. Storage CAS protects durable commits;
/// it cannot retract side effects already dispatched to a radio or service.
pub struct Coordinator<'a, const I: usize, const O: usize, const S: usize> {
    catalog: &'a mut Catalog<I, O, S>,
    staging: &'a mut Catalog<I, O, S>,
    ready: [bool; I],
    poisoned: bool,
}

impl<'a, const I: usize, const O: usize, const S: usize> Coordinator<'a, I, O, S> {
    pub fn open<T: Storage<I, O, S>>(
        storage: &mut T,
        catalog: &'a mut Catalog<I, O, S>,
        staging: &'a mut Catalog<I, O, S>,
    ) -> Result<Self, Error<T::Error>> {
        let mut owner = Self {
            catalog,
            staging,
            ready: [false; I],
            poisoned: true,
        };
        owner.reload(storage)?;
        Ok(owner)
    }

    /// Reloading grants a fresh durable session and clears all runtime readiness.
    /// Stable enabled identities require `recovery_action` before network admission.
    pub fn reload<T: Storage<I, O, S>>(&mut self, storage: &mut T) -> Result<(), Error<T::Error>> {
        self.poisoned = true;
        self.ready.fill(false);
        storage.load(self.catalog).map_err(Error::Storage)?;
        validate_catalog(self.catalog)?;
        self.staging.copy_from(self.catalog);
        self.staging.session = self
            .catalog
            .session
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        // `persist_staging` is the only operation permitted while poisoned here.
        self.persist_staging(storage)?;
        self.poisoned = false;
        Ok(())
    }

    /// Commit the unchanged logical catalog at a new revision. Native adapters
    /// may co-commit bounded metadata in the same storage CAS. This preserves
    /// session, readiness and existing authority on success; an uncertain write
    /// poisons this coordinator exactly like every other catalog commit.
    pub fn checkpoint<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
    ) -> Result<(), Error<T::Error>> {
        self.healthy()?;
        self.stage();
        self.persist_staging(storage)
    }

    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Diagnostic snapshot only. When poisoned this can be stale; it must never
    /// be used to authorize an effect. Public action/admission methods fail closed.
    pub fn snapshot(&self) -> &Catalog<I, O, S> {
        self.catalog
    }

    pub fn identity(&self, id: LocalIdentityId) -> Result<&IdentityRecord<S>, Fault> {
        self.healthy()?;
        let index = self.identity_index(id)?;
        self.catalog.identities[index]
            .as_ref()
            .ok_or(Fault::UnknownIdentity)
    }

    pub fn operation(&self, token: OperationToken) -> Result<&OperationRecord<S>, Fault> {
        self.healthy()?;
        let index = self.operation_index(token)?;
        self.catalog.operations[index]
            .as_ref()
            .ok_or(Fault::UnknownOperation)
    }

    /// Issue a token without starting any key generation or lifecycle effect.
    /// The caller must retain this token before calling `begin`. A lost reply can
    /// be recovered using `find_reserved` or the bounded operation iterator.
    /// Calling reserve is explicitly requesting a fresh token, not retrying work.
    pub fn reserve<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        command: Command<S>,
        digest: CommandDigest,
    ) -> Result<OperationToken, Error<T::Error>> {
        self.healthy()?;
        validate_command(&command)?;
        if let Command::Transition(prepared) = command {
            let index = self.check_identity_guard(prepared.identity)?;
            let identity = self.catalog.identities[index]
                .as_ref()
                .ok_or(Fault::UnknownIdentity)?;
            if identity.desired == DesiredState::Retired {
                return Err(Fault::Retired.into());
            }
            if identity.pending.is_some()
                || identity.recovery_cleanup.is_some()
                || self.catalog.operations().any(|operation| {
                    operation.state == OperationState::Reserved
                        && matches!(operation.command,
                        Command::Transition(other) if other.identity.id == prepared.identity.id)
                })
            {
                return Err(Fault::Busy.into());
            }
            validate_report(&prepared.review, &identity.services)?;
            validate_service_target(prepared.target, &identity.services)?;
        }
        let slot = self
            .catalog
            .operations
            .iter()
            .position(Option::is_none)
            .ok_or(Fault::CapacityExceeded)?;
        let sequence = self
            .catalog
            .issued_sequence
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let token = OperationToken {
            incarnation: self.catalog.incarnation,
            sequence,
            digest,
        };
        self.stage();
        self.staging.issued_sequence = sequence;
        self.staging.operations[slot] = Some(OperationRecord {
            token,
            command,
            state: OperationState::Reserved,
            unpublished_key: None,
        });
        check_stop_capacity(self.staging)?;
        self.persist_staging(storage)?;
        Ok(token)
    }

    /// Recover the exact retained started/completed stop before requesting fresh
    /// fencing. A historical retry must not disturb a replacement live owner.
    pub fn find_applied_stop(
        &self,
        prepared: PreparedTransition<S>,
        digest: CommandDigest,
    ) -> Result<Option<OperationToken>, Fault> {
        self.healthy()?;
        if !matches!(
            prepared.target,
            TransitionTarget::Pause | TransitionTarget::Retire
        ) {
            return Err(Fault::InvalidTransition);
        }
        let command = Command::Transition(prepared);
        validate_command(&command)?;
        Ok(self
            .catalog
            .operations()
            .find(|operation| {
                operation.command == command
                    && operation.token.digest == digest
                    && operation.state != OperationState::Reserved
                    && operation.state != OperationState::Completed(OperationResult::Cancelled)
            })
            .map(|operation| operation.token))
    }

    /// Preferred stop path, including at ordinary journal saturation. This
    /// atomically issues a token AND records Pause/Retire intent, consuming only
    /// this identity's protected stop slot. External effects still require the
    /// committed ticket from `action`; no teardown happens inside this call.
    ///
    /// Repeating the exact prepared command and digest recovers its retained
    /// token after a lost reply. Unstarted reservations for this identity are
    /// cancelled in the same commit; begun work and key obligations are untouched.
    pub fn apply_stop<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        prepared: PreparedTransition<S>,
        digest: CommandDigest,
    ) -> Result<OperationToken, Error<T::Error>> {
        self.healthy()?;
        if !matches!(
            prepared.target,
            TransitionTarget::Pause | TransitionTarget::Retire
        ) {
            return Err(Fault::InvalidTransition.into());
        }
        let command = Command::Transition(prepared);
        if let Some(token) = self.find_applied_stop(prepared, digest)? {
            return Ok(token);
        }
        let identity_index = self.check_identity_guard(prepared.identity)?;
        let identity = self.identity(prepared.identity.id)?;
        if identity.desired == DesiredState::Retired {
            return Err(Fault::Retired.into());
        }
        if identity.pending.is_some() || identity.recovery_cleanup.is_some() {
            return Err(Fault::Busy.into());
        }
        validate_report(&prepared.review, &identity.services)?;
        if prepared
            .review
            .entries
            .iter()
            .flatten()
            .any(|entry| !entry.quiescence_supported)
        {
            return Err(Fault::UnqualifiedService.into());
        }
        let next_lifecycle = identity
            .lifecycle_revision
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let reserved = self
            .catalog
            .operations
            .iter()
            .enumerate()
            .find_map(|(index, operation)| {
                operation
                    .as_ref()
                    .filter(|operation| {
                        operation.state == OperationState::Reserved
                            && operation.command == command
                            && operation.token.digest == digest
                    })
                    .map(|operation| (index, operation.token))
            });
        let (slot, token) = if let Some(existing) = reserved {
            existing
        } else {
            let slot = self
                .catalog
                .operations
                .iter()
                .position(Option::is_none)
                .ok_or(Fault::CapacityExceeded)?;
            let sequence = self
                .catalog
                .issued_sequence
                .checked_add(1)
                .ok_or(Fault::Exhausted)?;
            (
                slot,
                OperationToken {
                    incarnation: self.catalog.incarnation,
                    sequence,
                    digest,
                },
            )
        };
        self.stage();
        for operation in self.staging.operations.iter_mut().flatten() {
            if operation.state == OperationState::Reserved
                && matches!(operation.command,
                Command::Transition(other) if other.identity.id == prepared.identity.id)
            {
                operation.state = OperationState::Completed(OperationResult::Cancelled);
            }
        }
        self.staging.issued_sequence = self.staging.issued_sequence.max(token.sequence);
        self.staging.operations[slot] = Some(OperationRecord {
            token,
            command,
            state: OperationState::Transitioning,
            unpublished_key: None,
        });
        let identity = self.staging.identities[identity_index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?;
        identity.desired = DesiredState::Paused;
        identity.lifecycle_revision = next_lifecycle;
        identity.pending = Some(PendingTransition {
            operation: token,
            target: prepared.target,
            phase: TransitionPhase::Quiescing,
        });
        check_stop_capacity(self.staging)?;
        self.persist_staging(storage)?;
        self.ready[identity_index] = false;
        Ok(token)
    }

    pub fn find_reserved(
        &self,
        command: &Command<S>,
        digest: CommandDigest,
    ) -> Result<Option<OperationToken>, Fault> {
        self.healthy()?;
        Ok(self
            .catalog
            .operations()
            .filter(|operation| {
                operation.state == OperationState::Reserved
                    && operation.command == *command
                    && operation.token.digest == digest
            })
            .min_by_key(|operation| operation.token.sequence)
            .map(|operation| operation.token))
    }

    /// Exact-result lookup is permitted after retirement; this never executes work.
    pub fn resume(
        &self,
        token: OperationToken,
        command: &Command<S>,
    ) -> Result<OperationState, Fault> {
        let operation = self.operation(token)?;
        if operation.command != *command {
            return Err(Fault::CommandMismatch);
        }
        Ok(operation.state)
    }

    /// Start a previously retained token. Repeating the exact command returns its
    /// current durable state and cannot execute it a second time.
    pub fn begin<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        token: OperationToken,
        command: &Command<S>,
    ) -> Result<OperationState, Error<T::Error>> {
        let state = self.resume(token, command)?;
        if state != OperationState::Reserved {
            return Ok(state);
        }
        let operation_index = self.operation_index(token)?;
        self.stage();
        let next = match *command {
            Command::Create(request) => {
                if let KeySource::Import { expected_hash } = request.source {
                    if let Some(existing) = self
                        .catalog
                        .identities()
                        .find(|i| i.public_hash == expected_hash)
                    {
                        OperationState::Completed(OperationResult::Existing {
                            identity: existing.id,
                        })
                    } else {
                        if self.catalog.operations().any(|other| {
                            other.state == OperationState::AwaitingKey && matches!(other.command,
                                Command::Create(CreateRequest { source: KeySource::Import { expected_hash: hash }, .. })
                                if hash == expected_hash)
                        }) { return Err(Fault::Busy.into()); }
                        self.check_creation_capacity()?;
                        OperationState::AwaitingKey
                    }
                } else {
                    self.check_creation_capacity()?;
                    OperationState::AwaitingKey
                }
            }
            Command::Transition(prepared) => {
                let index = self.check_identity_guard(prepared.identity)?;
                let current = self.catalog.identities[index]
                    .as_ref()
                    .ok_or(Fault::UnknownIdentity)?;
                if current.desired == DesiredState::Retired {
                    return Err(Fault::Retired.into());
                }
                if current.pending.is_some() || current.recovery_cleanup.is_some() {
                    return Err(Fault::Busy.into());
                }
                validate_report(&prepared.review, &current.services)?;
                validate_service_target(prepared.target, &current.services)?;
                if prepared
                    .review
                    .entries
                    .iter()
                    .flatten()
                    .any(|entry| !entry.quiescence_supported)
                {
                    return Err(Fault::UnqualifiedService.into());
                }
                let revision = current
                    .lifecycle_revision
                    .checked_add(1)
                    .ok_or(Fault::Exhausted)?;
                let record = self.staging.identities[index]
                    .as_mut()
                    .ok_or(Fault::UnknownIdentity)?;
                record.lifecycle_revision = revision;
                record.desired = match prepared.target {
                    TransitionTarget::Enable => DesiredState::Enabled,
                    TransitionTarget::Pause
                    | TransitionTarget::Retire
                    | TransitionTarget::AddService(_)
                    | TransitionTarget::RemoveService(_) => DesiredState::Paused,
                };
                record.pending = Some(PendingTransition {
                    operation: token,
                    target: prepared.target,
                    phase: match prepared.target {
                        TransitionTarget::Enable => TransitionPhase::Activating,
                        TransitionTarget::Pause
                        | TransitionTarget::Retire
                        | TransitionTarget::AddService(_)
                        | TransitionTarget::RemoveService(_) => TransitionPhase::Quiescing,
                    },
                });
                OperationState::Transitioning
            }
        };
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = next;
        self.persist_staging(storage)?;
        if let Command::Transition(prepared) = command {
            let index = self.identity_index(prepared.identity.id)?;
            self.ready[index] = false;
        }
        Ok(next)
    }

    pub fn cancel_reserved<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        token: OperationToken,
    ) -> Result<(), Error<T::Error>> {
        let operation = self.operation(token)?;
        if operation.state == OperationState::Completed(OperationResult::Cancelled) {
            return Ok(());
        }
        if operation.state != OperationState::Reserved {
            return Err(Fault::AlreadyStarted.into());
        }
        let index = self.operation_index(token)?;
        self.stage();
        self.staging.operations[index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = OperationState::Completed(OperationResult::Cancelled);
        self.persist_staging(storage)
    }

    /// Advisory only. The caller obtains service observations without holding an
    /// admission gate. Apply/begin fences this identity; quiesce acknowledgement
    /// must then supply a fresh authoritative report after draining admitted work.
    pub fn prepare(
        &self,
        id: LocalIdentityId,
        target: TransitionTarget,
        report: DependencyReport<S>,
    ) -> Result<PreparedTransition<S>, Fault> {
        let identity = self.identity(id)?;
        if identity.desired == DesiredState::Retired {
            return Err(Fault::Retired);
        }
        if identity.pending.is_some() || identity.recovery_cleanup.is_some() {
            return Err(Fault::Busy);
        }
        validate_report(&report, &identity.services)?;
        validate_service_target(target, &identity.services)?;
        Ok(PreparedTransition {
            identity: identity.guard(),
            target,
            review: report,
        })
    }

    pub fn action(&self, token: OperationToken) -> Result<Option<Action<S>>, Fault> {
        let operation = self.operation(token)?;
        match (operation.state, operation.command) {
            (OperationState::Reserved | OperationState::Completed(_), _) => Ok(None),
            (OperationState::AwaitingKey, Command::Create(request)) => {
                let id = LocalIdentityId {
                    incarnation: token.incarnation,
                    sequence: token.sequence,
                };
                Ok(Some(Action::PublishKey {
                    ticket: EffectTicket {
                        operation: Some(token),
                        identity: id,
                        session: self.catalog.session,
                        lifecycle_revision: 0,
                        kind: EffectKind::PublishKey,
                    },
                    key_reference: key_reference(id),
                    source: request.source,
                }))
            }
            (OperationState::Transitioning, Command::Transition(prepared)) => {
                let identity = self.identity(prepared.identity.id)?;
                let pending = identity.pending.ok_or(Fault::InvalidCatalog)?;
                if pending.operation != token {
                    return Err(Fault::InvalidCatalog);
                }
                let kind = match pending.phase {
                    TransitionPhase::Activating => EffectKind::Activate,
                    TransitionPhase::InitializingService => EffectKind::InitializeService,
                    TransitionPhase::Quiescing | TransitionPhase::ActivationCleanup(_) => {
                        EffectKind::Quiesce
                    }
                };
                let ticket = EffectTicket {
                    operation: Some(token),
                    identity: identity.id,
                    session: self.catalog.session,
                    lifecycle_revision: identity.lifecycle_revision,
                    kind,
                };
                Ok(Some(match pending.phase {
                    TransitionPhase::InitializingService => {
                        let TransitionTarget::AddService(service) = pending.target else {
                            return Err(Fault::InvalidCatalog);
                        };
                        Action::InitializeService {
                            ticket,
                            identity: identity.guard(),
                            key_reference: identity.key_reference,
                            service,
                        }
                    }
                    TransitionPhase::Activating => Action::Activate {
                        ticket,
                        identity: identity.guard(),
                        key_reference: identity.key_reference,
                        services: identity.services,
                    },
                    _ => Action::Quiesce {
                        ticket,
                        identity: identity.guard(),
                        services: identity.services,
                    },
                }))
            }
            _ => Err(Fault::InvalidCatalog),
        }
    }

    /// Restore runtime for a stable enabled identity after restart. This reopens
    /// the existing key; it never creates a replacement or a new durable sender.
    pub fn recovery_action(&self, id: LocalIdentityId) -> Result<Option<Action<S>>, Fault> {
        let identity = self.identity(id)?;
        if let Some(pending) = identity.pending {
            return self.action(pending.operation);
        }
        if identity.recovery_cleanup.is_some() {
            return Ok(Some(Action::Quiesce {
                ticket: EffectTicket {
                    operation: None,
                    identity: id,
                    session: self.catalog.session,
                    lifecycle_revision: identity.lifecycle_revision,
                    kind: EffectKind::RestoreCleanup,
                },
                identity: identity.guard(),
                services: identity.services,
            }));
        }
        let index = self.identity_index(id)?;
        if identity.desired != DesiredState::Enabled || self.ready[index] {
            return Ok(None);
        }
        Ok(Some(Action::Activate {
            ticket: EffectTicket {
                operation: None,
                identity: id,
                session: self.catalog.session,
                lifecycle_revision: identity.lifecycle_revision,
                kind: EffectKind::Restore,
            },
            identity: identity.guard(),
            key_reference: identity.key_reference,
            services: identity.services,
        }))
    }

    pub fn ack_key<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
        verified: VerifiedKey,
    ) -> Result<OperationResult, Error<T::Error>> {
        self.check_session(ticket)?;
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        if let OperationState::Completed(result) = self.operation(token)?.state {
            self.check_completed_ticket(ticket, EffectKind::PublishKey)?;
            if let OperationResult::Created { identity } | OperationResult::Existing { identity } =
                result
            {
                let record = self.identity(identity)?;
                if key_reference(ticket.identity) != verified.key_reference
                    || record.public_hash != verified.public_hash
                {
                    return Err(Fault::KeyMismatch.into());
                }
                return Ok(result);
            }
            return Err(Fault::StaleEffect.into());
        }
        self.check_effect(ticket)?;
        let operation_index = self.operation_index(token)?;
        let Command::Create(request) = self.operation(token)?.command else {
            return Err(Fault::StaleEffect.into());
        };
        if verified.key_reference != key_reference(ticket.identity) {
            return Err(Fault::KeyMismatch.into());
        }
        if let KeySource::Import { expected_hash } = request.source {
            if expected_hash != verified.public_hash {
                return Err(Fault::KeyMismatch.into());
            }
        }
        // Concurrent publication of this key must not create duplicate personas.
        // Unknown/possibly imported vault material is retained, never auto-erased.
        let duplicate = self
            .catalog
            .identities()
            .find(|existing| existing.public_hash == verified.public_hash)
            .map(|existing| existing.id);
        if let Some(existing) = duplicate {
            if !matches!(request.source, KeySource::Import { .. }) {
                return Err(Fault::KeyMismatch.into());
            }
            let result = OperationResult::Existing { identity: existing };
            self.stage();
            let operation = self.staging.operations[operation_index]
                .as_mut()
                .ok_or(Fault::UnknownOperation)?;
            operation.state = OperationState::Completed(result);
            operation.unpublished_key = Some(verified.key_reference);
            self.persist_staging(storage)?;
            return Ok(result);
        }
        let identity_index = self
            .catalog
            .identities
            .iter()
            .position(Option::is_none)
            .ok_or(Fault::CapacityExceeded)?;
        let result = OperationResult::Created {
            identity: ticket.identity,
        };
        self.stage();
        self.staging.identities[identity_index] = Some(IdentityRecord {
            id: ticket.identity,
            public_hash: verified.public_hash,
            key_reference: verified.key_reference,
            metadata: request.metadata,
            metadata_revision: 1,
            lifecycle_revision: 1,
            desired: DesiredState::Paused,
            initial_services: request.services,
            services: request.services,
            pending: None,
            recovery_cleanup: None,
        });
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = OperationState::Completed(result);
        self.persist_staging(storage)?;
        Ok(result)
    }

    pub fn ack_activated<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
    ) -> Result<(), Error<T::Error>> {
        self.check_session(ticket)?;
        if ticket.kind == EffectKind::Restore {
            if ticket.operation.is_some() {
                return Err(Fault::StaleEffect.into());
            }
            let index = self.identity_index(ticket.identity)?;
            if self.ready[index] {
                let identity = self.identity(ticket.identity)?;
                if identity.pending.is_none()
                    && identity.desired == DesiredState::Enabled
                    && identity.lifecycle_revision == ticket.lifecycle_revision
                {
                    return Ok(());
                }
            }
            self.check_effect(ticket)?;
            // Also CAS on restore acknowledgements: a replaced owner cannot mark
            // its stale in-memory runtime ready after another session has opened.
            self.stage();
            self.persist_staging(storage)?;
            self.ready[index] = true;
            return Ok(());
        }
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        if matches!(
            self.operation(token)?.state,
            OperationState::Completed(OperationResult::Enabled { .. })
        ) {
            self.check_completed_ticket(ticket, EffectKind::Activate)?;
            return Ok(());
        }
        self.check_effect(ticket)?;
        if ticket.kind != EffectKind::Activate {
            return Err(Fault::StaleEffect.into());
        }
        let index = self.identity_index(ticket.identity)?;
        let operation_index = self.operation_index(token)?;
        self.stage();
        self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?
            .pending = None;
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = OperationState::Completed(OperationResult::Enabled {
            identity: ticket.identity,
        });
        self.persist_staging(storage)?;
        self.ready[index] = true;
        Ok(())
    }

    /// Activation failures require confirmed scoped cleanup before Paused is
    /// complete. A partial registration must not become an invisible listener.
    pub fn activation_failed<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
        reason: ServiceFailure,
    ) -> Result<(), Error<T::Error>> {
        self.check_effect(ticket)?;
        if ticket.kind == EffectKind::Restore {
            // Cleanup has reserved per-identity space even when every operation
            // result slot is full. It survives restart and preserves Enabled.
            let index = self.identity_index(ticket.identity)?;
            let next_revision = self
                .identity(ticket.identity)?
                .lifecycle_revision
                .checked_add(1)
                .ok_or(Fault::Exhausted)?;
            self.stage();
            let record = self.staging.identities[index]
                .as_mut()
                .ok_or(Fault::UnknownIdentity)?;
            record.recovery_cleanup = Some(reason);
            record.lifecycle_revision = next_revision;
            self.persist_staging(storage)?;
            self.ready[index] = false;
            return Ok(());
        }
        if ticket.kind != EffectKind::Activate {
            return Err(Fault::StaleEffect.into());
        }
        let index = self.identity_index(ticket.identity)?;
        self.stage();
        let record = self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?;
        record.desired = DesiredState::Paused;
        record.pending.as_mut().ok_or(Fault::StaleEffect)?.phase =
            TransitionPhase::ActivationCleanup(reason);
        self.persist_staging(storage)?;
        self.ready[index] = false;
        Ok(())
    }

    /// Finish scoped cleanup after a failed restart restoration. The identity
    /// remains Enabled but unavailable; a later recovery action may restore it.
    /// This path cannot be pruned or blocked by a full operation-result journal.
    pub fn ack_recovery_quiesced<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
        report: DependencyReport<S>,
    ) -> Result<(), Error<T::Error>> {
        self.check_effect(ticket)?;
        if ticket.kind != EffectKind::RestoreCleanup {
            return Err(Fault::StaleEffect.into());
        }
        let index = self.identity_index(ticket.identity)?;
        let identity = self.identity(ticket.identity)?;
        validate_report(&report, &identity.services)?;
        if report
            .entries
            .iter()
            .flatten()
            .any(|entry| !entry.quiescence_supported)
        {
            return Err(Fault::UnqualifiedService.into());
        }
        self.stage();
        self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?
            .recovery_cleanup = None;
        self.persist_staging(storage)?;
        self.ready[index] = false;
        Ok(())
    }

    /// Acknowledges authoritative quiescence after admission fencing, draining,
    /// ratchet preservation, and all owned ingress/egress cleanup. Retirement
    /// rechecks the exact reviewed service consequences and blockers here.
    pub fn ack_quiesced<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
        report: DependencyReport<S>,
    ) -> Result<OperationResult, Error<T::Error>> {
        self.check_session(ticket)?;
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        if let OperationState::Completed(result) = self.operation(token)?.state {
            if matches!(
                result,
                OperationResult::Paused { .. }
                    | OperationResult::Retired { .. }
                    | OperationResult::RetirementBlocked { .. }
                    | OperationResult::ActivationFailed { .. }
            ) {
                self.check_completed_ticket(ticket, EffectKind::Quiesce)?;
                return Ok(result);
            }
            return Err(Fault::StaleEffect.into());
        }
        self.check_effect(ticket)?;
        if ticket.kind != EffectKind::Quiesce {
            return Err(Fault::StaleEffect.into());
        }
        let index = self.identity_index(ticket.identity)?;
        let identity = self.identity(ticket.identity)?;
        let pending = identity.pending.ok_or(Fault::StaleEffect)?;
        // A malformed/incomplete report cannot prove quiescence. Keep the
        // journal fenced until a complete report or authoritative recovery.
        validate_report(&report, &identity.services)?;
        if report
            .entries
            .iter()
            .flatten()
            .any(|entry| !entry.quiescence_supported)
        {
            return Err(Fault::UnqualifiedService.into());
        }
        let Command::Transition(prepared) = self.operation(token)?.command else {
            return Err(Fault::InvalidCatalog.into());
        };
        let (desired, result) = match pending.phase {
            TransitionPhase::ActivationCleanup(reason) => (
                DesiredState::Paused,
                OperationResult::ActivationFailed {
                    identity: identity.id,
                    reason,
                },
            ),
            TransitionPhase::Quiescing => match pending.target {
                TransitionTarget::Pause => (
                    DesiredState::Paused,
                    OperationResult::Paused {
                        identity: identity.id,
                    },
                ),
                TransitionTarget::Retire => {
                    let blocked = if !reports_equal(&prepared.review, &report) {
                        Some(RetirementBlock::ChangedDependencies)
                    } else {
                        retirement_block(&report)
                    };
                    match blocked {
                        Some(reason) => (
                            DesiredState::Paused,
                            OperationResult::RetirementBlocked {
                                identity: identity.id,
                                reason,
                            },
                        ),
                        None => (
                            DesiredState::Retired,
                            OperationResult::Retired {
                                identity: identity.id,
                            },
                        ),
                    }
                }
                TransitionTarget::Enable
                | TransitionTarget::AddService(_)
                | TransitionTarget::RemoveService(_) => return Err(Fault::InvalidCatalog.into()),
            },
            TransitionPhase::Activating | TransitionPhase::InitializingService => {
                return Err(Fault::StaleEffect.into());
            }
        };
        let operation_index = self.operation_index(token)?;
        self.stage();
        let record = self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?;
        record.pending = None;
        record.desired = desired;
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = OperationState::Completed(result);
        self.persist_staging(storage)?;
        self.ready[index] = false;
        Ok(result)
    }

    pub fn update_metadata<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        guard: MetadataGuard,
        metadata: Metadata,
    ) -> Result<(), Error<T::Error>> {
        self.healthy()?;
        validate_metadata(metadata)?;
        let index = self.identity_index(guard.id)?;
        let record = self.identity(guard.id)?;
        if record.public_hash != guard.public_hash {
            return Err(Fault::IdentityChanged.into());
        }
        if record.metadata_revision != guard.metadata_revision {
            return Err(Fault::StaleMetadata.into());
        }
        if record.desired == DesiredState::Retired {
            return Err(Fault::Retired.into());
        }
        if record.pending.is_some() || record.recovery_cleanup.is_some() {
            return Err(Fault::Busy.into());
        }
        let revision = record
            .metadata_revision
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        self.stage();
        let record = self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?;
        record.metadata = metadata;
        record.metadata_revision = revision;
        self.persist_staging(storage)
    }

    pub fn admit(
        &self,
        id: LocalIdentityId,
        public_hash: PublicIdentityHash,
        class: AdmissionClass,
    ) -> Result<AdmissionGuard, Fault> {
        let identity = self.identity(id)?;
        if identity.public_hash != public_hash {
            return Err(Fault::IdentityChanged);
        }
        if !matches!(class, AdmissionClass::History | AdmissionClass::Recovery) {
            if identity.desired == DesiredState::Retired {
                return Err(Fault::Retired);
            }
            if identity.pending.is_some() || identity.recovery_cleanup.is_some() {
                return Err(Fault::Busy);
            }
            if matches!(
                class,
                AdmissionClass::ServiceMutation | AdmissionClass::Network
            ) {
                if identity.desired == DesiredState::Paused {
                    return Err(Fault::Paused);
                }
                if class == AdmissionClass::Network && !self.ready[self.identity_index(id)?] {
                    return Err(Fault::RuntimeUnavailable);
                }
            }
        }
        Ok(AdmissionGuard {
            identity: identity.guard(),
            session: self.catalog.session,
            class,
        })
    }

    /// Validates a fresh action against its captured owner. Recovery/history may
    /// still refer to the same durable owner after lifecycle/session changes;
    /// the service must independently bind recovery to a previously admitted ID.
    pub fn validate_admission(&self, guard: AdmissionGuard) -> Result<(), Fault> {
        let current = self.admit(guard.identity.id, guard.identity.public_hash, guard.class)?;
        if matches!(
            guard.class,
            AdmissionClass::History | AdmissionClass::Recovery
        ) {
            return Ok(());
        }
        if current.session != guard.session {
            return Err(Fault::StaleSession);
        }
        if current.identity.lifecycle_revision != guard.identity.lifecycle_revision {
            return Err(Fault::StaleLifecycle);
        }
        Ok(())
    }

    /// Recheck a queued effect immediately before executing it under the
    /// adapter's exclusive session/admission gate. This does not itself acquire
    /// that gate or retract any effect already handed to an external system.
    pub fn validate_effect(&self, ticket: EffectTicket) -> Result<(), Fault> {
        self.check_effect(ticket)
    }

    pub fn runtime_status(&self, id: LocalIdentityId) -> Result<RuntimeStatus, Fault> {
        let identity = self.identity(id)?;
        if identity.recovery_cleanup.is_some() {
            return Ok(RuntimeStatus::Stopping);
        }
        if let Some(pending) = identity.pending {
            return Ok(match pending.phase {
                TransitionPhase::Activating => RuntimeStatus::Starting,
                _ => RuntimeStatus::Stopping,
            });
        }
        if identity.desired == DesiredState::Enabled && self.ready[self.identity_index(id)?] {
            Ok(RuntimeStatus::Running)
        } else {
            Ok(RuntimeStatus::Unavailable)
        }
    }

    /// Prune only a contiguous completed prefix. The monotonic floor permanently
    /// rejects those tokens; their commands can never be implicitly reexecuted.
    pub fn prune_completed<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        through_sequence: u64,
    ) -> Result<(), Error<T::Error>> {
        self.healthy()?;
        if through_sequence < self.catalog.completed_floor
            || through_sequence > self.catalog.issued_sequence
        {
            return Err(Fault::InvalidPruneFloor.into());
        }
        if through_sequence == self.catalog.completed_floor {
            return Ok(());
        }
        for operation in self.catalog.operations() {
            if operation.token.sequence <= through_sequence && operation.unpublished_key.is_some() {
                return Err(Fault::UnaccountedKey.into());
            }
            if operation.token.sequence <= through_sequence
                && !matches!(operation.state, OperationState::Completed(_))
            {
                return Err(Fault::CannotPrunePending.into());
            }
        }
        self.stage();
        for operation in &mut self.staging.operations {
            if operation
                .as_ref()
                .is_some_and(|operation| operation.token.sequence <= through_sequence)
            {
                *operation = None;
            }
        }
        self.staging.completed_floor = through_sequence;
        self.persist_staging(storage)
    }

    /// A duplicate import's extra vault reference must be durably recorded by
    /// the adapter before this fence is released. This authorizes no deletion;
    /// imported/uncertain keys remain retained for explicit recovery policy.
    pub fn acknowledge_retained_key<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        token: OperationToken,
        reference: KeyReference,
    ) -> Result<(), Error<T::Error>> {
        let operation = self.operation(token)?;
        if !matches!(
            (operation.command, operation.state),
            (
                Command::Create(CreateRequest {
                    source: KeySource::Import { .. },
                    ..
                }),
                OperationState::Completed(OperationResult::Existing { .. })
            )
        ) {
            return Err(Fault::StaleEffect.into());
        }
        let expected = key_reference(LocalIdentityId {
            incarnation: token.incarnation,
            sequence: token.sequence,
        });
        if reference != expected {
            return Err(Fault::KeyMismatch.into());
        }
        let Some(retained) = operation.unpublished_key else {
            return Ok(());
        };
        if retained != reference {
            return Err(Fault::KeyMismatch.into());
        }
        let index = self.operation_index(token)?;
        self.stage();
        self.staging.operations[index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .unpublished_key = None;
        self.persist_staging(storage)
    }

    fn healthy(&self) -> Result<(), Fault> {
        if self.poisoned {
            Err(Fault::Poisoned)
        } else {
            Ok(())
        }
    }

    fn stage(&mut self) {
        self.staging.copy_from(self.catalog);
    }

    fn persist_staging<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
    ) -> Result<(), Error<T::Error>> {
        self.staging.revision = self
            .catalog
            .revision
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        if let Err(error) = validate_catalog(self.staging) {
            self.poisoned = true;
            return Err(error.into());
        }
        if let Err(error) = storage.compare_and_store(self.catalog.revision, self.staging) {
            self.poisoned = true;
            self.ready.fill(false);
            return Err(Error::Storage(error));
        }
        self.catalog.copy_from(self.staging);
        Ok(())
    }

    fn identity_index(&self, id: LocalIdentityId) -> Result<usize, Fault> {
        self.catalog
            .identities
            .iter()
            .position(|record| record.as_ref().is_some_and(|record| record.id == id))
            .ok_or(Fault::UnknownIdentity)
    }

    fn operation_index(&self, token: OperationToken) -> Result<usize, Fault> {
        if token.incarnation != self.catalog.incarnation || token.sequence == 0 {
            return Err(Fault::InvalidToken);
        }
        if token.sequence <= self.catalog.completed_floor {
            return Err(Fault::ReplayExpired);
        }
        let index = self
            .catalog
            .operations
            .iter()
            .position(|record| {
                record
                    .as_ref()
                    .is_some_and(|record| record.token.sequence == token.sequence)
            })
            .ok_or(Fault::UnknownOperation)?;
        if self.catalog.operations[index]
            .as_ref()
            .ok_or(Fault::UnknownOperation)?
            .token
            != token
        {
            return Err(Fault::CommandMismatch);
        }
        Ok(index)
    }

    fn check_identity_guard(&self, guard: IdentityGuard) -> Result<usize, Fault> {
        let index = self.identity_index(guard.id)?;
        let identity = self.identity(guard.id)?;
        if identity.public_hash != guard.public_hash {
            return Err(Fault::IdentityChanged);
        }
        if identity.lifecycle_revision != guard.lifecycle_revision {
            return Err(Fault::StaleLifecycle);
        }
        Ok(index)
    }

    fn check_creation_capacity(&self) -> Result<(), Fault> {
        let occupied = self.catalog.identities().count();
        let reserved = self
            .catalog
            .operations()
            .filter(|operation| operation.state == OperationState::AwaitingKey)
            .count();
        if occupied.saturating_add(reserved) >= I {
            Err(Fault::CapacityExceeded)
        } else {
            Ok(())
        }
    }

    fn check_session(&self, ticket: EffectTicket) -> Result<(), Fault> {
        self.healthy()?;
        if ticket.session != self.catalog.session {
            Err(Fault::StaleSession)
        } else {
            Ok(())
        }
    }

    fn check_effect(&self, ticket: EffectTicket) -> Result<(), Fault> {
        self.check_session(ticket)?;
        let action = match ticket.operation {
            Some(token) => self.action(token)?,
            None => self.recovery_action(ticket.identity)?,
        }
        .ok_or(Fault::StaleEffect)?;
        if action.ticket() != ticket {
            return Err(Fault::StaleEffect);
        }
        Ok(())
    }

    fn check_completed_ticket(&self, ticket: EffectTicket, kind: EffectKind) -> Result<(), Fault> {
        if ticket.kind != kind {
            return Err(Fault::StaleEffect);
        }
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        let operation = self.operation(token)?;
        let (id, lifecycle_revision) = match operation.command {
            Command::Create(_) => (
                LocalIdentityId {
                    incarnation: token.incarnation,
                    sequence: token.sequence,
                },
                0,
            ),
            Command::Transition(prepared) => (
                prepared.identity.id,
                prepared
                    .identity
                    .lifecycle_revision
                    .checked_add(1)
                    .ok_or(Fault::Exhausted)?,
            ),
        };
        if ticket.identity != id || ticket.lifecycle_revision != lifecycle_revision {
            return Err(Fault::StaleEffect);
        }
        Ok(())
    }
}

fn key_reference(id: LocalIdentityId) -> KeyReference {
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(&id.incarnation);
    bytes[16..24].copy_from_slice(&id.sequence.to_be_bytes());
    bytes[24..].copy_from_slice(b"IDENTITY");
    KeyReference(bytes)
}

fn validate_metadata(metadata: Metadata) -> Result<(), Fault> {
    if metadata.local_label.as_str().is_empty() {
        Err(Fault::InvalidMetadata)
    } else {
        Ok(())
    }
}

fn validate_services<const S: usize>(services: &ServiceSet<S>) -> Result<(), Fault> {
    let mut ended = false;
    for (index, entry) in services.entries.iter().enumerate() {
        match entry {
            None => ended = true,
            Some(service) => {
                if ended || services.entries[..index].contains(&Some(*service)) {
                    return Err(Fault::InvalidServices);
                }
            }
        }
    }
    Ok(())
}

fn validate_report<const S: usize>(
    report: &DependencyReport<S>,
    services: &ServiceSet<S>,
) -> Result<(), Fault> {
    if !report.complete {
        return Err(Fault::IncompleteReport);
    }
    if report.entries.iter().flatten().count() != services.iter().count() {
        return Err(Fault::InvalidReport);
    }
    for (index, entry) in report.entries.iter().enumerate() {
        if let Some(entry) = entry {
            if !services.iter().any(|service| service == entry.service)
                || report.entries[..index]
                    .iter()
                    .flatten()
                    .any(|earlier| earlier.service == entry.service)
            {
                return Err(Fault::InvalidReport);
            }
        }
    }
    Ok(())
}

fn reports_equal<const S: usize>(left: &DependencyReport<S>, right: &DependencyReport<S>) -> bool {
    left.complete == right.complete
        && left.other_key_uses == right.other_key_uses
        && left.entries.iter().flatten().count() == right.entries.iter().flatten().count()
        && left
            .entries
            .iter()
            .flatten()
            .all(|entry| right.entries.iter().flatten().any(|other| other == entry))
}

fn validate_service_target<const S: usize>(
    target: TransitionTarget,
    services: &ServiceSet<S>,
) -> Result<(), Fault> {
    match target {
        TransitionTarget::AddService(service) => {
            if services.iter().any(|id| id == service) {
                return Err(Fault::InvalidServices);
            }
            if services.iter().count() == S {
                return Err(Fault::CapacityExceeded);
            }
        }
        TransitionTarget::RemoveService(service) if !services.iter().any(|id| id == service) => {
            return Err(Fault::InvalidServices);
        }
        _ => {}
    }
    Ok(())
}

fn retirement_block<const S: usize>(report: &DependencyReport<S>) -> Option<RetirementBlock> {
    if !report.complete {
        return Some(RetirementBlock::IncompleteReport);
    }
    for entry in report.entries.iter().flatten() {
        if !entry.quiescence_supported {
            return Some(RetirementBlock::UnqualifiedService);
        }
        if entry.recovery_pending {
            return Some(RetirementBlock::RecoveryPending);
        }
        if entry.authority_count != 0 {
            return Some(RetirementBlock::AuthorityOwned);
        }
        if entry.unsettled_control != 0 {
            return Some(RetirementBlock::UnsettledControl);
        }
        if entry.pending_messages != 0 {
            return Some(RetirementBlock::PendingMessages);
        }
    }
    None
}

/// Validate the canonical creation intent independently of reserving an operation.
pub fn validate_create_request<const S: usize>(request: &CreateRequest<S>) -> Result<(), Fault> {
    validate_metadata(request.metadata)?;
    validate_services(&request.services)
}

fn validate_command<const S: usize>(command: &Command<S>) -> Result<(), Fault> {
    match command {
        Command::Create(request) => validate_create_request(request),
        Command::Transition(prepared) => {
            if prepared.identity.id.sequence == 0 || prepared.identity.lifecycle_revision == 0 {
                return Err(Fault::InvalidCatalog);
            }
            if !prepared.review.complete {
                return Err(Fault::IncompleteReport);
            }
            match prepared.target {
                TransitionTarget::AddService(service)
                    if prepared
                        .review
                        .entries
                        .iter()
                        .flatten()
                        .any(|entry| entry.service == service) =>
                {
                    return Err(Fault::InvalidServices);
                }
                TransitionTarget::RemoveService(service)
                    if !prepared
                        .review
                        .entries
                        .iter()
                        .flatten()
                        .any(|entry| entry.service == service) =>
                {
                    return Err(Fault::InvalidServices);
                }
                _ => {}
            }
            for (index, entry) in prepared.review.entries.iter().enumerate() {
                if entry.as_ref().is_some_and(|entry| {
                    prepared.review.entries[..index]
                        .iter()
                        .flatten()
                        .any(|prior| prior.service == entry.service)
                }) {
                    return Err(Fault::InvalidReport);
                }
            }
            Ok(())
        }
    }
}

/// Ordinary reservations never consume the last stop slot of a current or
/// prospectively enabled identity. The atomic stop path changes desired state
/// before consuming its own slot, and therefore cannot steal another's reserve.
fn check_stop_capacity<const I: usize, const O: usize, const S: usize>(
    catalog: &Catalog<I, O, S>,
) -> Result<(), Fault> {
    let protected = catalog
        .identities()
        .filter(|identity| {
            identity.desired == DesiredState::Enabled
                || catalog.operations().any(|operation| {
                    operation.state == OperationState::Reserved
                        && matches!(operation.command,
                Command::Transition(prepared) if prepared.identity.id == identity.id
                    && prepared.target == TransitionTarget::Enable)
                })
        })
        .count();
    let free = catalog
        .operations
        .iter()
        .filter(|operation| operation.is_none())
        .count();
    if free < protected {
        Err(Fault::CapacityExceeded)
    } else {
        Ok(())
    }
}

/// Validate typed decoded storage before it can authorize effects. Callers may
/// also use this on a checkpoint before submitting it to their durable adapter.
pub fn validate_catalog<const I: usize, const O: usize, const S: usize>(
    catalog: &Catalog<I, O, S>,
) -> Result<(), Fault> {
    if catalog.format != CATALOG_FORMAT
        || catalog.incarnation == [0; 16]
        || catalog.completed_floor > catalog.issued_sequence
        || catalog.session > catalog.revision
        || (catalog.revision != 0 && catalog.session == 0)
        || catalog.issued_sequence > catalog.revision
    {
        return Err(Fault::InvalidCatalog);
    }
    let retained = catalog.issued_sequence - catalog.completed_floor;
    if retained != catalog.operations().count() as u64 || retained > O as u64 {
        return Err(Fault::InvalidCatalog);
    }
    for (index, operation) in catalog.operations.iter().enumerate() {
        let Some(operation) = operation else {
            continue;
        };
        if operation.token.incarnation != catalog.incarnation
            || operation.token.sequence <= catalog.completed_floor
            || operation.token.sequence > catalog.issued_sequence
            || catalog.operations[..index]
                .iter()
                .flatten()
                .any(|earlier| earlier.token.sequence == operation.token.sequence)
            || validate_command(&operation.command).is_err()
        {
            return Err(Fault::InvalidCatalog);
        }
        match (operation.state, operation.command) {
            (OperationState::Reserved, _) => {}
            (OperationState::AwaitingKey, Command::Create(_)) => {
                if catalog
                    .identities()
                    .any(|identity| identity.id.sequence == operation.token.sequence)
                {
                    return Err(Fault::InvalidCatalog);
                }
            }
            (OperationState::Transitioning, Command::Transition(prepared)) => {
                if !catalog.identities().any(|identity| {
                    identity.id == prepared.identity.id
                        && identity
                            .pending
                            .is_some_and(|pending| pending.operation == operation.token)
                }) {
                    return Err(Fault::InvalidCatalog);
                }
            }
            (OperationState::Completed(result), command) => {
                validate_result(catalog, operation.token, command, result)?;
            }
            _ => return Err(Fault::InvalidCatalog),
        }
        if let Some(reference) = operation.unpublished_key {
            if reference
                != key_reference(LocalIdentityId {
                    incarnation: operation.token.incarnation,
                    sequence: operation.token.sequence,
                })
                || !matches!(
                    (operation.command, operation.state),
                    (
                        Command::Create(CreateRequest {
                            source: KeySource::Import { .. },
                            ..
                        }),
                        OperationState::Completed(OperationResult::Existing { .. })
                    )
                )
            {
                return Err(Fault::InvalidCatalog);
            }
        }
    }
    let creation_reservations = catalog
        .operations()
        .filter(|operation| operation.state == OperationState::AwaitingKey)
        .count();
    if catalog
        .identities()
        .count()
        .saturating_add(creation_reservations)
        > I
    {
        return Err(Fault::InvalidCatalog);
    }
    for (index, identity) in catalog.identities.iter().enumerate() {
        let Some(identity) = identity else {
            continue;
        };
        if identity.id.incarnation != catalog.incarnation
            || identity.id.sequence == 0
            || identity.id.sequence > catalog.issued_sequence
            || identity.key_reference != key_reference(identity.id)
            || identity.lifecycle_revision == 0
            || identity.metadata_revision == 0
            || identity.lifecycle_revision > catalog.revision
            || identity.metadata_revision > catalog.revision
            || validate_metadata(identity.metadata).is_err()
            || validate_services(&identity.services).is_err()
            || validate_services(&identity.initial_services).is_err()
            || (identity.lifecycle_revision == 1 && identity.services != identity.initial_services)
            || catalog.identities[..index].iter().flatten().any(|earlier| {
                earlier.id == identity.id
                    || earlier.public_hash == identity.public_hash
                    || earlier.key_reference == identity.key_reference
            })
        {
            return Err(Fault::InvalidCatalog);
        }
        if identity.id.sequence > catalog.completed_floor
            && !catalog.operations().any(|operation| {
                operation.token.sequence == identity.id.sequence
                    && operation.state
                        == OperationState::Completed(OperationResult::Created {
                            identity: identity.id,
                        })
            })
        {
            return Err(Fault::InvalidCatalog);
        }
        if identity.recovery_cleanup.is_some()
            && (identity.desired != DesiredState::Enabled || identity.pending.is_some())
        {
            return Err(Fault::InvalidCatalog);
        }
        if let Some(pending) = identity.pending {
            if identity.desired == DesiredState::Retired {
                return Err(Fault::InvalidCatalog);
            }
            let operation = catalog
                .operations()
                .find(|operation| operation.token == pending.operation)
                .ok_or(Fault::InvalidCatalog)?;
            let Command::Transition(prepared) = operation.command else {
                return Err(Fault::InvalidCatalog);
            };
            if operation.state != OperationState::Transitioning
                || prepared.identity.id != identity.id
                || prepared.identity.public_hash != identity.public_hash
                || pending.target != prepared.target
                || prepared.identity.lifecycle_revision.checked_add(1)
                    != Some(identity.lifecycle_revision)
                || validate_report(&prepared.review, &identity.services).is_err()
                || validate_service_target(prepared.target, &identity.services).is_err()
            {
                return Err(Fault::InvalidCatalog);
            }
            match (pending.target, pending.phase, identity.desired) {
                (TransitionTarget::Enable, TransitionPhase::Activating, DesiredState::Enabled)
                | (
                    TransitionTarget::Enable,
                    TransitionPhase::ActivationCleanup(_),
                    DesiredState::Paused,
                )
                | (
                    TransitionTarget::Pause
                    | TransitionTarget::Retire
                    | TransitionTarget::AddService(_)
                    | TransitionTarget::RemoveService(_),
                    TransitionPhase::Quiescing,
                    DesiredState::Paused,
                )
                | (
                    TransitionTarget::AddService(_),
                    TransitionPhase::InitializingService,
                    DesiredState::Paused,
                ) => {}
                _ => return Err(Fault::InvalidCatalog),
            }
        }
    }
    if check_stop_capacity(catalog).is_err() {
        return Err(Fault::InvalidCatalog);
    }
    Ok(())
}

fn validate_result<const I: usize, const O: usize, const S: usize>(
    catalog: &Catalog<I, O, S>,
    token: OperationToken,
    command: Command<S>,
    result: OperationResult,
) -> Result<(), Fault> {
    if result == OperationResult::Cancelled {
        return Ok(());
    }
    let id = match result {
        OperationResult::Created { identity }
        | OperationResult::Existing { identity }
        | OperationResult::Enabled { identity }
        | OperationResult::Paused { identity }
        | OperationResult::Retired { identity }
        | OperationResult::RetirementBlocked { identity, .. }
        | OperationResult::ActivationFailed { identity, .. }
        | OperationResult::ServiceAdded { identity, .. }
        | OperationResult::ServiceRemoved { identity, .. }
        | OperationResult::ServiceRemovalBlocked { identity, .. } => identity,
        OperationResult::Cancelled => return Ok(()),
    };
    let identity = catalog
        .identities()
        .find(|identity| identity.id == id)
        .ok_or(Fault::InvalidCatalog)?;
    let valid = match (command, result) {
        (Command::Create(request), OperationResult::Created { .. }) => {
            id.sequence == token.sequence
                && request.services == identity.initial_services
                && match request.source {
                    KeySource::Generate => true,
                    KeySource::Import { expected_hash } => identity.public_hash == expected_hash,
                }
        }
        (
            Command::Create(CreateRequest {
                source: KeySource::Import { expected_hash },
                ..
            }),
            OperationResult::Existing { .. },
        ) => identity.public_hash == expected_hash,
        (Command::Transition(prepared), result) => {
            prepared.identity.id == id
                && prepared.identity.public_hash == identity.public_hash
                && prepared
                    .identity
                    .lifecycle_revision
                    .checked_add(1)
                    .is_some_and(|revision| revision <= identity.lifecycle_revision)
                && (!matches!(result, OperationResult::Retired { .. })
                    || (identity.desired == DesiredState::Retired
                        && identity.pending.is_none()
                        && identity.recovery_cleanup.is_none()
                        && validate_report(&prepared.review, &identity.services).is_ok()))
                && match (prepared.target, result) {
                    (
                        TransitionTarget::Enable,
                        OperationResult::Enabled { .. } | OperationResult::ActivationFailed { .. },
                    )
                    | (TransitionTarget::Pause, OperationResult::Paused { .. })
                    | (
                        TransitionTarget::Retire,
                        OperationResult::Retired { .. } | OperationResult::RetirementBlocked { .. },
                    ) => {
                        prepared.identity.lifecycle_revision.checked_add(1)
                            != Some(identity.lifecycle_revision)
                            || validate_report(&prepared.review, &identity.services).is_ok()
                    }
                    (
                        TransitionTarget::AddService(expected),
                        OperationResult::ServiceAdded { service, .. },
                    )
                    | (
                        TransitionTarget::RemoveService(expected),
                        OperationResult::ServiceRemovalBlocked { service, .. },
                    ) => {
                        expected == service
                            && (prepared.identity.lifecycle_revision.checked_add(1)
                                != Some(identity.lifecycle_revision)
                                || (identity.desired == DesiredState::Paused
                                    && identity.pending.is_none()
                                    && service_configuration::result_matches_set(
                                        &prepared,
                                        result,
                                        &identity.services,
                                    )))
                    }
                    (
                        TransitionTarget::RemoveService(expected),
                        OperationResult::ServiceRemoved { service, .. },
                    ) => {
                        expected == service
                            && (prepared.identity.lifecycle_revision.checked_add(1)
                                != Some(identity.lifecycle_revision)
                                || (identity.desired == DesiredState::Paused
                                    && identity.pending.is_none()
                                    && service_configuration::result_matches_set(
                                        &prepared,
                                        result,
                                        &identity.services,
                                    )))
                    }
                    _ => false,
                }
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Fault::InvalidCatalog)
    }
}
