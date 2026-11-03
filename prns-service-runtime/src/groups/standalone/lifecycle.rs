//! One retained owner future: local storage outlives every host generation.
use super::*;
use std::future::poll_fn;
use tokio::runtime::Handle;

fn identity(error: prns_identities_core::ApiError) -> Error {
    Error::Identity { error }
}
enum RuntimeTransition {
    Suspend,
    Resume,
}
async fn control(local: &Local, transition: RuntimeTransition) -> Result<(), Error> {
    loop {
        let control = local.runtime_control();
        let result = match transition {
            RuntimeTransition::Suspend => control.suspend().await,
            RuntimeTransition::Resume => control.resume().await,
        };
        match result {
            Err(prns_identities_core::ApiError::Runtime {
                reason: prns_identities_core::RuntimeFailure::Busy,
            })
            | Err(prns_identities_core::ApiError::Submission {
                reason: owned::SubmitFailure::Full,
            }) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            value => return value.map_err(identity),
        }
    }
}
async fn attach(local: &Local, context: network::Context) -> Result<(), Error> {
    loop {
        let (tx, rx) = oneshot::channel();
        match local
            .commands()
            .submit(network::Command::Attach(context.clone(), tx))
        {
            Ok(()) => {
                return rx
                    .await
                    .map_err(|_| Error::LifecyclePanicked)?
                    .map_err(Error::from)
            }
            Err(owned::SubmitFailure::Full) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(_) => return Err(Error::Closing),
        }
    }
}
async fn detach(local: &Local) -> Result<(), Error> {
    loop {
        let (tx, rx) = oneshot::channel();
        match local.commands().submit(network::Command::Detach(tx)) {
            Ok(()) => {
                return rx
                    .await
                    .map_err(|_| Error::LifecyclePanicked)?
                    .map_err(Error::from)
            }
            Err(owned::SubmitFailure::Full) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            Err(_) => return Err(Error::Closing),
        }
    }
}
fn check_start(shared: &Shared, generation: u64) -> Result<(), Error> {
    let state = shared.lock();
    if state.closed {
        return Err(Error::Closing);
    }
    if state.network.phase != NetworkPhase::Starting || state.network.generation != generation {
        return Err(Error::StartupCancelled);
    }
    Ok(())
}
struct Network {
    host: Option<Arc<OwnedSession>>,
    unresolved_start: bool,
    #[cfg(feature = "connections")]
    connections: Option<Arc<HostBinding>>,
}
impl Network {
    async fn stop(&mut self, shared: &Shared, local: &Local) -> Result<(), Error> {
        if self.unresolved_start {
            return Err(Error::HostOwnershipUnavailable);
        }
        control(local, RuntimeTransition::Suspend).await?;
        detach(local).await?;
        self.release_host(shared).await
    }
    async fn release_host(&mut self, shared: &Shared) -> Result<(), Error> {
        if self.unresolved_start {
            return Err(Error::HostOwnershipUnavailable);
        }
        #[cfg(feature = "connections")]
        if let Some(binding) = &self.connections {
            binding.fence();
            binding
                .close()
                .await
                .map_err(|error| Error::ConnectionDrainUnresolved {
                    detail: error.to_string(),
                })?;
            let retired = shared.lock().connections.take();
            drop(retired);
            self.connections = None;
        }
        if let Some(host) = &self.host {
            let result = host.stop().await;
            match result {
                Ok(()) => (),
                Err(error @ SessionError::Stop(_)) => {
                    self.host = None;
                    let retired = shared.lock().host.take();
                    drop(retired);
                    return Err(error.into());
                }
                Err(error) => return Err(error.into()),
            }
            self.host = None;
        }
        let retired = shared.lock().host.take();
        drop(retired);
        Ok(())
    }
    async fn start(
        &mut self,
        shared: &Shared,
        local: &Local,
        startup: Startup,
        generation: u64,
        executor: Handle,
        #[cfg(feature = "connections")] profiles: Option<&Arc<ProfileOwner>>,
    ) -> Result<(), Error> {
        let Startup {
            host: config,
            mut embedding,
        } = startup;
        check_start(shared, generation)?;
        let router = network::Router::new(4).map_err(|_| Error::InvalidOptions)?;
        let events = router.clone();
        let mut previous = embedding.on_event.take();
        embedding.on_event = Some(Box::new(move |event| {
            events.on_event_ref(&event);
            if let Some(previous) = &mut previous {
                previous(event);
            }
        }));
        embedding.application_events =
            prns_host_runtime::ApplicationEventDispatch::EmbeddingCallback;
        let host = match OwnedSession::open_with_embedding(config, embedding).await {
            Ok(host) => host,
            Err(error) => {
                self.unresolved_start =
                    !matches!(error, SessionError::Busy | SessionError::Start(_));
                return Err(error.into());
            }
        };
        self.host = Some(host.clone());
        check_start(shared, generation)?;
        #[cfg(feature = "connections")]
        if let Some(profiles) = profiles {
            let binding =
                Arc::new(
                    profiles
                        .bind(&host)
                        .await
                        .map_err(|error| Error::Connections {
                            detail: error.to_string(),
                        })?,
                );
            self.connections = Some(binding.clone());
            shared.lock().connections = Some(binding);
        }
        check_start(shared, generation)?;
        let context = network::Context::attach(host.client(), executor, router)
            .map_err(|_| Error::HostOwnershipUnavailable)?;
        attach(local, context).await?;
        check_start(shared, generation)?;
        control(local, RuntimeTransition::Resume).await?;
        shared.lock().host = Some(host.client_scope());
        Ok(())
    }
}

pub(super) async fn run(
    shared: Arc<Shared>,
    path: PathBuf,
    create: bool,
    options: network::OwnerOptions,
    recovery: StartupRecovery,
    executor: Handle,
    #[cfg(feature = "connections")] profiles: Option<Arc<ProfileOwner>>,
) -> Result<(), Error> {
    let opened = executor
        .spawn_blocking(move || {
            if create {
                network::create_new_detached::<4, 32, 4, 16>(path, options)
            } else {
                network::open_existing_detached::<4, 32, 4, 16>(path, options)
            }
        })
        .await;
    let local = match opened {
        Ok(Ok(local)) => Arc::new(local),
        Ok(Err(error)) => {
            let error = Error::from(error);
            shared.fail(error.clone(), false);
            return Err(error);
        }
        Err(_) => {
            shared.fail(Error::LifecyclePanicked, true);
            return std::future::pending().await;
        }
    };
    if shared.lock().closed {
        match local.close().await {
            Ok(()) => return Ok(()),
            Err(error) => {
                shared.fail(Error::from(&error), true);
                return std::future::pending().await;
            }
        }
    }
    // Detached local readiness cannot admit activation while a host is absent.
    if let Err(error) = control(&local, RuntimeTransition::Suspend).await {
        shared.fail(error.clone(), false);
        let _ = local.close().await;
        return Err(error);
    }
    shared.change(|state| {
        state.local = Some(local.clone());
        state.diagnostics = Some(local.diagnostics());
        if !state.closed {
            state.startup = Some(Ok(()));
            state.status.phase = Phase::Ready;
        }
    });
    let mut network = Network {
        host: None,
        unresolved_start: false,
        #[cfg(feature = "connections")]
        connections: None,
    };
    loop {
        let request = poll_fn(|_| {
            let mut state = shared.lock();
            if state.closed {
                return Poll::Ready(None);
            }
            match state.request.take() {
                Some(request) => Poll::Ready(Some(request)),
                None => Poll::Pending,
            }
        })
        .await;
        let Some(mut request) = request else {
            break;
        };
        // Move the one-shot embedding into the host without changing request identity.
        let kind = std::mem::replace(&mut request.kind, RequestKind::Stop);
        let result = match kind {
            RequestKind::Start(startup) => {
                let result = network
                    .start(
                        &shared,
                        &local,
                        *startup,
                        request.generation,
                        executor.clone(),
                        #[cfg(feature = "connections")]
                        profiles.as_ref(),
                    )
                    .await;
                if result.is_ok()
                    && recovery == StartupRecovery::RecoverCurrent
                    && !shared.lock().closed
                {
                    match check_start(&shared, request.generation) {
                        Ok(()) => recover_current(local.identities()).await.map_err(identity),
                        Err(error) => Err(error),
                    }
                } else {
                    result
                }
            }
            RequestKind::Stop => network.stop(&shared, &local).await,
        };
        if shared.lock().closed {
            request.completion.send_replace(Some(Err(Error::Closing)));
            break;
        }
        let result = if result.is_ok() && shared.lock().request.is_some() {
            Err(Error::StartupCancelled)
        } else {
            result
        };
        let mut unresolved = false;
        if result.is_err() && request.start {
            // A start error can own a partial host or binding; drain it before a
            // retry. Data remains available throughout the retained cleanup.
            if let Err(error) = network.stop(&shared, &local).await {
                shared.complete_network(request, Err(error), true);
                continue;
            }
        }
        if result.is_err() && (!request.start || network.host.is_some()) {
            unresolved = true;
        }
        shared.complete_network(request, result, unresolved);
    }
    // Full close alone fences the data lane. Accepted jobs and mutation leases
    // drain before interfaces and executor are released.
    match local.close().await {
        Ok(()) => (),
        Err(error) => {
            shared.fail(Error::from(&error), true);
            return std::future::pending().await;
        }
    }
    let retired = shared.lock().local.take();
    drop(retired);
    drop(local);
    loop {
        let attempt = shared.lock().cleanup_retry;
        match network.release_host(&shared).await {
            Ok(()) => break,
            Err(error @ Error::HostShutdown { .. }) => {
                shared.lock().status.failure = Some(error);
                break;
            }
            Err(error) => {
                shared.fail(error, true);
                // Explicit repeated close wakes the retained cleanup retry.
                poll_fn(|_| {
                    if shared.lock().cleanup_retry != attempt {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
            }
        }
    }
    let pending = shared.lock().request.take();
    if let Some(request) = pending {
        request.completion.send_replace(Some(Err(Error::Closing)));
    }
    shared.change(|state| {
        state.network.phase = NetworkPhase::Offline;
        if state.startup.is_none() {
            state.startup = Some(Err(Error::StartupCancelled));
        }
    });
    let failure = shared.lock().status.failure.clone();
    failure.map_or(Ok(()), Err)
}
