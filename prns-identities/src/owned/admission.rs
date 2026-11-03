//! Borrowed authority for synchronous typed domain commands on the owner lane.
use super::*;
use prns_identities_runtime::Authority;

/// A trusted domain derives this from its command and retained evidence. It is
/// not an input on any public identity/domain client or foreign API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Pure historical reads, including an archived/retired identity.
    History,
    /// Exact already-admitted durable work; the domain verifies that evidence.
    Recovery,
    /// Fresh private local changes; never eventual network work.
    LocalMutation,
    ServiceMutation {
        service: core::ServiceId,
        operation: runtime::MutationId,
    },
}

#[derive(Debug)]
pub enum AdmissionError {
    Store(crate::Error),
    Authority(core::Fault),
    Runtime(runtime::Fault),
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native identity admission: {self:?}")
    }
}
impl std::error::Error for AdmissionError {}

/// Exact owned admission, never a replacement for the domain's durable request.
/// Drop releases its native key use but DOES NOT settle its tracked mutation.
#[must_use = "an admitted mutation must be explicitly settled with actual completion evidence"]
pub struct Permit {
    guard: core::AdmissionGuard,
    mutation: Option<runtime::MutationLease>,
    _key: Option<KeyLease>,
}

/// Restricted owner progress for already-held mutation permits. It cannot
/// acquire a replacement lease or grant new network/service authority.
pub trait RecoveryAdmission {
    fn validate_existing(
        &mut self,
        identity: core::IdentityReference,
        service: core::ServiceId,
        operation: runtime::MutationId,
        permit: &Permit,
    ) -> Result<(), AdmissionError>;
    /// Keep the original permit and key use in the caller's slot on every error.
    /// Only successful canonical settlement removes and drops that permit.
    fn settle_existing(
        &mut self,
        permit: &mut Option<Permit>,
        settlement: runtime::Settlement,
    ) -> Result<(), AdmissionError>;
}

/// Available only inside a trusted typed command handler. The context borrows
/// the actual coordinator/runtime; it retains no parallel authority table.
/// Calls and storage work must finish synchronously before the handler returns.
/// No transaction or borrowed authority may cross an await.
pub trait Admission: RecoveryAdmission {
    fn acquire(
        &mut self,
        identity: core::IdentityReference,
        access: Access,
    ) -> Result<Permit, AdmissionError>;
    fn settle(
        &mut self,
        permit: Permit,
        settlement: runtime::Settlement,
    ) -> Result<(), AdmissionError>;
    /// Validate the captured generation on the exclusive owner lane at dequeue.
    fn validate_network(&mut self, permit: runtime::NetworkPermit) -> Result<(), AdmissionError>;
    /// Acquire using the captured Network guard; does not reconstruct authority.
    fn acquire_network(
        &mut self,
        permit: runtime::NetworkPermit,
        operation: runtime::MutationId,
    ) -> Result<Permit, AdmissionError>;
}

pub(super) struct Gate<'a, 'buffers, const I: usize, const O: usize, const S: usize, const L: usize>
{
    pub store: &'a IdentityStore<I, O, S>,
    pub coordinator: &'a core::Coordinator<'buffers, I, O, S>,
    pub state: &'a mut runtime::RuntimeState<I, S, L>,
    pub key: &'a dyn Fn(core::IdentityReference) -> Option<Arc<driver::Key>>,
}
impl<const I: usize, const O: usize, const S: usize, const L: usize> Admission
    for Gate<'_, '_, I, O, S, L>
{
    fn acquire(
        &mut self,
        identity: core::IdentityReference,
        access: Access,
    ) -> Result<Permit, AdmissionError> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(AdmissionError::Store)?;
        let class = match access {
            Access::History => core::AdmissionClass::History,
            Access::Recovery => core::AdmissionClass::Recovery,
            Access::LocalMutation => core::AdmissionClass::LocalMutation,
            Access::ServiceMutation { .. } => core::AdmissionClass::ServiceMutation,
        };
        let guard = self
            .coordinator
            .admit(identity.id, identity.public_hash, class)
            .map_err(AdmissionError::Authority)?;
        let authority = self.store.runtime_authority(self.coordinator);
        authority
            .validate_admission(guard)
            .map_err(AdmissionError::Runtime)?;
        let (mutation, key) = if let Access::ServiceMutation { service, operation } = access {
            let key = (self.key)(identity)
                .ok_or(AdmissionError::Runtime(runtime::Fault::StorageNotReady))?;
            let mutation = runtime::Controller::new(self.state)
                .admit_mutation(&authority, guard, service, operation)
                .map_err(AdmissionError::Runtime)?;
            (Some(mutation), Some(KeyLease::new(key)))
        } else {
            (None, None)
        };
        Ok(Permit {
            guard,
            mutation,
            _key: key,
        })
    }
    fn validate_network(&mut self, permit: runtime::NetworkPermit) -> Result<(), AdmissionError> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(AdmissionError::Store)?;
        runtime::Controller::new(self.state)
            .validate_network(&self.store.runtime_authority(self.coordinator), permit)
            .map_err(AdmissionError::Runtime)
    }
    fn acquire_network(
        &mut self,
        permit: runtime::NetworkPermit,
        operation: runtime::MutationId,
    ) -> Result<Permit, AdmissionError> {
        self.validate_network(permit)?;
        let guard = permit.admission();
        let identity = core::IdentityReference {
            id: guard.identity.id,
            public_hash: guard.identity.public_hash,
        };
        let key =
            (self.key)(identity).ok_or(AdmissionError::Runtime(runtime::Fault::StorageNotReady))?;
        let mutation = runtime::Controller::new(self.state)
            .admit_mutation(
                &self.store.runtime_authority(self.coordinator),
                guard,
                permit.service(),
                operation,
            )
            .map_err(AdmissionError::Runtime)?;
        Ok(Permit {
            guard,
            mutation: Some(mutation),
            _key: Some(KeyLease::new(key)),
        })
    }
    fn settle(
        &mut self,
        permit: Permit,
        settlement: runtime::Settlement,
    ) -> Result<(), AdmissionError> {
        if let Some(mutation) = permit.mutation {
            runtime::Controller::new(self.state)
                .complete_mutation(mutation, settlement)
                .map_err(AdmissionError::Runtime)?;
        } else {
            self.store
                .runtime_authority(self.coordinator)
                .validate_admission(permit.guard)
                .map_err(AdmissionError::Runtime)?;
        }
        Ok(())
    }
}

impl<const I: usize, const O: usize, const S: usize, const L: usize> RecoveryAdmission
    for Gate<'_, '_, I, O, S, L>
{
    fn validate_existing(
        &mut self,
        identity: core::IdentityReference,
        service: core::ServiceId,
        operation: runtime::MutationId,
        permit: &Permit,
    ) -> Result<(), AdmissionError> {
        self.store
            .validate_owner(self.coordinator)
            .map_err(AdmissionError::Store)?;
        let recovery = self
            .coordinator
            .admit(
                identity.id,
                identity.public_hash,
                core::AdmissionClass::Recovery,
            )
            .map_err(AdmissionError::Authority)?;
        let original = permit
            .mutation
            .ok_or(AdmissionError::Runtime(runtime::Fault::InvalidAdmission))?;
        let existing = runtime::Controller::new(self.state)
            .mutation_for_recovery(
                &self.store.runtime_authority(self.coordinator),
                recovery,
                original.owner(),
                service,
                operation,
            )
            .map_err(AdmissionError::Runtime)?;
        if existing != original {
            return Err(AdmissionError::Runtime(runtime::Fault::StaleLease));
        }
        Ok(())
    }
    fn settle_existing(
        &mut self,
        slot: &mut Option<Permit>,
        settlement: runtime::Settlement,
    ) -> Result<(), AdmissionError> {
        let permit = slot
            .as_ref()
            .ok_or(AdmissionError::Runtime(runtime::Fault::StaleLease))?;
        let mutation = permit
            .mutation
            .ok_or(AdmissionError::Runtime(runtime::Fault::InvalidAdmission))?;
        runtime::Controller::new(self.state)
            .complete_mutation(mutation, settlement)
            .map_err(AdmissionError::Runtime)?;
        // No fallible operation follows canonical settlement. Removing the
        // original guard releases exactly its original native key use.
        drop(slot.take());
        Ok(())
    }
}

/// A domain's existing typed job is the extension payload. The handler owns its
/// reply and derives access from that job; no generic closure executor or method
/// inventory is introduced. Each invocation is bounded and synchronous.
pub trait CommandHandler<const S: usize, J>: Driver<S> {
    fn command(&mut self, job: J, admission: &mut dyn Admission);
    /// An admitted queue job that did not start before global closure.
    fn reject(job: J);
}
impl<const S: usize, D: Driver<S>> CommandHandler<S, std::convert::Infallible> for D {
    fn command(&mut self, job: std::convert::Infallible, _: &mut dyn Admission) {
        match job {}
    }
    fn reject(job: std::convert::Infallible) {
        match job {}
    }
}
