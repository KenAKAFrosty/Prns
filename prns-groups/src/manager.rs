//! One native database owner for the enrollment catalog and every group namespace.
mod dependencies;
use crate::{
    client::{Command, Reply},
    enrollment::Enrollment,
    Error, Job, Provisioned,
};
use prns_groups_core::GroupId;
use prns_groups_store_redb::RedbStoreOwner;
use prns_host::IdentityHash;
use redb::Database;

pub struct GroupManager<'db> {
    database: &'db Database,
    owner: RedbStoreOwner<'db>,
    identity: IdentityHash,
    provisioning: Provisioning<'db>,
    retained: Option<crate::retained::Store<'db>>,
    observations: crate::observation::SourceLease<'db>,
    pending_carrier: Option<&'db mut Option<crate::attachment::PendingCarrier>>,
    profile_support: Option<&'db mut Option<crate::ProfileSupport>>,
}

enum Provisioning<'db> {
    Managed(Box<Enrollment<'db>>),
    Fixed(Box<Provisioned>),
}
impl<'db> Provisioning<'db> {
    fn managed(&self) -> Result<&Enrollment<'db>, Error> {
        match self {
            Self::Managed(enrollment) => Ok(enrollment),
            Self::Fixed(_) => Err(Error::NotProvisioned),
        }
    }
    fn fixed(&self) -> Option<&Provisioned> {
        match self {
            Self::Fixed(config) => Some(config),
            Self::Managed(_) => None,
        }
    }
    fn recovery_configuration(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
    ) -> Result<Option<Provisioned>, Error> {
        match self {
            Self::Managed(enrollment) => enrollment
                .recovery_configuration(owner, group, incarnation)
                .map_err(enrollment_error),
            Self::Fixed(config) => Ok((config.group == group && config.incarnation == incarnation)
                .then(|| config.as_ref().clone())),
        }
    }
}

impl<'db> GroupManager<'db> {
    /// Stable, read-only invitation scope for a caller's durable action journal.
    pub fn invitation_group(&self, invitation: [u8; 32]) -> Result<Option<GroupId>, Error> {
        self.provisioning
            .managed()?
            .invitation_group(invitation)
            .map_err(enrollment_error)
    }
    /// Intentionally create this domain's catalog on an exclusive database lane.
    /// Existing or partial catalog state is rejected, never replaced.
    pub fn create_managed(database: &'db Database, identity: IdentityHash) -> Result<Self, Error> {
        if crate::retained::Store::open(database, identity)?.is_some() {
            return Err(Error::Configuration);
        }
        Enrollment::create_new(database, identity).map_err(enrollment_error)?;
        Self::open_managed(database, identity)
    }

    /// Attach an existing managed catalog after validating its immutable identity
    /// and format. This performs no initialization or writes; group transitions
    /// resume through their ordinary recovery operations after attachment.
    pub fn open_managed(database: &'db Database, identity: IdentityHash) -> Result<Self, Error> {
        crate::archive::validate_open(database, identity)?;
        let observations = crate::observation::Source::default();
        let enrollment = Enrollment::open_existing(database, identity)
            .map_err(enrollment_error)?
            .with_change_witness(observations.witness());
        Ok(Self {
            database,
            owner: RedbStoreOwner::new(database).with_change_witness(observations.witness()),
            identity,
            provisioning: Provisioning::Managed(Box::new(enrollment)),
            retained: crate::retained::Store::open(database, identity)?
                .map(|store| store.with_change_witness(observations.witness())),
            observations: crate::observation::SourceLease::Owned(observations),
            pending_carrier: None,
            profile_support: None,
        })
    }
    /// Retain bounded validation evidence on an exclusive standalone lane.
    /// The existing constructor still performs strict cold validation. Reserved
    /// namespace tables must only be changed through this owner; explicit full
    /// inspection remains strict even when evidence is warm. The caller keeps
    /// this non-clone cache alive and drops it before reporting physical close.
    pub fn with_validation_cache(
        mut self,
        cache: &'db mut prns_groups_store_redb::ValidationCache,
    ) -> Result<Self, Error> {
        if !cache.matches_database(self.database) {
            return Err(Error::Configuration);
        }
        let witness = self.owner.change_witness().cloned();
        let stamp = self.owner.change_stamp();
        self.owner = cache.owner();
        if let Some(witness) = witness {
            self.owner = self.owner.with_change_witness(witness);
        }
        if let Some(stamp) = stamp {
            self.owner = self.owner.with_change_stamp(stamp);
        }
        Ok(self)
    }

    /// Reconstruct a sequential view while preserving the attachment's
    /// subscriptions. A changed/missing store incarnation never silently
    /// retargets the attachment, even when the public identity is unchanged.
    pub fn open_attached(
        database: &'db Database,
        attachment: &'db mut crate::Attachment,
    ) -> Result<Self, Error> {
        attachment.require_database(database)?;
        let stamp = prns_groups_store_redb::change::read(database)?.ok_or(Error::Configuration)?;
        if stamp.incarnation != attachment.incarnation {
            return Err(Error::Configuration);
        }
        let identity = attachment.identity;
        crate::archive::selected(database, identity)?;
        let witness = attachment.observations.witness();
        let enrollment = Enrollment::open_attached(database, identity, attachment.catalog_limits)
            .map_err(enrollment_error)?
            .with_profile_support(attachment.profile_support)
            .with_change_stamp(attachment.incarnation)
            .with_change_witness(witness.clone());
        let retained =
            crate::retained::Store::open_attached(database, identity, attachment.retained)?.map(
                |store| {
                    store
                        .with_change_witness(witness.clone())
                        .with_change_stamp(attachment.incarnation)
                },
            );
        let owner = match attachment.validation.as_mut() {
            Some(cache) => cache.owner(),
            None => RedbStoreOwner::new(database),
        }
        .with_change_witness(witness)
        .with_change_stamp(attachment.incarnation);
        Ok(Self {
            database,
            owner,
            identity,
            provisioning: Provisioning::Managed(Box::new(enrollment)),
            retained,
            observations: crate::observation::SourceLease::Borrowed(&mut attachment.observations),
            pending_carrier: Some(&mut attachment.carrier),
            profile_support: Some(&mut attachment.profile_support),
        })
    }

    /// Attach a completely installed fixed membership, without an enrollment
    /// catalog or management authority. Both modes execute the same record engine.
    pub fn open_fixed(database: &'db Database, provisioned: Provisioned) -> Result<Self, Error> {
        if provisioned.owner_identity.is_some() {
            return Err(Error::Configuration);
        }
        crate::StoreSession::open_on_manager_lane(database, provisioned.clone())?;
        let observations = crate::observation::Source::default();
        Ok(Self {
            database,
            owner: RedbStoreOwner::new(database).with_change_witness(observations.witness()),
            identity: provisioned.local_reticulum_identity,
            retained: crate::retained::Store::open(database, provisioned.local_reticulum_identity)?
                .map(|store| store.with_change_witness(observations.witness())),
            observations: crate::observation::SourceLease::Owned(observations),
            pending_carrier: None,
            profile_support: None,
            provisioning: Provisioning::Fixed(Box::new(provisioned)),
        })
    }
    /// Unmanaged direct lane. Retained commands require explicit authorization;
    /// this entry point never silently substitutes an unmanaged gate for them.
    pub fn execute(&mut self, job: Job) {
        let result = self.dispatch(job.group, job.control, job.command);
        self.observations.publish();
        Job::notify_commit(&job.wake, &result);
        let _ = job.reply.send(result);
    }
    /// The trusted assembly supplies its real admission gate and the opaque
    /// existing Job. Only this domain classifies commands; no caller-supplied
    /// access class or mirrored command inventory crosses the owner boundary.
    pub fn execute_authorized<A: crate::admission::Authorization>(
        &mut self,
        job: Job,
        authority: &mut A,
    ) {
        let result = self.dispatch_authorized(job.group, job.control, job.command, authority);
        self.observations.publish();
        Job::notify_commit(&job.wake, &result);
        let _ = job.reply.send(result);
    }
    fn dispatch_authorized<A: crate::admission::Authorization>(
        &mut self,
        scope: Option<GroupId>,
        control: Option<prns_groups_core::ControlReference>,
        command: Command,
        authority: &mut A,
    ) -> Result<Reply, Error> {
        use crate::admission::{Access, Settlement};
        self.check_carrier_fence(scope, &command)?;
        match command {
            command @ (Command::EnrollmentHandle { .. } | Command::EnrollmentComplete { .. }) => {
                let enrollment = self.provisioning.managed()?;
                let pending = self
                    .pending_carrier
                    .as_deref_mut()
                    .ok_or(Error::Configuration)?;
                enrollment.execute_carrier(&mut self.owner, command, pending, authority)
            }
            Command::Receive { wire, options } => {
                let group = scope.ok_or(Error::Configuration)?;
                let cfg = self.provisioned(group)?;
                if control != Some(cfg.control) || cfg.lifecycle != crate::GroupLifecycle::Active {
                    return Err(Error::Configuration);
                }
                let pending = self
                    .pending_carrier
                    .as_deref_mut()
                    .ok_or(Error::Configuration)?;
                crate::session::receive_authorized_with_owner(
                    self.database,
                    &mut self.owner,
                    &cfg,
                    (&wire, options),
                    pending,
                    authority,
                )
            }
            Command::SubmitRetained {
                id,
                intent,
                context,
            } => self
                .submit_retained(id, intent, context, authority)
                .map(|operation| Reply::Operation(Some(operation))),
            Command::DismissRetained(id) => {
                self.dismiss_retained(id, authority).map(|()| Reply::Done)
            }
            Command::Execute { request, context } => self
                .execute_retained_request(request, context, authority)
                .map(Reply::Outcome),
            command => {
                let access = match &command {
                    Command::Subscribe
                    | Command::ProfileSupport(_)
                    | Command::Identity
                    | Command::GroupScope(_)
                    | Command::RetainedOperation(_)
                    | Command::RetainedOperations { .. }
                    | Command::Conversations { .. }
                    | Command::Invitation { .. }
                    | Command::Invitations { .. } => Access::History,
                    Command::Conversation { .. }
                    | Command::Members { .. }
                    | Command::Records { .. }
                    | Command::Record { .. }
                    | Command::Progress { .. }
                    | Command::DeliveryTargets { .. }
                    | Command::Changes { .. } => {
                        // Fixed fixture setup may finish already committed MLS
                        // output cleanup. Managed record/history reads are pure.
                        if self.provisioning.fixed().is_some() {
                            Access::Recovery
                        } else {
                            Access::History
                        }
                    }
                    #[cfg(feature = "text")]
                    Command::Messages { .. } => {
                        if self.provisioning.fixed().is_some() {
                            Access::Recovery
                        } else {
                            Access::History
                        }
                    }
                    // These may finish a catalog-backed accepted Create, but
                    // never admit a new user or peer operation.
                    Command::Config
                    | Command::EnrollmentWork { .. }
                    | Command::Maintain(_)
                    | Command::RepairReceipts
                    | Command::Receipt(_)
                    | Command::Known { .. } => Access::Recovery,
                    Command::LoadCursor(_)
                    | Command::Snapshot
                    | Command::PendingDelivery { .. }
                    | Command::Inventory { .. }
                    | Command::Envelope(_) => {
                        // Carrier reads require the exact selected group;
                        // managed identities have no implicit singleton.
                        if scope.is_none() {
                            return Err(Error::Configuration);
                        }
                        Access::History
                    }
                    Command::SaveCursor(..) => Access::LocalMutation,
                    Command::EnrollmentHandle { .. }
                    | Command::EnrollmentComplete { .. }
                    | Command::Receive { .. }
                    | Command::Execute { .. }
                    | Command::SubmitRetained { .. }
                    | Command::DismissRetained(_) => return Err(Error::Corrupt),
                };
                let permit = authority.acquire(access)?;
                let result = match command {
                    Command::RetainedOperation(id) => {
                        self.retained_operation(id).map(Reply::Operation)
                    }
                    Command::RetainedOperations { after, limit } => self
                        .retained_operations(after, limit)
                        .map(Reply::Operations),
                    command => self.dispatch(scope, control, command),
                };
                // This handler did not admit fresh domain work. Recovery writes
                // belong to their pre-existing durable domain evidence.
                authority.settle(permit, Settlement::NotDispatched)?;
                result
            }
        }
    }
    fn carrier_pending(&self) -> bool {
        self.pending_carrier
            .as_ref()
            .is_some_and(|pending| pending.is_some())
    }
    fn reject_pending_carrier(&self) -> Result<(), Error> {
        if self.carrier_pending() {
            Err(Error::Busy)
        } else {
            Ok(())
        }
    }
    fn check_carrier_fence(&self, scope: Option<GroupId>, command: &Command) -> Result<(), Error> {
        if !self.carrier_pending() {
            return Ok(());
        }
        let pure = match command {
            Command::Subscribe
            | Command::Identity
            | Command::GroupScope(_)
            | Command::RetainedOperation(_)
            | Command::RetainedOperations { .. }
            | Command::Conversations { .. }
            | Command::Conversation { .. }
            | Command::Members { .. }
            | Command::Invitation { .. }
            | Command::Invitations { .. }
            | Command::Records { .. }
            | Command::Record { .. }
            | Command::Progress { .. }
            | Command::DeliveryTargets { .. }
            | Command::Changes { .. } => true,
            #[cfg(feature = "text")]
            Command::Messages { .. } => true,
            Command::LoadCursor(_)
            | Command::Snapshot
            | Command::PendingDelivery { .. }
            | Command::Inventory { .. }
            | Command::Envelope(_) => scope.is_some(),
            _ => false,
        };
        if pure {
            Ok(())
        } else {
            Err(Error::Busy)
        }
    }
    /// Read the original scope without maintenance or new mutation admission.
    pub fn group_scope(&mut self, group: GroupId) -> Result<crate::OriginalScope, Error> {
        if let Some(fixed) = self.provisioning.fixed() {
            if fixed.group != group {
                return Err(Error::NotProvisioned);
            }
            return Ok(crate::OriginalScope::Control {
                control: fixed.control,
                incarnation: fixed.incarnation,
            });
        }
        self.provisioning
            .managed()?
            .group_scope(&mut self.owner, group)
            .map_err(enrollment_error)
    }
    /// One synchronous domain execution path on the existing owner lane. A
    /// retained facility calls this directly, never re-enters its async Client.
    pub fn execute_request(
        &mut self,
        request: crate::Request,
        context: crate::ExecutionContext,
    ) -> Result<crate::Outcome, Error> {
        let result = self.execute_request_inner(request, context);
        self.observations.publish();
        result
    }
    fn execute_request_inner(
        &mut self,
        request: crate::Request,
        context: crate::ExecutionContext,
    ) -> Result<crate::Outcome, Error> {
        self.reject_pending_carrier()?;
        if let Some(outcome) = self.recover_direct(&request)? {
            return Ok(outcome);
        }
        if let Some(store) = self.retained.clone() {
            store.check(&request, self.operation_scope(&request.intent)?)?;
            store.validate_new(request.id, context)?;
        }
        self.execute_domain(request, context)
    }
    /// Managed direct calls share durable native ownership with submission.
    /// Only the convenience submission path captures scope: the exact Request
    /// supplied here is never rebound to a newer membership.
    fn execute_retained_request<A: crate::admission::Authorization>(
        &mut self,
        request: crate::Request,
        context: crate::ExecutionContext,
        authority: &mut A,
    ) -> Result<crate::Outcome, Error> {
        use crate::{
            admission::{Access, Settlement},
            retained::State,
        };
        request.validate()?;
        let permit = authority.acquire(Access::Recovery)?;
        let recovered = (|| {
            if let Some(store) = self.retained.clone() {
                if let Some(operation) = store.operation(request.id)? {
                    if operation.request != request {
                        return Err(Error::IdempotencyConflict);
                    }
                    // The coordinator owns recovery and possible re-admission
                    // of an existing unresolved row. Do not bypass its state.
                    return Ok(None);
                }
            }
            // Old exact results remain recoverable without a fresh token or a
            // selected retainer. Absence alone never admits domain execution.
            self.recover_request(request.clone())
        })();
        authority.settle(permit, Settlement::NotDispatched)?;
        if let Some(outcome) = recovered? {
            return Ok(outcome);
        }
        match self
            .submit_retained_request(request, context, authority)?
            .state
        {
            State::Completed(outcome) => Ok(outcome),
            State::Rejected(reason) => {
                Err(Error::Api(prns_groups_core::ApiError::Rejected { reason }))
            }
            State::Uncertain(error) => Err(Error::Api(error)),
            State::Pending | State::Dispatched => Err(Error::Busy),
        }
    }
    fn recover_direct(
        &mut self,
        request: &crate::Request,
    ) -> Result<Option<crate::Outcome>, Error> {
        request.validate()?;
        if let Some(store) = self.retained.clone() {
            if let Some(existing) = store.operation(request.id)? {
                if existing.request != *request {
                    return Err(Error::IdempotencyConflict);
                }
                match existing.state {
                    crate::retained::State::Completed(outcome) => return Ok(Some(outcome)),
                    crate::retained::State::Rejected(reason) => {
                        return Err(Error::Api(prns_groups_core::ApiError::Rejected { reason }))
                    }
                    _ => (),
                }
                if let Some(outcome) = self.recover_request(request.clone())? {
                    return Ok(Some(outcome));
                }
                return Err(Error::Busy);
            }
        }
        self.recover_request(request.clone())
    }
    fn execute_domain(
        &mut self,
        request: crate::Request,
        context: crate::ExecutionContext,
    ) -> Result<crate::Outcome, Error> {
        let crate::Intent::Publish(intent) = &request.intent else {
            return self
                .provisioning
                .managed()?
                .execute_request(&mut self.owner, request, context)
                .map_err(enrollment_error);
        };
        let cfg = self.provisioned(intent.group)?;
        match crate::session::execute_with_owner(
            self.database,
            &mut self.owner,
            &cfg,
            Command::Execute { request, context },
        )? {
            Reply::Outcome(outcome) => Ok(outcome),
            _ => Err(Error::Corrupt),
        }
    }
    /// Exact domain recovery only. None grants no permission to execute fresh
    /// work; a retained coordinator must obtain mutation admission separately.
    pub(crate) fn recover_request(
        &mut self,
        request: crate::Request,
    ) -> Result<Option<crate::Outcome>, Error> {
        request.validate()?;
        let crate::Intent::Publish(intent) = &request.intent else {
            return self
                .provisioning
                .managed()?
                .recover_request(&mut self.owner, request)
                .map_err(enrollment_error);
        };
        let incarnation = request
            .original_scope
            .ok_or(Error::InvalidInput)?
            .incarnation();
        let Some(cfg) =
            self.provisioning
                .recovery_configuration(&mut self.owner, intent.group, incarnation)?
        else {
            return Ok(None);
        };
        crate::session::retained_publication_with_owner(
            self.database,
            &mut self.owner,
            &cfg,
            &request,
        )
    }
    fn operation_scope(
        &self,
        intent: &crate::Intent,
    ) -> Result<crate::retained::OperationScope, Error> {
        use crate::{retained::OperationScope, Intent};
        if let Some(group) = intent.group() {
            return Ok(OperationScope::Group(group));
        }
        let invitation = match intent {
            Intent::Create { .. } => return Ok(OperationScope::Create),
            Intent::Respond { invitation, .. }
            | Intent::CancelInvitation { invitation }
            | Intent::DismissInvitation { invitation } => *invitation,
            _ => return Err(Error::InvalidInput),
        };
        self.invitation_group(invitation.0)?
            .map(OperationScope::Group)
            .ok_or(Error::Enrollment(
                crate::enrollment::EnrollmentError::Missing,
            ))
    }
    /// None means no retained record, never proof that an effect did not happen.
    pub fn retained_operation(
        &self,
        id: crate::RequestId,
    ) -> Result<Option<crate::retained::Operation>, Error> {
        self.retained
            .clone()
            .ok_or(Error::Configuration)?
            .operation(id)
    }
    pub fn retained_operations(
        &self,
        after: Option<crate::RequestId>,
        limit: crate::PageLimits,
    ) -> Result<Vec<crate::retained::Operation>, Error> {
        self.retained
            .clone()
            .ok_or(Error::Configuration)?
            .list(after, limit)
    }
    /// Native-owned retained submission. The caller's real authorization gate
    /// must serialize acquisition with pause and keep exact permits through this
    /// handler. This core seam alone does not provide catalog-managed authority.
    pub fn submit_retained<A: crate::admission::Authorization>(
        &mut self,
        id: crate::RequestId,
        intent: crate::Intent,
        context: crate::ExecutionContext,
        authority: &mut A,
    ) -> Result<crate::retained::Operation, Error> {
        let result = (|| {
            self.reject_pending_carrier()?;
            use crate::admission::{Access, Settlement};
            let store = self.retained.clone().ok_or(Error::Configuration)?;
            let permit = authority.acquire(Access::Recovery)?;
            let request = (|| {
                if let Some(operation) = store.operation(id)? {
                    if operation.request.intent != intent {
                        return Err(Error::IdempotencyConflict);
                    }
                    return Ok(operation.request);
                }
                if self.provisioning.fixed().is_some()
                    && !matches!(intent, crate::Intent::Publish(_))
                {
                    return Err(Error::NotProvisioned);
                }
                store.validate_new(id, context)?;
                let original_scope = intent
                    .group()
                    .map(|group| self.group_scope(group))
                    .transpose()?;
                Ok(crate::Request {
                    id,
                    intent,
                    original_scope,
                })
            })();
            authority.settle(permit, Settlement::NotDispatched)?;
            self.submit_retained_request(request?, context, authority)
        })();
        self.observations.publish();
        result
    }
    /// The one retained coordinator consumes a complete immutable Request.
    /// Convenience submission and exact direct execution only differ in how
    /// they obtain that input before reaching this boundary.
    fn submit_retained_request<A: crate::admission::Authorization>(
        &mut self,
        request: crate::Request,
        context: crate::ExecutionContext,
        authority: &mut A,
    ) -> Result<crate::retained::Operation, Error> {
        use crate::{
            admission::{mutation_access, Access, Settlement},
            retained::{Operation, State},
        };
        let store = self.retained.clone().ok_or(Error::Configuration)?;
        let permit = authority.acquire(Access::Recovery)?;
        let mut accepted = false;
        let mut recovery_failed = false;
        let mut settlement = Settlement::NotDispatched;
        let prepared = (|| {
            if let Some(mut operation) = store.operation(request.id)? {
                accepted = true;
                if operation.request != request {
                    return Err(Error::IdempotencyConflict);
                }
                if operation.state.is_terminal() {
                    return Ok(operation);
                }
                settlement = Settlement::Retained;
                match self.recover_request(operation.request.clone()) {
                    Ok(Some(outcome)) => {
                        operation.state = State::Completed(outcome);
                        store.save(&operation)?;
                        settlement = Settlement::Completed;
                    }
                    Ok(None) => (),
                    Err(error) => {
                        recovery_failed = true;
                        operation.state = State::Uncertain(prns_groups_core::ApiError::from(error));
                        store.save(&operation)?;
                    }
                }
                return Ok(operation);
            }
            if self.provisioning.fixed().is_some()
                && !matches!(request.intent, crate::Intent::Publish(_))
            {
                return Err(Error::NotProvisioned);
            }
            request.validate()?;
            store.validate_new(request.id, context)?;
            let scope = self.operation_scope(&request.intent)?;
            store.check(&request, scope)?;
            Ok(Operation {
                request,
                scope,
                state: State::Pending,
            })
        })();
        authority.settle(permit, settlement)?;
        let mut operation = prepared?;
        if operation.state.is_terminal() || recovery_failed {
            return Ok(operation);
        }
        let was_dispatched = accepted && !matches!(operation.state, State::Pending);
        let permit = authority.acquire(mutation_access(&operation.request))?;
        let mut settlement = Settlement::NotDispatched;
        let execution = (|| {
            store.check(&operation.request, operation.scope)?;
            if !accepted {
                store.accept(&operation, context)?;
            }
            settlement = Settlement::Retained;
            #[cfg(test)]
            crate::retained::fault::cut(crate::retained::fault::Cut::Accepted)?;
            operation.state = State::Dispatched;
            store.save(&operation)?;
            #[cfg(test)]
            crate::retained::fault::cut(crate::retained::fault::Cut::Dispatched)?;
            // The ID may have an exact domain receipt from a prior direct
            // caller, including an archived membership. Never skip that search.
            let result = self
                .recover_request(operation.request.clone())
                .and_then(|prior| match prior {
                    Some(outcome) => Ok(outcome),
                    None => self.execute_domain(operation.request.clone(), context),
                });
            #[cfg(test)]
            crate::retained::fault::cut(crate::retained::fault::Cut::DomainFinished)?;
            operation.state = match result {
                Ok(outcome) => State::Completed(outcome),
                Err(error) => {
                    if !was_dispatched {
                        if let Some(reason) = error.rejection_reason() {
                            State::Rejected(reason)
                        } else {
                            State::Uncertain(prns_groups_core::ApiError::from(error))
                        }
                    } else {
                        State::Uncertain(prns_groups_core::ApiError::from(error))
                    }
                }
            };
            store.save(&operation)?;
            settlement = match operation.state {
                State::Completed(_) => Settlement::Completed,
                State::Rejected(_) => Settlement::Rejected,
                _ => Settlement::Retained,
            };
            #[cfg(test)]
            crate::retained::fault::cut(crate::retained::fault::Cut::OutcomeSaved)?;
            Ok(operation)
        })();
        authority.settle(permit, settlement)?;
        execution
    }
    /// Forget a terminal result with an atomic replay floor. Unresolved rows keep
    /// their group exclusion and cannot be dismissed or evicted for capacity.
    pub fn dismiss_retained<A: crate::admission::Authorization>(
        &mut self,
        id: crate::RequestId,
        authority: &mut A,
    ) -> Result<(), Error> {
        let result = self.dismiss_retained_inner(id, authority);
        self.observations.publish();
        result
    }
    fn dismiss_retained_inner<A: crate::admission::Authorization>(
        &mut self,
        id: crate::RequestId,
        authority: &mut A,
    ) -> Result<(), Error> {
        self.reject_pending_carrier()?;
        let store = self.retained.clone().ok_or(Error::Configuration)?;
        let permit = authority.acquire(crate::admission::Access::LocalMutation)?;
        let result = store.dismiss(id);
        authority.settle(
            permit,
            if result.is_ok() {
                crate::admission::Settlement::Completed
            } else {
                crate::admission::Settlement::NotDispatched
            },
        )?;
        result
    }
    fn provisioned(&mut self, group: GroupId) -> Result<Provisioned, Error> {
        if let Some(cfg) = self.provisioning.fixed() {
            if cfg.group == group {
                return Ok(cfg.clone());
            }
        }
        self.provisioning
            .managed()?
            .provisioned(&mut self.owner, group)
            .map_err(enrollment_error)
    }
    fn history_scope(
        &mut self,
        scope: Option<GroupId>,
        control: Option<prns_groups_core::ControlReference>,
        group: GroupId,
    ) -> Result<usize, Error> {
        if scope.is_some_and(|bound| bound != group) {
            return Err(Error::Configuration);
        }
        if let Some(control) = control {
            if self.provisioned(group)?.control != control {
                return Err(Error::Configuration);
            }
        }
        // Control validation may open the current namespace. Include it in the
        // same finite per-job namespace budget even when paging old history.
        Ok(crate::MAX_HISTORY_NAMESPACES_PER_PAGE - usize::from(control.is_some()))
    }
    fn history_configuration(
        &mut self,
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
    ) -> Result<Provisioned, Error> {
        match &self.provisioning {
            Provisioning::Fixed(cfg) if cfg.group == group && cfg.incarnation == incarnation => {
                Ok(cfg.as_ref().clone())
            }
            Provisioning::Fixed(_) => Err(Error::InvalidInput),
            Provisioning::Managed(enrollment) => enrollment
                .history_configuration(&mut self.owner, group, incarnation)
                .map_err(enrollment_error),
        }
    }
    fn history_records(
        &mut self,
        group: GroupId,
        before: Option<crate::RecordCursor>,
        limit: usize,
        clock: prns_groups_core::ClockReading,
        selection: crate::RecordSelection,
        namespace_budget: usize,
    ) -> Result<crate::RecordPage, Error> {
        if limit == 0 || limit > 100 || before.is_some_and(|cursor| cursor.group != group) {
            return Err(Error::InvalidInput);
        }
        let (current_incarnation, configurations, after_page) = match &self.provisioning {
            Provisioning::Fixed(cfg) => {
                if cfg.group != group
                    || before.is_some_and(|cursor| cursor.incarnation != cfg.incarnation)
                {
                    return Err(Error::InvalidInput);
                }
                (cfg.incarnation, vec![cfg.as_ref().clone()], None)
            }
            Provisioning::Managed(enrollment) => {
                let page = enrollment
                    .history_configurations(
                        &mut self.owner,
                        group,
                        before.map(|cursor| cursor.incarnation),
                        namespace_budget,
                    )
                    .map_err(enrollment_error)?;
                (page.current_incarnation, page.configurations, page.next)
            }
        };
        let start = |incarnation| crate::RecordCursor {
            group,
            incarnation,
            before: None,
        };
        let mut result = crate::RecordPage {
            records: Vec::new(),
            visited_heads: Vec::new(),
            current_incarnation,
            next_before: after_page.map(start),
        };
        for (index, cfg) in configurations.iter().enumerate() {
            let Reply::Records(mut page) = crate::session::execute_with_owner(
                self.database,
                &mut self.owner,
                cfg,
                Command::Records {
                    group,
                    before: before.filter(|cursor| cursor.incarnation == cfg.incarnation),
                    limit: limit - result.records.len(),
                    clock,
                    selection,
                },
            )?
            else {
                return Err(Error::Corrupt);
            };
            result.records.append(&mut page.records);
            result.visited_heads.append(&mut page.visited_heads);
            if page.next_before.is_some() {
                result.next_before = page.next_before;
                break;
            }
            if result.records.len() == limit {
                result.next_before = configurations
                    .get(index + 1)
                    .map(|cfg| start(cfg.incarnation))
                    .or_else(|| after_page.map(start));
                break;
            }
        }
        Ok(result)
    }
    fn dispatch(
        &mut self,
        scope: Option<GroupId>,
        control: Option<prns_groups_core::ControlReference>,
        command: Command,
    ) -> Result<Reply, Error> {
        self.check_carrier_fence(scope, &command)?;
        match command {
            Command::ProfileSupport(support) => {
                let supported = match &mut self.provisioning {
                    Provisioning::Managed(enrollment) => enrollment
                        .set_profile_support(support)
                        .map_err(enrollment_error)?,
                    Provisioning::Fixed(config) => {
                        support
                            .require_wire(config.mls_limits.max_wire_bytes)
                            .map_err(enrollment_error)?;
                        support
                    }
                };
                if let Some(selected) = self.profile_support.as_deref_mut() {
                    *selected = Some(supported);
                }
                Ok(Reply::Done)
            }
            Command::Subscribe => self
                .observations
                .subscribe()
                .map(Reply::Subscription)
                .map_err(Error::from),
            Command::SubmitRetained { .. }
            | Command::RetainedOperation(_)
            | Command::RetainedOperations { .. }
            | Command::DismissRetained(_) => Err(Error::Configuration),
            Command::GroupScope(group) => {
                if scope.is_some_and(|bound| bound != group) {
                    return Err(Error::Configuration);
                }
                self.group_scope(group).map(Reply::GroupScope)
            }
            Command::Maintain(clock) => {
                if let Some(group) = scope {
                    let cfg = self.provisioned(group)?;
                    if control.is_some_and(|current| current != cfg.control) {
                        return Err(Error::Configuration);
                    }
                    if cfg.lifecycle == crate::GroupLifecycle::Active {
                        crate::session::execute_with_owner(
                            self.database,
                            &mut self.owner,
                            &cfg,
                            Command::Maintain(clock),
                        )?;
                    }
                } else if let Some(cfg) = self.provisioning.fixed() {
                    crate::session::execute_with_owner(
                        self.database,
                        &mut self.owner,
                        cfg,
                        Command::Maintain(clock),
                    )?;
                } else if let prns_groups_service::ClockReading::Trusted(interval) = clock {
                    self.provisioning
                        .managed()?
                        .maintain(&mut self.owner, interval.lower().0)
                        .map_err(enrollment_error)?;
                }
                Ok(Reply::Done)
            }
            Command::Identity => Ok(Reply::Identity(self.identity)),
            Command::Execute { request, context } => {
                self.execute_request(request, context).map(Reply::Outcome)
            }
            Command::Invitation { id, now } => {
                let enrollment = self.provisioning.managed()?;
                if let Some(control) = control {
                    let group = scope.ok_or(Error::Configuration)?;
                    if enrollment
                        .conversation_summary(group)
                        .map_err(enrollment_error)?
                        .and_then(|summary| summary.control)
                        != Some(control)
                    {
                        return Err(Error::Configuration);
                    }
                }
                let invitation = enrollment.invitation(id, now).map_err(enrollment_error)?;
                Ok(Reply::Invitation(invitation.filter(|invitation| {
                    scope.is_none_or(|group| invitation.group == group)
                })))
            }
            Command::Invitations { now, after, limit } => {
                limit.validate().map_err(Error::Api)?;
                let enrollment = self.provisioning.managed()?;
                if let Some(control) = control {
                    let group = scope.ok_or(Error::Configuration)?;
                    if enrollment
                        .conversation_summary(group)
                        .map_err(enrollment_error)?
                        .and_then(|summary| summary.control)
                        != Some(control)
                    {
                        return Err(Error::Configuration);
                    }
                }
                let mut page = enrollment
                    .invitations_page(now, after, limit)
                    .map_err(enrollment_error)?;
                if let Some(group) = scope {
                    // Preserve the native continuation even when this bounded
                    // identity page contains no invitation for the bound group.
                    page.items.retain(|invitation| invitation.group == group);
                }
                Ok(Reply::Invitations(page))
            }
            Command::EnrollmentWork { now, after, limit } => {
                limit.validate().map_err(Error::Api)?;
                // Work identities belong to this identity's enrollment carrier,
                // not to a caller-selected group/control scope.
                if scope.is_some() || control.is_some() {
                    return Err(Error::Configuration);
                }
                Ok(Reply::EnrollmentWork(
                    self.provisioning
                        .managed()?
                        .pending(&mut self.owner, now, after, limit)
                        .map_err(enrollment_error)?,
                ))
            }
            Command::EnrollmentHandle {
                requester,
                frame,
                now,
            } => Ok(Reply::EnrollmentFrame(
                self.provisioning
                    .managed()?
                    .handle(&mut self.owner, requester, &frame, now)
                    .map_err(enrollment_error)?,
            )),
            Command::EnrollmentComplete { id, response, now } => {
                let activity = self
                    .provisioning
                    .managed()?
                    .complete(&mut self.owner, id, &response, now)
                    .map_err(enrollment_error)?;
                Ok(Reply::EnrollmentCompleted(activity))
            }
            Command::Conversations { after, limit } => {
                limit.validate().map_err(Error::Api)?;
                if control.is_some() && scope.is_none() {
                    return Err(Error::Configuration);
                }
                if let Some(fixed) = self.provisioning.fixed() {
                    if scope.is_some_and(|group| group != fixed.group)
                        || control.is_some_and(|current| current != fixed.control)
                    {
                        return Err(Error::Configuration);
                    }
                    return crate::session::conversation_summaries(fixed, after, limit)
                        .map(Reply::Conversations);
                }
                let enrollment = self.provisioning.managed()?;
                let page = if let Some(group) = scope {
                    let summary = enrollment
                        .conversation_summary(group)
                        .map_err(enrollment_error)?;
                    if control.is_some_and(|current| {
                        summary.as_ref().and_then(|row| row.control) != Some(current)
                    }) {
                        return Err(Error::Configuration);
                    }
                    crate::ConversationPage::single(
                        summary.filter(|row| after.is_none_or(|after| row.group > after)),
                        limit,
                    )
                    .map_err(Error::Api)?
                } else {
                    enrollment
                        .conversation_summaries(after, limit)
                        .map_err(enrollment_error)?
                };
                Ok(Reply::Conversations(page))
            }
            command @ (Command::Conversation { .. } | Command::Members { .. }) => {
                let (group, limit) = match &command {
                    Command::Conversation { group, limit }
                    | Command::Members { group, limit, .. } => (*group, *limit),
                    _ => return Err(Error::Corrupt),
                };
                limit.validate().map_err(Error::Api)?;
                if scope.is_some_and(|bound| bound != group) {
                    return Err(Error::Configuration);
                }
                let managed_detail = if matches!(&command, Command::Conversation { .. }) {
                    match &self.provisioning {
                        Provisioning::Managed(enrollment) => Some((
                            enrollment
                                .managed_profile(group)
                                .map_err(enrollment_error)?
                                .ok_or(Error::NotProvisioned)?,
                            enrollment
                                .conversation_summary(group)
                                .map_err(enrollment_error)?
                                .ok_or(Error::NotProvisioned)?,
                        )),
                        Provisioning::Fixed(_) => None,
                    }
                } else {
                    None
                };
                let cfg = self.provisioned(group)?;
                if control.is_some_and(|current| current != cfg.control) {
                    return Err(Error::Configuration);
                }
                let mut reply = crate::session::execute_with_owner(
                    self.database,
                    &mut self.owner,
                    &cfg,
                    command,
                )?;
                if let (Reply::Conversation(detail), Some((profile, summary))) =
                    (&mut reply, managed_detail)
                {
                    if summary.control != Some(detail.control)
                        || summary.member_count != detail.member_count
                        || summary.owner_identity != detail.owner_identity
                        || summary.title != detail.title
                        || (summary.lifecycle != detail.lifecycle
                            && !(summary.lifecycle == crate::GroupLifecycle::Leaving
                                && detail.lifecycle == crate::GroupLifecycle::Active))
                    {
                        return Err(Error::Corrupt);
                    }
                    // A durable local Leave fences actions immediately, while
                    // the authenticated namespace remains Active until the
                    // owner commits its terminal membership transition.
                    detail.lifecycle = summary.lifecycle;
                    detail.capabilities = summary.capabilities;
                    detail.profile = Some(profile);
                }
                Ok(reply)
            }
            #[cfg(feature = "text")]
            Command::Messages {
                group,
                before,
                limit,
                clock,
            } => {
                let budget = self.history_scope(scope, control, group)?;
                let page = self.history_records(
                    group,
                    before,
                    limit,
                    clock,
                    crate::RecordSelection::Text,
                    budget,
                )?;
                crate::session::records::messages(page).map(Reply::Messages)
            }
            Command::Records {
                group,
                before,
                limit,
                clock,
                selection,
            } => {
                let budget = self.history_scope(scope, control, group)?;
                self.history_records(group, before, limit, clock, selection, budget)
                    .map(Reply::Records)
            }
            command @ (Command::Record { .. }
            | Command::Progress { .. }
            | Command::DeliveryTargets { .. }
            | Command::Changes { .. }) => {
                let (group, incarnation) = match &command {
                    Command::Record {
                        group, incarnation, ..
                    }
                    | Command::Progress {
                        group, incarnation, ..
                    }
                    | Command::DeliveryTargets {
                        group, incarnation, ..
                    } => (*group, Some(*incarnation)),
                    Command::Changes { group, after, .. } => {
                        (*group, after.map(|token| token.incarnation))
                    }
                    _ => return Err(Error::Corrupt),
                };
                self.history_scope(scope, control, group)?;
                let cfg = match incarnation {
                    Some(incarnation) => self.history_configuration(group, incarnation)?,
                    None => self.provisioned(group)?,
                };
                crate::session::execute_with_owner(self.database, &mut self.owner, &cfg, command)
            }
            command => {
                let group = scope;
                let cfg = if let Some(group) = group {
                    self.provisioned(group)?
                } else {
                    // Managed identities require an explicit group. Discovering
                    // an implicit singleton would scan an unbounded catalog.
                    self.provisioning
                        .fixed()
                        .cloned()
                        .ok_or(Error::NotProvisioned)?
                };
                if cfg.lifecycle != crate::GroupLifecycle::Active
                    && !matches!(
                        &command,
                        Command::Records { .. }
                            | Command::Record { .. }
                            | Command::Progress { .. }
                            | Command::DeliveryTargets { .. }
                            | Command::Changes { .. }
                    )
                {
                    return Err(Error::Enrollment(
                        if cfg.lifecycle == crate::GroupLifecycle::Closed {
                            crate::enrollment::EnrollmentError::GroupClosed
                        } else {
                            crate::enrollment::EnrollmentError::NotJoined
                        },
                    ));
                }
                if control.is_some_and(|head| head != cfg.control) {
                    return Err(Error::Configuration);
                }
                crate::session::execute_with_owner(self.database, &mut self.owner, &cfg, command)
            }
        }
    }
}
fn enrollment_error(error: crate::enrollment::EnrollmentError) -> Error {
    Error::Enrollment(error)
}
