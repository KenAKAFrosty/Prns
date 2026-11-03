//! Group ingress on one supplied host and service executor.
//!
//! Install `Router` in `HostEmbedding` before starting the host, then attach
//! its resulting `HostClient` here. The embedding retains the host owner and
//! executor until every service has completed its scoped cleanup. No runtime or
//! host is constructed by a group, and this module grants no lifecycle authority.
mod client;
pub use client::{announcements, Command};
mod driver;
mod effects;
mod factory;
mod routes;
pub use driver::{Driver, DriverError, RunOptions};
pub use effects::{Diagnostic, Diagnostics, Failure, Operation};
pub use factory::{
    create_new, create_new_detached, open_existing, open_existing_detached, Owner, OwnerOptions,
};
pub use routes::{Registration, Router};

use prns_groups_reticulum as transport;
use prns_host::{DestinationHash, IdentityHash};
use prns_host_runtime::{
    owner::{HostClient, SessionError},
    service_scope::{HostDispatchGate, HostServiceScope, HostServiceScopeError},
};
use prns_identities::owned::{DispatchLease, KeyLease};
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc};

#[derive(Debug)]
pub enum Error {
    InvalidCapacity,
    Allocation,
    CallbackOccupied,
    ApplicationEventsUnavailable,
    Capacity,
    DuplicateIdentity,
    WrongOwner,
    NotDrained,
    Host(SessionError),
    Scope(HostServiceScopeError),
    Transport(transport::NetworkError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native group network attachment: {self:?}")
    }
}
impl std::error::Error for Error {}

/// Explicit bounds for this identity's carrier resources. Domain frame/queue
/// limits retain their canonical transport types and validation.
#[derive(Clone, Copy)]
pub struct Options {
    pub operations: usize,
    pub outgoing_links: usize,
    pub sync: transport::Limits,
    pub enrollment: transport::enrollment::Limits,
}

/// Borrowed host plus the embedding's one service executor. Clones do not stop
/// either resource. The supplied executor must outlive all retained group tasks;
/// an executor handle alone is not proof that its runtime remains running.
#[derive(Clone)]
pub struct Context {
    host: HostClient,
    executor: Handle,
    router: Router,
    diagnostics: Diagnostics,
}
impl Context {
    /// `router` must be the exact router installed in this host's native
    /// embedding. This trusted native construction API cannot retrofit ownership
    /// of application events onto an already started queue-mode host.
    pub fn attach(host: HostClient, executor: Handle, router: Router) -> Result<Self, Error> {
        if host.application_event_dispatch()
            != prns_host_runtime::ApplicationEventDispatch::EmbeddingCallback
        {
            return Err(Error::ApplicationEventsUnavailable);
        }
        host.service_client().map_err(Error::Host)?;
        Ok(Self {
            host,
            executor,
            router,
            diagnostics: Diagnostics::default(),
        })
    }
    pub fn host(&self) -> &HostClient {
        &self.host
    }
    pub fn executor(&self) -> &Handle {
        &self.executor
    }
    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics.clone()
    }

    /// Assemble bounded routes before registering/activating host endpoints.
    /// The scoped driver owns the returned registration and receivers through
    /// actual teardown. This does not register destinations or start a runner.
    pub fn prepare(
        &self,
        dispatch: DispatchLease,
        key: KeyLease,
        options: Options,
    ) -> Result<Prepared, Error> {
        // The endpoint retirement API accepts at most 256 exact outgoing links.
        // Keep this explicit plan within that cleanup capability before effects.
        if options.outgoing_links == 0 || options.outgoing_links > 256 {
            return Err(Error::InvalidCapacity);
        }
        let identity = IdentityHash::new(*key.material().identity_hash().as_bytes());
        let sync_destination = transport::destination_for(identity).map_err(Error::Transport)?;
        let enrollment_destination =
            transport::enrollment_destination_for(identity).map_err(Error::Transport)?;
        let (sync, sync_jobs) =
            transport::Ingress::new(sync_destination, options.sync).map_err(Error::Transport)?;
        let (enrollment, enrollment_jobs) =
            transport::enrollment::Ingress::new(enrollment_destination, options.enrollment)
                .map_err(Error::Transport)?;
        let scope = HostServiceScope::new_restricted(
            self.host.clone(),
            identity,
            vec![sync_destination, enrollment_destination],
            options.operations,
            options.outgoing_links,
            Arc::new(Dispatch(dispatch.clone())),
        )
        .map_err(Error::Scope)?;
        let registration = self.router.register(routes::Route {
            dispatch,
            scope,
            sync,
            enrollment,
            sync_destination,
            enrollment_destination,
            _key: key,
        })?;
        Ok(Prepared {
            registration,
            sync_jobs,
            enrollment_jobs,
        })
    }
}
struct Dispatch(DispatchLease);
impl HostDispatchGate for Dispatch {
    fn is_open(&self) -> bool {
        self.0.is_open()
    }
}

/// The real transport queues remain owned by the retained runner future. Losing
/// a result waiter must not drop this state or unregister its callback route.
pub struct Prepared {
    pub registration: Registration,
    pub sync_jobs: mpsc::Receiver<transport::RequestJob>,
    pub enrollment_jobs: mpsc::Receiver<transport::enrollment::RequestJob>,
}
