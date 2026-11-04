use super::*;

impl<const I: usize, const S: usize> owned::Driver<S> for Local<I> {
    fn poll_recovery(
        &mut self,
        context: &mut Context<'_>,
        admission: &mut dyn owned::RecoveryAdmission,
    ) {
        self.poll_recovery(context, admission);
    }
    fn poll_observations(&mut self, context: &mut Context<'_>) {
        self.poll_observations(context);
    }
    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<S>,
    ) -> Result<runtime::ActivationPlan<S>, Error> {
        let mut services = [None; S];
        for (slot, service) in services.iter_mut().zip(identity.services.iter()) {
            if service != SERVICE {
                return Err(Error::UnsupportedService(service));
            }
            *slot = Some(runtime::ServicePlan {
                service,
                resources: runtime::Resources::ZERO,
                requires_receive_state: false,
                capabilities: runtime::Capabilities {
                    scoped_teardown: true,
                    admission_gate: true,
                    durable_drain: true,
                    durable_receive_state: false,
                },
            });
        }
        let plan = runtime::ActivationPlan {
            identity_resources: runtime::Resources {
                held_keys: 1,
                ..runtime::Resources::ZERO
            },
            services,
        };
        plan.total()?;
        Ok(plan)
    }
    fn dependencies(
        &mut self,
        identity: core::IdentityReference,
        services: core::ServiceSet<S>,
    ) -> Result<core::DependencyReport<S>, Error> {
        let mut report = core::DependencyReport::empty();
        for (slot, service) in report.entries.iter_mut().zip(services.iter()) {
            if service != SERVICE {
                return Err(Error::UnsupportedService(service));
            }
            *slot = Some(self.dependency(identity)?);
        }
        Ok(report)
    }
    fn bind_key(&mut self, _: runtime::OwnerToken, _: owned::KeyLease) {}
    fn classify_failure(&self, error: &Error) -> core::ServiceFailure {
        error.service_failure()
    }
}
impl<const I: usize> runtime::configuration::ServiceInitializer for Local<I> {
    type Error = Error;
    fn initialize_service(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
        _: core::KeyReference,
        service: core::ServiceId,
    ) -> Result<core::InitializedService, Error> {
        if service != SERVICE {
            return Err(Error::UnsupportedService(service));
        }
        self.initialize(ticket, identity)
    }
}
impl<const I: usize, const S: usize> runtime::ScopedDriver<S> for Local<I> {
    type Error = Error;
    fn preflight(
        &mut self,
        scope: runtime::Scope,
        plan: &runtime::ActivationPlan<S>,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::Resources>>, Error> {
        if plan.services.iter().any(Option::is_some) {
            return Err(Error::NetworkUnavailable);
        }
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: plan.total()?,
        }))
    }
    fn prepare_service(
        &mut self,
        _: runtime::Scope,
        _: core::KeyReference,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::PreparedService>>, Error> {
        if plan.service != SERVICE {
            return Err(Error::UnsupportedService(plan.service));
        }
        Err(Error::NetworkUnavailable)
    }
    fn activate_service(
        &mut self,
        _: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<()>>, Error> {
        if plan.service != SERVICE {
            return Err(Error::UnsupportedService(plan.service));
        }
        Err(Error::NetworkUnavailable)
    }
    fn fence_previous_workers(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<bool>>, Error> {
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: true,
        }))
    }
    fn quiesce_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, Error> {
        if plan.service != SERVICE {
            return Err(Error::UnsupportedService(plan.service));
        }
        self.quiesce(scope)
    }
    fn release_identity(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::ReleasedIdentity<S>>>, Error> {
        let dependency = match self.released_dependency(scope) {
            Ok(value) => value,
            Err(Error::Observing) => return Ok(runtime::Progress::Pending),
            Err(error) => return Err(error),
        };
        let mut report = core::DependencyReport::empty();
        if let Some(value) = dependency {
            *report.entries.first_mut().ok_or(Error::Capacity)? = Some(value);
        }
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: runtime::ReleasedIdentity {
                owned_reservations_released: true,
                shared_owners_preserved: true,
                report,
            },
        }))
    }
}
impl<const I: usize, const S: usize> owned::CommandHandler<S, Command> for Local<I> {
    fn command(&mut self, job: Command, admission: &mut dyn owned::Admission) {
        self.command(job, admission);
    }
    fn reject(job: Command) {
        Self::reject(job);
    }
}
