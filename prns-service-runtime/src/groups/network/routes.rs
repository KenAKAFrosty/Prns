use super::*;
use prns_host_runtime::{ApplicationEventDispatch, HostEmbedding};
use prns_identities_runtime::OwnerToken;
use prns_runtime::runtime::PrnsEvent;
use std::sync::{Mutex, MutexGuard};

const MAX_ROUTES: usize = 32;
struct State {
    routes: Vec<Option<Arc<Route>>>,
}
/// Application-event ownership, prepared before host startup. This is a
/// bounded table of actual ingress/cleanup resources, not identity authority.
#[derive(Clone)]
pub struct Router(Arc<Mutex<State>>);
impl Router {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 || capacity > MAX_ROUTES {
            return Err(Error::InvalidCapacity);
        }
        let mut routes = Vec::new();
        routes
            .try_reserve_exact(capacity)
            .map_err(|_| Error::Allocation)?;
        routes.resize_with(capacity, || None);
        Ok(Self(Arc::new(Mutex::new(State { routes }))))
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Preserve other platform/announce configuration, and refuse to replace an
    /// existing application callback. The host claims EmbeddingCallback ownership
    /// at startup so these requests cannot also flow to the foreign event queue.
    pub fn install(&self, embedding: &mut HostEmbedding) -> Result<(), Error> {
        if embedding.on_event.is_some()
            || embedding.application_events != ApplicationEventDispatch::Queue
        {
            return Err(Error::CallbackOccupied);
        }
        let router = self.clone();
        embedding.on_event = Some(Box::new(move |event| router.on_event_ref(&event)));
        embedding.application_events = ApplicationEventDispatch::EmbeddingCallback;
        Ok(())
    }
    /// Route borrowed events from a selected native composition callback. This
    /// permits one callback to combine groups with another native protocol;
    /// `install` is the exclusive convenience path, not a service catalog.
    pub fn on_event_ref(&self, event: &PrnsEvent<'_>) {
        // Snapshot only Arc ownership under the table lock, without allocation.
        // Scope locks, filtering/copying input and dropping keys all occur later.
        let mut routes: [Option<Arc<Route>>; MAX_ROUTES] = std::array::from_fn(|_| None);
        {
            let state = self.lock();
            for (copy, source) in routes.iter_mut().zip(&state.routes) {
                *copy = source.clone();
            }
        }
        for route in routes.into_iter().flatten() {
            // LinkClosed remains routable after the execution gate closes.
            route.scope.on_event_ref(event);
            if route.scope.check_open().is_ok() {
                route.sync.on_event_ref(event);
                route.enrollment.on_event_ref(event);
            }
        }
    }
    pub(super) fn register(&self, route: Route) -> Result<Registration, Error> {
        let route = Arc::new(route);
        {
            let mut state = self.lock();
            if state
                .routes
                .iter()
                .flatten()
                .any(|existing| existing.scope.identity_hash() == route.scope.identity_hash())
            {
                return Err(Error::DuplicateIdentity);
            }
            let slot = state
                .routes
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(Error::Capacity)?;
            *slot = Some(route.clone());
        }
        Ok(Registration {
            router: self.clone(),
            route,
            removed: false,
        })
    }
}

pub(super) struct Route {
    pub dispatch: DispatchLease,
    pub scope: HostServiceScope,
    pub sync: transport::Ingress,
    pub enrollment: transport::enrollment::Ingress,
    pub sync_destination: DestinationHash,
    pub enrollment_destination: DestinationHash,
    pub _key: KeyLease,
}
/// Retain through runner stop, actual host-operation drainage and both endpoint
/// retirement barriers. Drop does not unregister: uncertain cleanup remains in
/// the router, retaining its key and native catalog lock until explicit finish.
#[must_use = "a native route must be explicitly finished after endpoint cleanup"]
pub struct Registration {
    router: Router,
    route: Arc<Route>,
    removed: bool,
}
impl Registration {
    pub fn owner(&self) -> OwnerToken {
        self.route.dispatch.owner()
    }
    pub fn scope(&self) -> &HostServiceScope {
        &self.route.scope
    }
    pub fn sync_destination(&self) -> DestinationHash {
        self.route.sync_destination
    }
    pub fn enrollment_destination(&self) -> DestinationHash {
        self.route.enrollment_destination
    }

    /// Call only after both ServiceEndpoint retirement receipts have completed.
    /// This check additionally requires the scope's own fence, settled operations
    /// and no unpublished Link results. It does not turn a Link snapshot into
    /// endpoint evidence, including after bounded closure-history overflow.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.removed {
            return Ok(());
        }
        if self.route.dispatch.is_open() {
            return Err(Error::NotDrained);
        }
        // This verifies the local fence, settled operations and no unpublished
        // Link results. Nonempty candidates can be already closed after history
        // overflow; only the caller's exact endpoint barriers prove retirement.
        self.route.scope.outgoing_links().map_err(Error::Scope)?;
        let removed = {
            let mut state = self.router.lock();
            let slot = state
                .routes
                .iter_mut()
                .find(|slot| {
                    slot.as_ref()
                        .is_some_and(|route| Arc::ptr_eq(route, &self.route))
                })
                .ok_or(Error::WrongOwner)?;
            slot.take()
        };
        // Key and host-client destruction must never run under the router lock.
        drop(removed);
        self.removed = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
