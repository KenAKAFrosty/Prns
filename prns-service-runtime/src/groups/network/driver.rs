//! Concrete retained group carrier around the existing local domain adapter.
mod announcements;
#[cfg(feature = "contact-card")]
mod contact;
use super::{
    effects::{self, Failure, Operation, Task},
    *,
};
use crate::groups::{self as assembly, Local, SERVICE};
use prns_groups as groups;
use prns_host_runtime::service_registration::{
    staged_service_registration, IdentitySecretKey, ServiceEndpoint, ServiceLifecycleError,
};
use prns_identities::owned;
use prns_identities_core as core;
use prns_identities_runtime as runtime;
use std::{
    sync::Arc,
    task::{Context as TaskContext, Waker},
    time::Duration,
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Copy)]
pub struct RunOptions {
    pub carrier: Options,
    pub policy: groups::SyncPolicy,
    pub announce_interval_millis: Option<u64>,
}
#[derive(Debug)]
pub enum DriverError {
    Local(assembly::Error),
    Announce(prns_identities::announces::Error<prns_identities::Error>),
    Network(Error),
    Registration(prns_host_runtime::HostStartError),
    Runtime(runtime::Fault),
    Failure(Arc<Failure>),
    InvalidOptions,
    MissingOwner,
    WrongOwner,
    #[cfg(feature = "contact-card")]
    Contact(core::ApiError),
}
impl DriverError {
    pub(crate) fn service_failure(&self) -> core::ServiceFailure {
        match self {
            Self::Local(error) => error.service_failure(),
            _ => core::ServiceFailure::Unavailable(2),
        }
    }
}
impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native group carrier: {self:?}")
    }
}
impl std::error::Error for DriverError {}
impl From<assembly::Error> for DriverError {
    fn from(error: assembly::Error) -> Self {
        Self::Local(error)
    }
}
impl From<Error> for DriverError {
    fn from(error: Error) -> Self {
        Self::Network(error)
    }
}
impl From<runtime::Fault> for DriverError {
    fn from(error: runtime::Fault) -> Self {
        Self::Runtime(error)
    }
}

struct Resources {
    owner: runtime::OwnerToken,
    dispatch: DispatchLease,
    key: Option<KeyLease>,
    identity: Option<core::IdentityReference>,
    #[cfg(feature = "contact-card")]
    contact: contact::Advertisement,
    route: Option<Registration>,
    queues: Option<(
        mpsc::Receiver<transport::RequestJob>,
        mpsc::Receiver<transport::enrollment::RequestJob>,
    )>,
    endpoints: Option<[ServiceEndpoint; 2]>,
    registration: Option<Task>,
    runner: Option<Task>,
    start: Option<oneshot::Sender<runtime::NetworkPermit>>,
    stop: Option<watch::Sender<bool>>,
    retirement: Option<Task>,
    ingress_ready: Arc<std::sync::atomic::AtomicBool>,
    opened: bool,
    quiesced: bool,
    failure: Option<Arc<Failure>>,
}
impl Resources {
    fn new(dispatch: DispatchLease) -> Self {
        Self {
            owner: dispatch.owner(),
            dispatch,
            key: None,
            identity: None,
            #[cfg(feature = "contact-card")]
            contact: contact::Advertisement::default(),
            route: None,
            queues: None,
            endpoints: None,
            registration: None,
            runner: None,
            start: None,
            stop: None,
            retirement: None,
            ingress_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            opened: false,
            quiesced: false,
            failure: None,
        }
    }
    fn fence(&mut self) {
        if let Some(route) = &self.route {
            route.scope().fence();
        }
        if let Some(stop) = &self.stop {
            stop.send_replace(true);
        }
        self.start.take();
    }
}

/// One selected network adapter. These slots own actual tasks/endpoints/keys;
/// the identity Controller remains the sole lifecycle and admission authority.
pub struct Driver<const I: usize, const S: usize, J> {
    local: Local<I>,
    network: Option<Context>,
    diagnostics: effects::Diagnostics,
    announcements: announcements::Shared<I>,
    submission: assembly::Submission<J>,
    route: fn(assembly::Command) -> J,
    options: RunOptions,
    wake: Waker,
    owners: Vec<Resources>,
}
impl<const I: usize, const S: usize, J: Send + 'static> Driver<I, S, J> {
    pub fn new(
        local: Local<I>,
        network: Context,
        commands: owned::Commands<J>,
        route: fn(assembly::Command) -> J,
        options: RunOptions,
        wake: Waker,
    ) -> Result<Self, DriverError> {
        Self::construct(local, Some(network), commands, route, options, wake)
    }
    pub fn new_detached(
        local: Local<I>,
        commands: owned::Commands<J>,
        route: fn(assembly::Command) -> J,
        options: RunOptions,
        wake: Waker,
    ) -> Result<Self, DriverError> {
        Self::construct(local, None, commands, route, options, wake)
    }
    fn construct(
        local: Local<I>,
        network: Option<Context>,
        commands: owned::Commands<J>,
        route: fn(assembly::Command) -> J,
        options: RunOptions,
        wake: Waker,
    ) -> Result<Self, DriverError> {
        Self::validate_options(options)?;
        Self::validate_client_limits(options, local.state.input.limits())?;
        let support = Self::profile_support(options)?;
        let local = local.with_profile_support(support)?;
        let mut owners = Vec::new();
        owners.try_reserve_exact(I).map_err(|_| Error::Allocation)?;
        let submission = local.submission(commands);
        let announcements = announcements::Shared::open(local.stores.root())?;
        let diagnostics = network
            .as_ref()
            .map(Context::diagnostics)
            .unwrap_or_default();
        Ok(Self {
            local,
            network,
            diagnostics,
            announcements,
            submission,
            route,
            options,
            wake,
            owners,
        })
    }
    pub(super) fn validate_options(options: RunOptions) -> Result<(), DriverError> {
        HostServiceScope::validate_capacity(
            options.carrier.operations,
            options.carrier.outgoing_links,
        )
        .map_err(Error::Scope)?;
        if !options.policy.is_valid()
            || options.carrier.outgoing_links == 0
            || options.carrier.outgoing_links > 256
        {
            return Err(DriverError::InvalidOptions);
        }
        transport::destination_config(options.carrier.sync).map_err(Error::Transport)?;
        transport::enrollment::destination_config(options.carrier.enrollment)
            .map_err(Error::Transport)?;
        prns_identities::announces::Settings {
            interval_millis: options.announce_interval_millis,
        }
        .validate()
        .map_err(|_| DriverError::InvalidOptions)?;
        Self::service_resources(options)?;
        Ok(())
    }
    fn profile_support(options: RunOptions) -> Result<groups::ProfileSupport, DriverError> {
        let wire = options
            .carrier
            .sync
            .max_envelope_bytes()
            .map_err(Error::Transport)?;
        let enrollment = options
            .carrier
            .enrollment
            .max_application_frame_bytes()
            .map_err(Error::Transport)?;
        groups::ProfileSupport::new(wire, enrollment).map_err(|_| DriverError::InvalidOptions)
    }
    pub(super) fn validate_client_limits(
        options: RunOptions,
        limits: groups::ClientLimits,
    ) -> Result<(), DriverError> {
        limits.validate().map_err(|_| DriverError::InvalidOptions)?;
        let payload = options.carrier.sync.max_frame_bytes.max(
            options
                .carrier
                .enrollment
                .max_application_frame_bytes()
                .map_err(Error::Transport)?,
        );
        let payload = u64::try_from(payload).map_err(|_| DriverError::InvalidOptions)?;
        let required = groups::ClientLimits::required_job_bytes(payload)
            .map_err(|_| DriverError::InvalidOptions)?;
        if limits.max_job_bytes < required {
            return Err(DriverError::InvalidOptions);
        }
        Ok(())
    }
    pub fn local(&mut self) -> &mut Local<I> {
        &mut self.local
    }
    pub fn diagnostics(&self) -> effects::Diagnostics {
        self.diagnostics.clone()
    }
    fn network(&self) -> Result<&Context, DriverError> {
        self.network
            .as_ref()
            .ok_or(assembly::Error::NetworkUnavailable.into())
    }
    pub fn attach(&mut self, network: Context) -> Result<(), DriverError> {
        if !self.owners.is_empty() || self.network.is_some() {
            return Err(DriverError::InvalidOptions);
        }
        self.diagnostics = network.diagnostics();
        self.network = Some(network);
        self.wake.wake_by_ref();
        Ok(())
    }
    pub fn detach(&mut self) -> Result<(), DriverError> {
        if !self.owners.is_empty() {
            return Err(DriverError::Network(Error::NotDrained));
        }
        self.announcements.detach();
        self.network.take();
        Ok(())
    }
    fn index(&self, owner: runtime::OwnerToken) -> Result<usize, DriverError> {
        self.owners
            .iter()
            .position(|entry| entry.owner == owner)
            .ok_or(DriverError::MissingOwner)
    }
    fn service_resources(options: RunOptions) -> Result<runtime::Resources, DriverError> {
        let queue_slots = options
            .carrier
            .sync
            .inbound_jobs
            .checked_add(options.carrier.enrollment.inbound_jobs)
            .and_then(|value| value.checked_add(options.carrier.operations))
            .and_then(|value| value.checked_add(1))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(DriverError::InvalidOptions)?;
        Ok(runtime::Resources {
            held_keys: 0,
            destinations: 2,
            request_handlers: 2,
            links: u32::try_from(options.carrier.outgoing_links)
                .map_err(|_| DriverError::InvalidOptions)?,
            queue_slots,
        })
    }
    fn reference(scope: runtime::Scope) -> core::IdentityReference {
        core::IdentityReference {
            id: scope.identity.id,
            public_hash: scope.identity.public_hash,
        }
    }
    fn cleanup(&mut self, index: usize) -> Result<bool, DriverError> {
        if self.announcements.pending(self.owners[index].owner) {
            return Ok(false);
        }
        let network = self.network.clone();
        let entry = &mut self.owners[index];
        entry.fence();
        #[cfg(feature = "contact-card")]
        if let Some(publication) = &mut entry.contact.task {
            if publication.task.poll(&self.wake).is_none() {
                return Ok(false);
            }
        }
        // A cancelled caller or task result cannot hide an accepted registration.
        // Both endpoint tokens were retained before the first host command.
        if let Some(task) = &mut entry.registration {
            if task.poll(&self.wake).is_none() {
                return Ok(false);
            }
        }
        if let Some(task) = &mut entry.runner {
            if task.poll(&self.wake).is_none() {
                return Ok(false);
            }
        }
        if entry.retirement.is_none() {
            if let (Some(route), Some(endpoints), Some(key)) =
                (&entry.route, &entry.endpoints, &entry.key)
            {
                let scope = route.scope().clone();
                let endpoints = endpoints.clone();
                let key = key.clone();
                entry.retirement = Some(Task::spawn(
                    network
                        .as_ref()
                        .ok_or(assembly::Error::NetworkUnavailable)?,
                    entry.owner,
                    Operation::Retirement,
                    async move {
                        let _key = key;
                        scope.wait_drained().await;
                        let links = scope.outgoing_links().map_err(Failure::Scope)?;
                        let links: Vec<_> = links
                            .into_iter()
                            .map(|link| {
                                prns_runtime::routing::links::LinkId::new(link.into_bytes())
                            })
                            .collect();
                        for endpoint in endpoints {
                            let pause = match endpoint.prepare_pause(links.clone()).await {
                                Ok(pause) => pause,
                                Err(ServiceLifecycleError::NotRegistered) => continue,
                                Err(error) => return Err(Failure::Service(error)),
                            };
                            // Canonical group destinations use NoRatchets. MLS
                            // receive state belongs to the already-durable group store.
                            if pause.ratchets().is_some() {
                                return Err(Failure::UnexpectedReceiveState);
                            }
                            while !pause.is_drained() {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            endpoint
                                .finish_pause(&pause)
                                .await
                                .map_err(Failure::Service)?;
                        }
                        Ok(())
                    },
                ));
            }
        }
        if let Some(task) = &mut entry.retirement {
            match task.poll(&self.wake) {
                None => return Ok(false),
                Some(Err(error)) => {
                    entry.retirement = None;
                    entry.failure = Some(error.clone());
                    return Err(DriverError::Failure(error));
                }
                Some(Ok(())) => (),
            }
        }
        if let Some(route) = &mut entry.route {
            route.finish()?;
        }
        entry.route = None;
        entry.queues = None;
        entry.endpoints = None;
        entry.registration = None;
        entry.runner = None;
        entry.retirement = None;
        entry.quiesced = true;
        Ok(true)
    }
}
impl<const I: usize, const S: usize, J: Send + 'static> owned::Driver<S> for Driver<I, S, J> {
    fn poll_recovery(
        &mut self,
        context: &mut TaskContext<'_>,
        admission: &mut dyn owned::RecoveryAdmission,
    ) {
        self.local.poll_recovery(context, admission);
    }
    fn poll_observations(&mut self, context: &mut TaskContext<'_>) {
        self.local.poll_observations(context);
        #[cfg(feature = "contact-card")]
        self.poll_contact_publications();
        self.poll_announcements(context);
        for entry in &mut self.owners {
            if entry.opened && !entry.dispatch.is_open() {
                entry.fence();
            }
            for task in [&mut entry.registration, &mut entry.runner]
                .into_iter()
                .flatten()
            {
                if let Some(Err(error)) = task.poll(context.waker()) {
                    entry.failure = Some(error);
                    if let Some(route) = &entry.route {
                        route.scope().fence();
                    }
                }
            }
        }
    }
    fn bind_dispatch(&mut self, owner: runtime::OwnerToken, dispatch: DispatchLease) {
        if owner != dispatch.owner() {
            return;
        }
        if self.owners.iter().any(|entry| entry.owner == owner) {
            return;
        }
        if self.owners.len() < I {
            self.owners.push(Resources::new(dispatch));
        }
    }
    #[cfg(feature = "contact-card")]
    fn validate_metadata(&self, metadata: core::Metadata) -> Result<(), core::Fault> {
        prns_identities::contact_card::validate_name(metadata.published_name.as_str())
            .map_err(|_| core::Fault::InvalidMetadata)
    }
    #[cfg(feature = "contact-card")]
    fn bind_metadata(
        &mut self,
        owner: runtime::OwnerToken,
        identity: core::IdentityReference,
        metadata: core::Metadata,
    ) {
        if let Some(entry) = self.owners.iter_mut().find(|entry| entry.owner == owner) {
            if entry.identity.is_some_and(|current| current != identity) {
                entry.fence();
                return;
            }
            entry.identity = Some(identity);
            if entry.contact.name != Some(metadata.published_name) {
                if let Some(publication) = &entry.contact.task {
                    publication.supersede();
                }
                entry.contact.name = Some(metadata.published_name);
                self.wake.wake_by_ref();
            }
        }
    }
    fn bind_key(&mut self, owner: runtime::OwnerToken, key: KeyLease) {
        if let Some(entry) = self.owners.iter_mut().find(|entry| entry.owner == owner) {
            entry.key = Some(key);
        }
    }
    fn dispatch_opened(&mut self, permit: runtime::NetworkPermit) {
        if permit.service() != SERVICE {
            return;
        }
        if let Some(entry) = self
            .owners
            .iter_mut()
            .find(|entry| entry.owner == permit.owner())
        {
            if entry.identity != Some(assembly::reference(permit.admission().identity)) {
                entry.fence();
                return;
            }
            if let Some(start) = entry.start.take() {
                entry.opened = true;
                if start.send(permit).is_err() {
                    entry.fence();
                }
            }
        }
    }
    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<S>,
    ) -> Result<runtime::ActivationPlan<S>, DriverError> {
        let mut services = [None; S];
        for (slot, service) in services.iter_mut().zip(identity.services.iter()) {
            if service != SERVICE {
                return Err(assembly::Error::UnsupportedService(service).into());
            }
            *slot = Some(runtime::ServicePlan {
                service,
                resources: Self::service_resources(self.options)?,
                requires_receive_state: true,
                capabilities: runtime::Capabilities::QUALIFIED,
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
    ) -> Result<core::DependencyReport<S>, DriverError> {
        let mut report =
            <Local<I> as owned::Driver<S>>::dependencies(&mut self.local, identity, services)?;
        if self.owners.iter().any(|entry| {
            entry.identity == Some(identity) && entry.failure.is_some() && !entry.quiesced
        }) {
            for entry in report.entries.iter_mut().flatten() {
                if entry.service == SERVICE {
                    entry.recovery_pending = true;
                }
            }
        }
        Ok(report)
    }
    fn classify_failure(&self, error: &DriverError) -> core::ServiceFailure {
        error.service_failure()
    }
}
impl<const I: usize, const S: usize, J: Send + 'static> runtime::configuration::ServiceInitializer
    for Driver<I, S, J>
{
    type Error = DriverError;
    fn initialize_service(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
        key: core::KeyReference,
        service: core::ServiceId,
    ) -> Result<core::InitializedService, Self::Error> {
        runtime::configuration::ServiceInitializer::initialize_service(
            &mut self.local,
            ticket,
            identity,
            key,
            service,
        )
        .map_err(Into::into)
    }
}
impl<const I: usize, const S: usize, J: Send + 'static> runtime::ScopedDriver<S>
    for Driver<I, S, J>
{
    type Error = DriverError;
    fn preflight(
        &mut self,
        scope: runtime::Scope,
        plan: &runtime::ActivationPlan<S>,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::Resources>>, Self::Error> {
        let network = self.network()?.clone();
        let index = self.index(scope.owner)?;
        let identity = Self::reference(scope);
        let entry = &mut self.owners[index];
        if entry
            .key
            .as_ref()
            .is_none_or(|key| key.material().identity_hash().as_bytes() != &identity.public_hash.0)
        {
            return Err(DriverError::WrongOwner);
        }
        entry.identity = Some(identity);
        if plan.services.iter().any(Option::is_some) {
            network.host().service_client().map_err(Error::Host)?;
            match self.local.dependency(identity) {
                Ok(_) => (),
                Err(assembly::Error::Observing) => return Ok(runtime::Progress::Pending),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: plan.total()?,
        }))
    }
    fn prepare_service(
        &mut self,
        scope: runtime::Scope,
        _: core::KeyReference,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::PreparedService>>, Self::Error> {
        if plan.service != SERVICE {
            return Err(assembly::Error::UnsupportedService(plan.service).into());
        }
        let network = self.network()?.clone();
        let index = self.index(scope.owner)?;
        if self.owners[index].registration.is_none() {
            let entry = &self.owners[index];
            let key = entry.key.as_ref().ok_or(DriverError::MissingOwner)?.clone();
            let secret = || IdentitySecretKey::new(*key.material().as_bytes());
            let sync = staged_service_registration(
                transport::destination_config(self.options.carrier.sync)
                    .map_err(Error::Transport)?,
                secret(),
            )
            .map_err(DriverError::Registration)?;
            let enrollment_config =
                transport::enrollment::destination_config(self.options.carrier.enrollment)
                    .map_err(Error::Transport)?;
            #[cfg(feature = "contact-card")]
            let enrollment_config = {
                let mut config = enrollment_config;
                config.announce_app_data = contact::card(entry)?;
                config
            };
            let enrollment = staged_service_registration(enrollment_config, secret())
                .map_err(DriverError::Registration)?;
            let endpoints = {
                let host = network.host().service_client().map_err(Error::Host)?;
                [
                    host.protocols().service_endpoint(),
                    host.protocols().service_endpoint(),
                ]
            };
            let prepared =
                network.prepare(entry.dispatch.clone(), key.clone(), self.options.carrier)?;
            let expected = [
                prepared.registration.sync_destination(),
                prepared.registration.enrollment_destination(),
            ];
            let task_endpoints = endpoints.clone();
            let entry = &mut self.owners[index];
            #[cfg(feature = "contact-card")]
            {
                entry.contact.installed = entry.contact.name;
            }
            entry.route = Some(prepared.registration);
            entry.queues = Some((prepared.sync_jobs, prepared.enrollment_jobs));
            entry.endpoints = Some(endpoints);
            entry.registration = Some(Task::spawn(
                &network,
                scope.owner,
                Operation::Registration,
                async move {
                    let _key = key;
                    for ((endpoint, registration), destination) in task_endpoints
                        .into_iter()
                        .zip([sync, enrollment])
                        .zip(expected)
                    {
                        let registered = endpoint
                            .register(registration)
                            .await
                            .map_err(Failure::Service)?;
                        if registered.as_bytes() != destination.as_bytes() {
                            return Err(Failure::WrongDestination);
                        }
                    }
                    Ok(())
                },
            ));
        }
        let entry = &mut self.owners[index];
        match entry
            .registration
            .as_mut()
            .ok_or(DriverError::MissingOwner)?
            .poll(&self.wake)
        {
            None => Ok(runtime::Progress::Pending),
            Some(Err(error)) => Err(DriverError::Failure(error)),
            Some(Ok(())) => Ok(runtime::Progress::Complete(runtime::Evidence {
                scope,
                value: runtime::PreparedService {
                    storage_ready: true,
                    receive_state_restored: true,
                },
            })),
        }
    }
    fn activate_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<()>>, Self::Error> {
        if plan.service != SERVICE {
            return Err(assembly::Error::UnsupportedService(plan.service).into());
        }
        let context = self.network()?.clone();
        let index = self.index(scope.owner)?;
        let entry = &mut self.owners[index];
        if entry.runner.is_none() {
            let (sync_jobs, enrollment_jobs) =
                entry.queues.take().ok_or(DriverError::MissingOwner)?;
            let route = entry.route.as_ref().ok_or(DriverError::MissingOwner)?;
            let network = route.scope().clone();
            let dispatch = entry.dispatch.clone();
            let ingress_ready = entry.ingress_ready.clone();
            let owner_wake = self.wake.clone();
            let sync_destination = route.sync_destination();
            let enrollment_destination = route.enrollment_destination();
            let endpoints = entry
                .endpoints
                .as_ref()
                .ok_or(DriverError::MissingOwner)?
                .clone();
            let key = entry.key.as_ref().ok_or(DriverError::MissingOwner)?.clone();
            let identity = entry.identity.ok_or(DriverError::MissingOwner)?;
            let submission = self.submission.clone();
            let map = self.route;
            let options = self.options;
            let (start, ready) = oneshot::channel();
            let (stop, mut stopping) = watch::channel(false);
            // Shared durable scheduling owns all unsolicited announcements.
            let (_, requests) = groups::announce_control(None)
                .map_err(|error| DriverError::Failure(Arc::new(Failure::Group(error))))?;
            entry.start = Some(start);
            entry.stop = Some(stop);
            entry.runner = Some(Task::spawn(
                &context,
                scope.owner,
                Operation::Runner,
                async move {
                    let _key = key;
                    let result = async {
                        let permit = tokio::select! {
                            value = ready => match value {
                                Ok(permit) => permit,
                                Err(_) => return Ok(()),
                            },
                            _ = stopping.wait_for(|value| *value) => return Ok(()),
                        };
                        if *stopping.borrow() || network.check_open().is_err() {
                            return Ok(());
                        }
                        let client = assembly::client::runner_client(
                            submission.clone(),
                            identity,
                            permit,
                            map,
                            dispatch.clone(),
                        )
                        .map_err(Failure::Group)?;
                        let _observations = submission
                            .observe_runner(identity, permit.owner(), &client)
                            .map_err(Failure::Group)?;
                        for endpoint in endpoints {
                            let activation = network
                                .protocol_operation(move |_| async move {
                                    endpoint.activate_ingress().await
                                })
                                .await;
                            match activation {
                                // A canonical stop/owner close can win after
                                // confirmation but before this queued opening.
                                // Its exact immutable fence is expected; other
                                // startup failures retain their original result.
                                Err(HostServiceScopeError::Fenced) if !dispatch.is_open() => {
                                    return Ok(());
                                }
                                result => {
                                    result.map_err(Failure::Scope)?.map_err(Failure::Service)?
                                }
                            }
                        }
                        ingress_ready.store(true, std::sync::atomic::Ordering::Release);
                        owner_wake.wake_by_ref();
                        let result = groups::run_multi_scoped_with_announcements(
                            client,
                            network.clone(),
                            sync_destination,
                            enrollment_destination,
                            sync_jobs,
                            enrollment_jobs,
                            options.carrier.sync,
                            options.carrier.enrollment,
                            options.policy,
                            requests,
                            stopping,
                        )
                        .await;
                        match result {
                            // The transport constructor checks this same
                            // immutable scope. Canonical Close can win after
                            // profile binding but before the next constructor.
                            Err(groups::Error::Transport(transport::Error::Network(
                                transport::NetworkError::Scope(HostServiceScopeError::Fenced),
                            ))) if !dispatch.is_open() => Ok(()),
                            result => result.map_err(Failure::Group),
                        }
                    }
                    .await;
                    if result.is_err() {
                        network.fence();
                    }
                    result
                },
            ));
        }
        if let Some(Err(error)) = entry
            .runner
            .as_mut()
            .ok_or(DriverError::MissingOwner)?
            .poll(&self.wake)
        {
            return Err(DriverError::Failure(error));
        }
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: (),
        }))
    }
    fn fence_previous_workers(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<bool>>, Self::Error> {
        let identity = Self::reference(scope);
        if self.owners.iter().any(|entry| {
            entry.owner != scope.owner && entry.identity == Some(identity) && !entry.quiesced
        }) {
            return Err(DriverError::WrongOwner);
        }
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: true,
        }))
    }
    fn quiesce_service(
        &mut self,
        scope: runtime::Scope,
        plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, Self::Error> {
        if plan.service != SERVICE {
            return Err(assembly::Error::UnsupportedService(plan.service).into());
        }
        if let Ok(index) = self.index(scope.owner) {
            if !self.cleanup(index)? {
                return Ok(runtime::Progress::Pending);
            }
        }
        self.local.quiesce(scope).map_err(Into::into)
    }
    fn release_identity(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::ReleasedIdentity<S>>>, Self::Error>
    {
        if let Ok(index) = self.index(scope.owner) {
            if self.owners[index].route.is_some() && !self.owners[index].quiesced {
                return Ok(runtime::Progress::Pending);
            }
        }
        let result =
            <Local<I> as runtime::ScopedDriver<S>>::release_identity(&mut self.local, scope)?;
        if matches!(result, runtime::Progress::Complete(_)) {
            if let Ok(index) = self.index(scope.owner) {
                drop(self.owners.swap_remove(index));
            }
        }
        Ok(result)
    }
}
impl<const I: usize, const S: usize, J: Send + 'static> owned::CommandHandler<S, assembly::Command>
    for Driver<I, S, J>
{
    fn command(&mut self, job: assembly::Command, admission: &mut dyn owned::Admission) {
        self.local.command(job, admission);
    }
    fn reject(job: assembly::Command) {
        Local::<I>::reject(job);
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;

impl<const I: usize, const S: usize, J: Send + 'static> owned::CommandHandler<S, super::Command>
    for Driver<I, S, J>
{
    fn command(&mut self, job: super::Command, admission: &mut dyn owned::Admission) {
        match job {
            super::Command::Group(job) => self.local.command(job, admission),
            super::Command::Announce(job) => self.announce_command(job, admission),
            super::Command::Attach(context, reply) => {
                let _ = reply.send(self.attach(context));
            }
            super::Command::Detach(reply) => {
                let _ = reply.send(self.detach());
            }
        }
    }
    fn reject(job: super::Command) {
        match job {
            super::Command::Group(job) => Local::<I>::reject(job),
            super::Command::Announce(job) => job.fail(core::ApiError::Closing),
            super::Command::Attach(_, reply) | super::Command::Detach(reply) => {
                let _ = reply.send(Err(DriverError::MissingOwner));
            }
        }
    }
}
