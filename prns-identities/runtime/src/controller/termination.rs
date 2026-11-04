use super::*;

impl<const I: usize, const S: usize, const L: usize> Controller<'_, I, S, L> {
    /// Irreversibly fence fresh runtime admission and retain all current owners,
    /// resources and accepted mutations. Repeated calls return the same token.
    ///
    /// This is process-resource cleanup, not Pause/Retire or a catalog commit.
    /// A disjoint termination scope invalidates ordinary callbacks without
    /// requiring another generation value when a counter has been exhausted.
    pub fn begin_termination(&mut self) -> TerminationToken {
        if let Some(token) = self.state.termination {
            return token;
        }
        let token = TerminationToken {
            incarnation: self.state.incarnation,
        };
        self.state.termination = Some(token);
        for owner in self
            .state
            .owners
            .iter_mut()
            .flatten()
            .chain(self.state.cleanup.iter_mut())
        {
            owner.phase = Phase::FencingPreviousWorkers;
            owner.report = None;
            for service in owner.services.iter_mut().flatten() {
                service.phase = ServicePhase::Declared;
                service.dependencies = None;
            }
        }
        token
    }

    /// Validate a queued driver step at its actual dispatch boundary, under the
    /// same exclusive gate as controller changes. Ordinary scopes require fresh
    /// catalog authority; termination can only drain this runtime's exact owner,
    /// generation, service and step, even if the catalog is unavailable.
    pub fn validate_scope<A: Authority>(&self, authority: &A, scope: Scope) -> Result<(), Fault> {
        let current = self.scope(authority, scope.owner, scope.service, scope.step)?;
        if scope != current {
            return Err(Fault::StaleEvidence);
        }
        let owner = self.get(scope.owner)?;
        let correct_phase = match scope.step {
            Step::FencePreviousWorkers => {
                scope.service.is_none()
                    && matches!(
                        owner.phase,
                        Phase::FencingActivation | Phase::FencingPreviousWorkers
                    )
            }
            Step::Preflight => scope.service.is_none() && owner.phase == Phase::Preflighting,
            Step::PrepareService | Step::ActivateService | Step::QuiesceService => {
                let service = scope.service.ok_or(Fault::UnknownService)?;
                let state =
                    owner.services[service_index(owner, service)?].ok_or(Fault::UnknownService)?;
                match scope.step {
                    Step::PrepareService => {
                        owner.phase == Phase::Activating && state.phase == ServicePhase::Declared
                    }
                    Step::ActivateService => {
                        owner.phase == Phase::Activating && state.phase == ServicePhase::Prepared
                    }
                    Step::QuiesceService => {
                        owner.phase == Phase::Quiescing && state.phase != ServicePhase::Quiesced
                    }
                    _ => false,
                }
            }
            Step::ReleaseIdentity => {
                scope.service.is_none()
                    && owner.phase == Phase::Quiescing
                    && owner
                        .services
                        .iter()
                        .flatten()
                        .all(|service| service.phase == ServicePhase::Quiesced)
            }
        };
        if !correct_phase {
            return Err(Fault::WrongPhase);
        }
        if matches!(
            scope.step,
            Step::Preflight | Step::QuiesceService | Step::ReleaseIdentity
        ) {
            self.drained(scope.owner)?;
        }
        Ok(())
    }

    /// Forget only the exact owner whose termination drain has completed.
    /// No catalog acknowledgment is performed or implied. A durable operation
    /// that was pending before termination remains available for restart recovery.
    pub fn confirm_termination(
        &mut self,
        termination: TerminationToken,
        receipt: QuiescenceReceipt<S>,
    ) -> Result<(), Fault> {
        self.check_termination(termination)?;
        if receipt.termination != Some(termination)
            || self.quiescence_receipt(receipt.owner)? != receipt
        {
            return Err(Fault::StaleEvidence);
        }
        self.drained(receipt.owner)?;
        let index = self.index(receipt.owner)?;
        if index == I {
            self.state.cleanup = None;
        } else {
            self.state.owners[index] = None;
        }
        Ok(())
    }

    /// True only after every retained runtime owner and mutation has drained.
    /// Native embeddings must additionally settle any other owned pending effects
    /// before releasing the catalog lock or reporting physical owner completion.
    pub fn termination_complete(&self, termination: TerminationToken) -> Result<bool, Fault> {
        self.check_termination(termination)?;
        Ok(self.state.owned_identities() == 0
            && self.state.leases.iter().all(Option::is_none)
            && self.state.used == Resources::ZERO)
    }

    fn check_termination(&self, token: TerminationToken) -> Result<(), Fault> {
        if self.state.termination == Some(token) {
            Ok(())
        } else {
            Err(Fault::StaleEvidence)
        }
    }

    pub(super) fn ensure_open(&self) -> Result<(), Fault> {
        if self.state.termination.is_some() {
            Err(Fault::Terminating)
        } else {
            Ok(())
        }
    }
}
