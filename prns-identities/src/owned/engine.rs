use super::*;
use client::{Command, Job, RuntimeCommand};
use prns_core::identity::PrivateIdentityMaterial;
use runtime::{Controller, Phase, Progress};

type DriverError<D, const S: usize> = <D as runtime::ScopedDriver<S>>::Error;
struct Slot<E> {
    reference: core::IdentityReference,
    active: bool,
    error: Option<Arc<Error<E>>>,
    // Fence cross-executor work before private key ownership can unwind.
    dispatch: Option<dispatch::Control>,
    key: Option<Arc<driver::Key>>,
}

pub(super) struct Engine<
    'a,
    'buffers,
    const I: usize,
    const O: usize,
    const S: usize,
    const L: usize,
    D: Driver<S>,
> {
    store: &'a mut IdentityStore<I, O, S>,
    coordinator: &'a mut core::Coordinator<'buffers, I, O, S>,
    state: &'a mut runtime::RuntimeState<I, S, L>,
    driver: &'a mut D,
    wake: Waker,
    slots: [Option<Slot<DriverError<D, S>>>; I],
    termination: Option<runtime::TerminationToken>,
    diagnostics: Diagnostics<DriverError<D, S>>,
    closed: prns_storage::CloseSignal,
    suspension: Suspension,
}
enum Suspension {
    Running,
    Draining(tokio::sync::oneshot::Sender<Result<(), core::ApiError>>),
    Suspended,
    Failed(core::ApiError),
}
impl<
        'a,
        'buffers,
        const I: usize,
        const O: usize,
        const S: usize,
        const L: usize,
        D: Driver<S>,
    > Engine<'a, 'buffers, I, O, S, L, D>
{
    pub fn new(
        store: &'a mut IdentityStore<I, O, S>,
        coordinator: &'a mut core::Coordinator<'buffers, I, O, S>,
        state: &'a mut runtime::RuntimeState<I, S, L>,
        driver: &'a mut D,
        wake: Waker,
        diagnostics: Diagnostics<DriverError<D, S>>,
        closed: prns_storage::CloseSignal,
    ) -> Self {
        Self {
            store,
            coordinator,
            state,
            driver,
            wake,
            slots: std::array::from_fn(|_| None),
            termination: None,
            diagnostics,
            closed,
            suspension: Suspension::Running,
        }
    }
    fn check(
        &self,
        reference: core::IdentityReference,
    ) -> Result<core::IdentityRecord<S>, Error<DriverError<D, S>>> {
        self.store.validate_owner(self.coordinator)?;
        let record = *self.coordinator.identity(reference.id)?;
        if record.reference() != reference {
            return Err(core::Fault::IdentityChanged.into());
        }
        Ok(record)
    }
    fn slot(
        &mut self,
        reference: core::IdentityReference,
    ) -> Result<usize, Error<DriverError<D, S>>> {
        self.check(reference)?;
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|s| s.reference == reference))
        {
            return Ok(index);
        }
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(core::Fault::CapacityExceeded)?;
        self.slots[index] = Some(Slot {
            reference,
            active: false,
            error: None,
            dispatch: None,
            key: None,
        });
        Ok(index)
    }
    fn schedule(
        &mut self,
        reference: core::IdentityReference,
    ) -> Result<(), Error<DriverError<D, S>>> {
        let index = self.slot(reference)?;
        if let Some(slot) = self.slots[index].as_mut() {
            slot.active = true;
            slot.error = None;
        }
        self.wake.wake_by_ref();
        Ok(())
    }
    fn fence_transition(
        &mut self,
        prepared: core::PreparedTransition<S>,
    ) -> Result<(), Error<DriverError<D, S>>> {
        // Reuse the coordinator's current guard/report/target validation. This
        // is serialized with begin; no copied service policy or authority table.
        let current =
            self.coordinator
                .prepare(prepared.identity.id, prepared.target, prepared.review)?;
        if current != prepared {
            return Err(core::Fault::StaleLifecycle.into());
        }
        if let Some(control) = self
            .slots
            .iter()
            .flatten()
            .find(|slot| slot.reference.id == prepared.identity.id)
            .and_then(|slot| slot.dispatch.as_ref())
        {
            control.fence();
        }
        // A subsequent storage/validation error must keep this generation
        // fenced. Never reopen it because a commit did not return success.
        Ok(())
    }

    pub fn handle<J>(&mut self, job: Job<J>)
    where
        D: CommandHandler<S, J>,
    {
        self.observe_close();
        match job.command {
            Command::Extension(job) => {
                if self.closed.is_closed() {
                    D::reject(job);
                    return;
                }
                let lookup = |reference| {
                    self.slots
                        .iter()
                        .flatten()
                        .find(|slot| slot.reference == reference)
                        .and_then(|slot| slot.key.clone())
                };
                let mut admission = admission::Gate {
                    store: self.store,
                    coordinator: self.coordinator,
                    state: self.state,
                    key: &lookup,
                };
                self.driver.command(job, &mut admission);
            }
            Command::Identity(job) => self.handle_identity(job),
            Command::Runtime(command) => self.handle_runtime(command),
        }
    }
    fn handle_identity(&mut self, job: crate::api::IdentityJob) {
        use crate::api::client::Message;
        let identity = job.identity_reference();
        macro_rules! reply {
            ($tx:expr, $body:expr) => {{
                let result: Result<_, Error<DriverError<D, S>>> = (|| {
                    if self.closed.is_closed() {
                        return Err(Error::Closing);
                    }
                    $body
                })();
                let result = result.map_err(|error| {
                    let public = crate::api::owner_error(&error, |error| {
                        self.driver.classify_failure(error)
                    });
                    let previous = {
                        self.diagnostics
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .replace(Diagnostic {
                                identity,
                                error: Arc::new(error),
                            })
                    };
                    drop(previous);
                    public
                });
                let _ = $tx.send(result);
            }};
        }
        match job.message {
            Message::List(tx) => reply!(tx, {
                self.store.validate_owner(self.coordinator)?;
                Ok(self
                    .coordinator
                    .snapshot()
                    .identities()
                    .copied()
                    .map(crate::api::IdentityRecord::from)
                    .collect())
            }),
            Message::Operations(tx) => reply!(tx, {
                self.store.validate_owner(self.coordinator)?;
                Ok(self
                    .coordinator
                    .snapshot()
                    .operations()
                    .copied()
                    .map(crate::api::OperationRecord::from)
                    .collect())
            }),
            Message::FindReserved(command, digest, tx) => reply!(tx, {
                let command = command.to_core::<S>()?;
                self.store.validate_owner(self.coordinator)?;
                Ok(self.coordinator.find_reserved(&command, digest)?)
            }),
            Message::CancelReserved(token, tx) => reply!(tx, {
                self.store.validate_owner(self.coordinator)?;
                Ok(self.coordinator.cancel_reserved(self.store, token)?)
            }),
            Message::Prune(through, tx) => reply!(tx, {
                self.store.validate_owner(self.coordinator)?;
                Ok(self.coordinator.prune_completed(self.store, through)?)
            }),
            Message::Scope(tx) => reply!(tx, {
                self.store.validate_owner(self.coordinator)?;
                Ok(self.store.creation_scope()?)
            }),
            Message::Create(id, intent, import, tx) => reply!(tx, {
                self.store
                    .create_or_recover_validated(
                        self.coordinator,
                        id,
                        intent.to_core::<S>()?,
                        import.as_ref().map(|key| key.secret()),
                        |metadata| self.driver.validate_metadata(metadata),
                    )
                    .map_err(Error::Creation)
            }),
            Message::Retire(ids, tx) => reply!(
                tx,
                self.store
                    .retire_creations(self.coordinator, &ids)
                    .map_err(Error::Creation)
            ),
            Message::Snapshot(reference, tx) => reply!(tx, {
                let identity = self.check(reference)?;
                let last_error = self
                    .slots
                    .iter()
                    .flatten()
                    .find(|s| s.reference == reference)
                    .and_then(|s| s.error.clone());
                Ok(Snapshot {
                    identity: identity.into(),
                    runtime: self.coordinator.runtime_status(reference.id)?,
                    last_error: last_error.map(|error| {
                        crate::api::owner_error(&error, |error| self.driver.classify_failure(error))
                    }),
                })
            }),
            Message::Prepare(reference, target, tx) => reply!(tx, {
                let record = self.check(reference)?;
                let report = self
                    .driver
                    .dependencies(reference, record.services)
                    .map_err(Error::Driver)?;
                Ok(self
                    .coordinator
                    .prepare(reference.id, target, report)?
                    .into())
            }),
            Message::Reserve(prepared, digest, tx) => reply!(tx, {
                let prepared = prepared.to_core::<S>()?;
                self.check(core::IdentityReference {
                    id: prepared.identity.id,
                    public_hash: prepared.identity.public_hash,
                })?;
                Ok(self.coordinator.reserve(
                    self.store,
                    core::Command::Transition(prepared),
                    digest,
                )?)
            }),
            Message::Begin(reference, token, prepared, tx) => reply!(tx, {
                let prepared = prepared.to_core::<S>()?;
                self.check(reference)?;
                if prepared.identity.id != reference.id
                    || prepared.identity.public_hash != reference.public_hash
                {
                    return Err(core::Fault::IdentityChanged.into());
                }
                self.slot(reference)?; // validate bounded scheduling capacity before mutation
                let command = core::Command::Transition(prepared);
                if self.coordinator.resume(token, &command)? == core::OperationState::Reserved {
                    // Every fresh transition invalidates the old generation,
                    // including an explicit Enable that restarts a live owner.
                    self.fence_transition(prepared)?;
                }
                let state = self.coordinator.begin(self.store, token, &command)?;
                if state == core::OperationState::Transitioning {
                    self.schedule(reference)?;
                }
                Ok(state)
            }),
            Message::Stop(reference, prepared, digest, tx) => reply!(tx, {
                let prepared = prepared.to_core::<S>()?;
                self.check(reference)?;
                if prepared.identity.id != reference.id
                    || prepared.identity.public_hash != reference.public_hash
                {
                    return Err(core::Fault::IdentityChanged.into());
                }
                self.slot(reference)?;
                if let Some(token) = self.coordinator.find_applied_stop(prepared, digest)? {
                    if self
                        .coordinator
                        .resume(token, &core::Command::Transition(prepared))?
                        == core::OperationState::Transitioning
                    {
                        self.schedule(reference)?;
                    }
                    return Ok(token);
                }
                self.fence_transition(prepared)?;
                let token = self.coordinator.apply_stop(self.store, prepared, digest)?;
                self.schedule(reference)?;
                Ok(token)
            }),
            Message::Operation(token, command, tx) => reply!(tx, {
                let command = command.to_core::<S>()?;
                self.store.validate_owner(self.coordinator)?;
                Ok(self.coordinator.resume(token, &command)?)
            }),
            #[cfg(feature = "contact-card")]
            Message::ContactCard(reference, service, name, tx) => reply!(tx, {
                let admission = self.coordinator.admit(
                    reference.id,
                    reference.public_hash,
                    core::AdmissionClass::OfflineSigning,
                )?;
                // local_key checks this exclusive store and exact current
                // admission. Its zeroizing material never leaves this bounded
                // synchronous handler, so no runtime activation is required.
                let material = prns_core::identity::PrivateIdentityMaterial::from(
                    self.store.local_key(self.coordinator, admission)?,
                );
                Ok(crate::contact_card::issue(&material, service, &name)?)
            }),
            Message::Recover(reference, tx) => reply!(tx, self.schedule(reference)),
            Message::Metadata(reference, guard, metadata, tx) => reply!(tx, {
                self.check(reference)?;
                if guard.id != reference.id || guard.public_hash != reference.public_hash {
                    return Err(core::Fault::IdentityChanged.into());
                }
                self.driver.validate_metadata(metadata)?;
                self.coordinator
                    .update_metadata(self.store, guard, metadata)?;
                let record = *self.coordinator.identity(reference.id)?;
                if let Some(owner) = Controller::new(self.state).owner(reference.id) {
                    self.driver.bind_metadata(owner, reference, record.metadata);
                }
                Ok(record.metadata_guard())
            }),
        }
    }
    fn handle_runtime(&mut self, command: RuntimeCommand) {
        if self.closed.is_closed() {
            let (RuntimeCommand::Suspend(reply) | RuntimeCommand::Resume(reply)) = command;
            let _ = reply.send(Err(core::ApiError::Closing));
            return;
        }
        match command {
            RuntimeCommand::Suspend(reply) => match self.suspension {
                Suspension::Running => {
                    self.suspension = Suspension::Draining(reply);
                    self.terminate();
                }
                Suspension::Suspended => {
                    let _ = reply.send(Ok(()));
                }
                Suspension::Failed(error) => {
                    let _ = reply.send(Err(error));
                }
                Suspension::Draining(_) => {
                    let _ = reply.send(Err(core::ApiError::Runtime {
                        reason: core::RuntimeFailure::Busy,
                    }));
                }
            },
            RuntimeCommand::Resume(reply) => {
                let result = (|| -> Result<(), Error<DriverError<D, S>>> {
                    match self.suspension {
                        Suspension::Running => return Ok(()),
                        Suspension::Draining(_) => return Err(runtime::Fault::Busy.into()),
                        Suspension::Suspended => (),
                        Suspension::Failed(error) => return Err(Error::Api(error)),
                    }
                    let mut incarnation = [0; 16];
                    getrandom::fill(&mut incarnation).map_err(crate::Error::Entropy)?;
                    *self.state = runtime::RuntimeState::new(incarnation, self.state.budget())?;
                    self.termination = None;
                    self.suspension = Suspension::Running;
                    Ok(())
                })();
                let _ = reply.send(
                    result.map_err(|error| error.api_error(|e| self.driver.classify_failure(e))),
                );
            }
        }
    }
    pub fn poll_suspension(&mut self) {
        if !matches!(self.suspension, Suspension::Draining(_)) || !self.drained() {
            return;
        }
        let Suspension::Draining(reply) =
            std::mem::replace(&mut self.suspension, Suspension::Suspended)
        else {
            return;
        };
        if self.closed.is_closed() {
            let _ = reply.send(Err(core::ApiError::Closing));
            return;
        }
        // A new session clears canonical readiness only after every old effect
        // and mutation lease has drained. Durable intents and receipts survive.
        let result = self
            .coordinator
            .reload(self.store)
            .map_err(Error::<DriverError<D, S>>::from);
        if result.is_ok() {
            self.slots = std::array::from_fn(|_| None);
        }
        let result = result.map_err(|error| error.api_error(|e| self.driver.classify_failure(e)));
        if let Err(error) = result {
            self.suspension = Suspension::Failed(error);
        }
        let _ = reply.send(result);
    }
    fn observe_close(&mut self) {
        if self.closed.is_closed() {
            self.terminate();
        }
    }
    pub fn poll_recovery(&mut self, context: &mut std::task::Context<'_>) {
        // Close may have won while the previous handler was executing. Do not
        // acknowledge unfinished catalog work before the queued Closing event.
        self.observe_close();
        let lookup = |reference| {
            self.slots
                .iter()
                .flatten()
                .find(|slot| slot.reference == reference)
                .and_then(|slot| slot.key.clone())
        };
        let mut admission = admission::Gate {
            store: self.store,
            coordinator: self.coordinator,
            state: self.state,
            key: &lookup,
        };
        self.driver.poll_recovery(context, &mut admission);
    }
    pub fn poll_observations(&mut self, context: &mut std::task::Context<'_>) {
        self.driver.poll_observations(context);
    }
    pub fn terminate(&mut self) {
        if self.termination.is_some() {
            return;
        }
        for control in self
            .slots
            .iter()
            .flatten()
            .filter_map(|slot| slot.dispatch.as_ref())
        {
            control.fence();
        }
        self.termination = Some(Controller::new(self.state).begin_termination());
        self.wake.wake_by_ref();
    }
    pub fn drained(&mut self) -> bool {
        let Some(termination) = self.termination else {
            return true;
        };
        Controller::new(self.state).termination_complete(termination) == Ok(true)
            && self.slots.iter().flatten().all(|slot| {
                slot.key
                    .as_ref()
                    .is_none_or(|key| Arc::strong_count(key) == 1)
            })
    }
    pub fn progress(&mut self) {
        for index in 0..I {
            let Some(slot) = self.slots[index].as_ref() else {
                continue;
            };
            if self.termination.is_none() && !slot.active {
                continue;
            }
            let reference = slot.reference;
            let result = if self.termination.is_some() {
                self.cleanup(reference)
            } else {
                self.advance(reference, index)
            };
            match result {
                Ok(Progress::Complete(())) => self.wake.wake_by_ref(),
                Ok(Progress::Pending) => {}
                // The controller's Busy here means its single recovery-cleanup
                // owner is still occupied. Keep this exact action scheduled;
                // that owner's retained completion wakes the lane. Do not
                // self-wake or turn a driver/authority error into a retry.
                Err(Error::Runtime(runtime::Fault::Busy)) if self.termination.is_none() => {}
                Err(error) => {
                    // Driver activation failures enter the existing durable
                    // cleanup action. An error never releases an owned effect.
                    let mut cleanup = false;
                    if self.termination.is_none() {
                        if let Error::Driver(ref cause) = error {
                            if let Ok(Some(core::Action::Activate { ticket, .. })) =
                                self.coordinator.recovery_action(reference.id)
                            {
                                if let Some(control) = self.slots[index]
                                    .as_ref()
                                    .and_then(|slot| slot.dispatch.as_ref())
                                {
                                    control.fence();
                                }
                                let failure = self.driver.classify_failure(cause);
                                cleanup = self
                                    .coordinator
                                    .activation_failed(self.store, ticket, failure)
                                    .is_ok();
                            }
                        }
                    }
                    if matches!(self.suspension, Suspension::Draining(_)) {
                        let public = error.api_error(|e| self.driver.classify_failure(e));
                        if let Suspension::Draining(reply) =
                            std::mem::replace(&mut self.suspension, Suspension::Failed(public))
                        {
                            let _ = reply.send(Err(public));
                        }
                    }
                    let error = Arc::new(error);
                    let previous = {
                        self.diagnostics
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .replace(Diagnostic {
                                identity: Some(reference),
                                error: error.clone(),
                            })
                    };
                    // A generic driver error may own resources or reenter on
                    // Drop. Never destroy it under the observation mutex.
                    drop(previous);
                    if let Some(slot) = self.slots[index].as_mut() {
                        slot.error = Some(error);
                        slot.active = cleanup;
                    }
                    if cleanup {
                        self.wake.wake_by_ref();
                    }
                }
            }
        }
    }
    fn advance(
        &mut self,
        reference: core::IdentityReference,
        index: usize,
    ) -> Result<Progress<()>, Error<DriverError<D, S>>> {
        self.check(reference)?;
        let Some(action) = self.coordinator.recovery_action(reference.id)? else {
            if let Some(slot) = self.slots[index].as_mut() {
                slot.active = false;
            }
            return Ok(Progress::Pending);
        };
        match action {
            core::Action::Activate { ticket, .. } => {
                let record = *self.coordinator.identity(reference.id)?;
                let authority = self.store.runtime_authority(self.coordinator);
                let mut controller = Controller::new(self.state);
                let token = if controller.owner(reference.id).is_some() {
                    let plan = self
                        .driver
                        .activation_plan(&record)
                        .map_err(Error::Driver)?;
                    match controller.begin_activation(&authority, action, plan) {
                        Ok(token) => token,
                        Err(runtime::Fault::OwnerStillActive) => {
                            controller.reconcile_activation(&authority, action, plan)?
                        }
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    let plan = self
                        .driver
                        .activation_plan(&record)
                        .map_err(Error::Driver)?;
                    controller.begin_activation(&authority, action, plan)?
                };
                if self.slots[index].as_ref().is_some_and(|slot| {
                    slot.dispatch
                        .as_ref()
                        .is_none_or(|control| control.owner() != token)
                }) {
                    let control = dispatch::Control::new(token, self.closed.clone());
                    let lease = control.lease();
                    // Install control before invoking the native adapter, so a
                    // panic fences any clone it retained before unwinding.
                    if let Some(slot) = self.slots[index].as_mut() {
                        slot.dispatch = Some(control);
                    }
                    self.driver.bind_dispatch(token, lease);
                }
                self.driver.bind_metadata(token, reference, record.metadata);
                if self.slots[index]
                    .as_ref()
                    .is_some_and(|slot| slot.key.is_none())
                {
                    let key = Arc::new(driver::Key {
                        material: PrivateIdentityMaterial::from(
                            self.store.activate_key(self.coordinator, ticket)?,
                        ),
                        wake: self.wake.clone(),
                        _lock: self.store._lock.clone(),
                    });
                    self.driver.bind_key(token, KeyLease::new(key.clone()));
                    if let Some(slot) = self.slots[index].as_mut() {
                        slot.key = Some(key);
                    }
                }
                match controller.phase(token)? {
                    Phase::FencingActivation => {
                        Ok(controller.fence_previous_workers(&authority, self.driver, token)?)
                    }
                    Phase::Preflighting => {
                        Ok(controller.preflight(&authority, self.driver, token)?)
                    }
                    Phase::Activating => {
                        for service in record.services.iter() {
                            if controller.prepare_service(
                                &authority,
                                self.driver,
                                token,
                                service,
                            )? == Progress::Pending
                            {
                                return Ok(Progress::Pending);
                            }
                            if controller.activate_service(
                                &authority,
                                self.driver,
                                token,
                                service,
                            )? == Progress::Pending
                            {
                                return Ok(Progress::Pending);
                            }
                        }
                        controller.finish_activation(&authority, token)?;
                        Ok(Progress::Complete(()))
                    }
                    Phase::AwaitingCatalogCommit => {
                        let receipt = controller.activation_receipt(token)?;
                        self.coordinator
                            .ack_activated(self.store, receipt.ticket())?;
                        let admission = self.coordinator.admit(
                            reference.id,
                            reference.public_hash,
                            core::AdmissionClass::Network,
                        )?;
                        controller.confirm_activation(
                            &self.store.runtime_authority(self.coordinator),
                            receipt,
                            admission,
                        )?;
                        // Capture exact service-generation authority before any
                        // native notification. An identity-only owner has none.
                        let mut permits = [None; S];
                        for (slot, service) in permits.iter_mut().zip(record.services.iter()) {
                            *slot = Some(controller.network_permit(
                                &self.store.runtime_authority(self.coordinator),
                                admission,
                                service,
                            )?);
                        }
                        let control = self.slots[index]
                            .as_ref()
                            .and_then(|slot| slot.dispatch.as_ref())
                            .filter(|control| control.owner() == token)
                            .ok_or(runtime::Fault::StaleEvidence)?;
                        if control.open() {
                            for permit in permits.into_iter().flatten() {
                                self.driver.dispatch_opened(permit);
                            }
                        }
                        Ok(Progress::Complete(()))
                    }
                    Phase::Running => Ok(Progress::Pending),
                    _ => Err(runtime::Fault::WrongPhase.into()),
                }
            }
            core::Action::Quiesce { .. } => {
                if let Some(control) = self.slots[index]
                    .as_ref()
                    .and_then(|slot| slot.dispatch.as_ref())
                {
                    control.fence();
                }
                let record = *self.coordinator.identity(reference.id)?;
                let authority = self.store.runtime_authority(self.coordinator);
                let mut controller = Controller::new(self.state);
                if controller.owner(reference.id).is_some() {
                    controller.begin_quiescence(&authority, action)?;
                } else {
                    let plan = self
                        .driver
                        .activation_plan(&record)
                        .map_err(Error::Driver)?;
                    controller.recover_quiescence(&authority, action, plan)?;
                }
                self.cleanup(reference)
            }
            core::Action::InitializeService { ticket, .. } => {
                let _key = self.store.initialization_key(self.coordinator, ticket)?;
                let initialized = runtime::configuration::initialize_service(
                    &self.store.runtime_authority(self.coordinator),
                    self.driver,
                    action,
                )?;
                self.coordinator
                    .ack_service_initialized(self.store, initialized)?;
                Ok(Progress::Complete(()))
            }
            core::Action::PublishKey { .. } => Err(core::Fault::InvalidTransition.into()),
        }
    }
    fn cleanup(
        &mut self,
        reference: core::IdentityReference,
    ) -> Result<Progress<()>, Error<DriverError<D, S>>> {
        let authority = self.store.runtime_authority(self.coordinator);
        let mut controller = Controller::new(self.state);
        let Some(token) = controller.owner(reference.id) else {
            return Ok(Progress::Pending);
        };
        match controller.phase(token)? {
            Phase::FencingPreviousWorkers => {
                Ok(controller.fence_previous_workers(&authority, self.driver, token)?)
            }
            Phase::Quiescing | Phase::Quiesced => {
                if controller.phase(token)? == Phase::Quiescing {
                    let mut services = [None; S];
                    for (slot, id) in services.iter_mut().zip(controller.service_ids(token)?) {
                        *slot = Some(id);
                    }
                    for service in services.into_iter().flatten() {
                        if controller.quiesce_service(&authority, self.driver, token, service)?
                            == Progress::Pending
                        {
                            return Ok(Progress::Pending);
                        }
                    }
                }
                let Progress::Complete(receipt) =
                    controller.finish_quiescence(&authority, self.driver, token)?
                else {
                    return Ok(Progress::Pending);
                };
                let slot = self
                    .slots
                    .iter_mut()
                    .flatten()
                    .find(|slot| slot.reference == reference)
                    .ok_or(core::Fault::UnknownIdentity)?;
                if slot
                    .key
                    .as_ref()
                    .is_some_and(|key| Arc::strong_count(key) > 1)
                {
                    return Ok(Progress::Pending);
                }
                slot.key = None;
                if let Some(termination) = self.termination {
                    controller.confirm_termination(termination, receipt)?;
                } else {
                    let ticket = receipt.ticket();
                    if ticket.kind == core::EffectKind::RestoreCleanup {
                        self.coordinator.ack_recovery_quiesced(
                            self.store,
                            ticket,
                            receipt.report(),
                        )?;
                    } else {
                        let operation = self
                            .coordinator
                            .operation(ticket.operation.ok_or(core::Fault::InvalidToken)?)?;
                        if matches!(
                            operation.command,
                            core::Command::Transition(core::PreparedTransition {
                                target: core::TransitionTarget::AddService(_)
                                    | core::TransitionTarget::RemoveService(_),
                                ..
                            })
                        ) {
                            self.coordinator.ack_configuration_quiesced(
                                self.store,
                                ticket,
                                receipt.report(),
                            )?;
                        } else {
                            self.coordinator
                                .ack_quiesced(self.store, ticket, receipt.report())?;
                        }
                    }
                    controller.confirm_quiescence(
                        &self.store.runtime_authority(self.coordinator),
                        receipt,
                    )?;
                }
                Ok(Progress::Complete(()))
            }
            _ => Err(runtime::Fault::WrongPhase.into()),
        }
    }
}
