use super::*;

impl<const I: usize, const O: usize, const S: usize> Coordinator<'_, I, O, S> {
    /// Acknowledge a configuration operation's existing-service drain. Addition
    /// retains a separate initialization phase; removal drops its entry only
    /// after the removed service's reviewed obligations are authoritatively clear.
    pub fn ack_configuration_quiesced<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        ticket: EffectTicket,
        report: DependencyReport<S>,
    ) -> Result<OperationState, Error<T::Error>> {
        self.check_session(ticket)?;
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        let operation = *self.operation(token)?;
        let Command::Transition(prepared) = operation.command else {
            return Err(Fault::StaleEffect.into());
        };
        if !matches!(
            prepared.target,
            TransitionTarget::AddService(_) | TransitionTarget::RemoveService(_)
        ) {
            return Err(Fault::StaleEffect.into());
        }
        if let OperationState::Completed(result) = operation.state {
            if !matches!(
                result,
                OperationResult::ServiceAdded { .. }
                    | OperationResult::ServiceRemoved { .. }
                    | OperationResult::ServiceRemovalBlocked { .. }
            ) {
                return Err(Fault::StaleEffect.into());
            }
            self.check_completed_ticket(ticket, EffectKind::Quiesce)?;
            return Ok(operation.state);
        }
        let index = self.identity_index(ticket.identity)?;
        let identity = *self.identity(ticket.identity)?;
        let pending = identity.pending.ok_or(Fault::StaleEffect)?;
        if pending.operation == token && pending.phase == TransitionPhase::InitializingService {
            self.check_completed_ticket(ticket, EffectKind::Quiesce)?;
            return Ok(OperationState::Transitioning);
        }
        self.check_effect(ticket)?;
        if ticket.kind != EffectKind::Quiesce || pending.phase != TransitionPhase::Quiescing {
            return Err(Fault::StaleEffect.into());
        }
        validate_report(&report, &identity.services)?;
        if report
            .entries
            .iter()
            .flatten()
            .any(|entry| !entry.quiescence_supported)
        {
            return Err(Fault::UnqualifiedService.into());
        }
        let operation_index = self.operation_index(token)?;
        self.stage();
        let state = match prepared.target {
            TransitionTarget::AddService(_) => {
                self.staging.identities[index]
                    .as_mut()
                    .ok_or(Fault::UnknownIdentity)?
                    .pending
                    .as_mut()
                    .ok_or(Fault::StaleEffect)?
                    .phase = TransitionPhase::InitializingService;
                OperationState::Transitioning
            }
            TransitionTarget::RemoveService(service) => {
                let removed = report
                    .entries
                    .iter()
                    .flatten()
                    .find(|entry| entry.service == service)
                    .copied()
                    .ok_or(Fault::IncompleteReport)?;
                let reviewed = prepared
                    .review
                    .entries
                    .iter()
                    .flatten()
                    .find(|entry| entry.service == service)
                    .ok_or(Fault::IncompleteReport)?;
                let removed_report =
                    DependencyReport::<S>::new(core::slice::from_ref(&removed), 0)?;
                let blocked = if *reviewed != removed {
                    Some(RetirementBlock::ChangedDependencies)
                } else {
                    retirement_block(&removed_report)
                };
                let record = self.staging.identities[index]
                    .as_mut()
                    .ok_or(Fault::UnknownIdentity)?;
                record.pending = None;
                let result = if let Some(reason) = blocked {
                    OperationResult::ServiceRemovalBlocked {
                        identity: identity.id,
                        service,
                        reason,
                    }
                } else {
                    let mut services = ServiceSet::empty();
                    for (index, retained) in identity
                        .services
                        .iter()
                        .filter(|id| *id != service)
                        .enumerate()
                    {
                        services.entries[index] = Some(retained);
                    }
                    record.services = services;
                    OperationResult::ServiceRemoved {
                        identity: identity.id,
                        service,
                    }
                };
                OperationState::Completed(result)
            }
            _ => return Err(Fault::StaleEffect.into()),
        };
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = state;
        self.persist_staging(storage)?;
        self.ready[index] = false;
        Ok(state)
    }

    /// Confirm durable, identity-bound service preparation. This adds only the
    /// stable service ID and leaves the identity paused. The domain must retain
    /// prepared state across reply loss and uncertain catalog acknowledgment.
    pub fn ack_service_initialized<T: Storage<I, O, S>>(
        &mut self,
        storage: &mut T,
        initialized: InitializedService,
    ) -> Result<OperationResult, Error<T::Error>> {
        let ticket = initialized.ticket;
        self.check_session(ticket)?;
        let token = ticket.operation.ok_or(Fault::StaleEffect)?;
        if let OperationState::Completed(result) = self.operation(token)?.state {
            if !matches!(result, OperationResult::ServiceAdded { service, .. } if service == initialized.service)
            {
                return Err(Fault::StaleEffect.into());
            }
            self.check_completed_ticket(ticket, EffectKind::InitializeService)?;
            return Ok(result);
        }
        self.check_effect(ticket)?;
        if ticket.kind != EffectKind::InitializeService {
            return Err(Fault::StaleEffect.into());
        }
        let index = self.identity_index(ticket.identity)?;
        let identity = self.identity(ticket.identity)?;
        let pending = identity.pending.ok_or(Fault::StaleEffect)?;
        if pending.phase != TransitionPhase::InitializingService
            || pending.target != TransitionTarget::AddService(initialized.service)
        {
            return Err(Fault::StaleEffect.into());
        }
        validate_service_target(pending.target, &identity.services)?;
        let operation_index = self.operation_index(token)?;
        self.stage();
        let record = self.staging.identities[index]
            .as_mut()
            .ok_or(Fault::UnknownIdentity)?;
        let slot = record
            .services
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(Fault::CapacityExceeded)?;
        *slot = Some(initialized.service);
        record.pending = None;
        let result = OperationResult::ServiceAdded {
            identity: ticket.identity,
            service: initialized.service,
        };
        self.staging.operations[operation_index]
            .as_mut()
            .ok_or(Fault::UnknownOperation)?
            .state = OperationState::Completed(result);
        self.persist_staging(storage)?;
        self.ready[index] = false;
        Ok(result)
    }
}

/// The current set at this exact lifecycle revision must be the complete
/// reviewed-set delta; preserving only the changed ID would miss lost neighbors.
pub(super) fn result_matches_set<const S: usize>(
    prepared: &PreparedTransition<S>,
    result: OperationResult,
    current: &ServiceSet<S>,
) -> bool {
    let old_count = prepared.review.entries.iter().flatten().count();
    let current_count = current.iter().count();
    match result {
        OperationResult::ServiceAdded { service, .. } => {
            old_count.checked_add(1) == Some(current_count)
                && current.iter().any(|id| id == service)
                && prepared
                    .review
                    .entries
                    .iter()
                    .flatten()
                    .all(|entry| current.iter().any(|id| id == entry.service))
        }
        OperationResult::ServiceRemoved { service, .. } => {
            old_count.checked_sub(1) == Some(current_count)
                && !current.iter().any(|id| id == service)
                && prepared
                    .review
                    .entries
                    .iter()
                    .flatten()
                    .filter(|entry| entry.service != service)
                    .all(|entry| current.iter().any(|id| id == entry.service))
        }
        OperationResult::ServiceRemovalBlocked { .. } => {
            validate_report(&prepared.review, current).is_ok()
        }
        _ => false,
    }
}
