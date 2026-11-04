//! Host service admission and initiator-Link ownership on one borrowed host.
//!
//! A scope is an execution fence, not an authorization credential. Its embedding
//! validates the durable lifecycle and registers the listed destinations before
//! constructing it. Never reopen a fenced scope; a restored generation gets a
//! fresh scope. Shared interfaces and the host itself are not owned here.

use crate::owner::{HostClient, SessionError};
use personal_rns::runtime::{Diagnostic, PrnsEvent};
use prns_host::{
    CommandFailure, CommandOutcome, DestinationHash, HostCommand, IdentityHash, LinkId,
};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostServiceScopeError {
    InvalidLimits,
    Capacity,
    Fenced,
    /// Closure history overflowed while an establishment result was pending.
    /// Retire this scope through its endpoint barrier, then create a fresh one.
    RecoveryRequired,
    NotDrained,
    WrongOwner,
    Session(SessionError),
}

struct State {
    accepting: bool,
    operations: usize,
    links: Vec<Option<Option<LinkId>>>,
    recently_closed: VecDeque<LinkId>,
    closure_history_lost: bool,
}

impl State {
    fn check_open(&self) -> Result<(), HostServiceScopeError> {
        if self.closure_history_lost {
            Err(HostServiceScopeError::RecoveryRequired)
        } else if self.accepting {
            Ok(())
        } else {
            Err(HostServiceScopeError::Fenced)
        }
    }

    fn has_unpublished_links(&self) -> bool {
        self.links.iter().any(|slot| matches!(slot, Some(None)))
    }

    fn clear_idle_closure_history(&mut self) {
        if !self.has_unpublished_links() {
            self.recently_closed.clear();
        }
    }
}

struct Shared {
    state: Mutex<State>,
    drained: Notify,
    maximum_operations: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An additional restrictive view supplied by a trusted native lifecycle owner.
/// It grants no authority and must never acquire resources or invoke foreign
/// callbacks. Checks are bounded, nonblocking, and run outside the scope lock.
/// A restored generation gets a new gate; an old gate must never reopen.
pub trait HostDispatchGate: Send + Sync {
    fn is_open(&self) -> bool;
}

/// Cloneable handle for one immutable service identity and generation.
#[derive(Clone)]
pub struct HostServiceScope {
    host: HostClient,
    identity: IdentityHash,
    destinations: Arc<[DestinationHash]>,
    shared: Arc<Shared>,
    dispatch: Option<Arc<dyn HostDispatchGate>>,
}

impl HostServiceScope {
    pub fn new(
        host: HostClient,
        identity: IdentityHash,
        destinations: Vec<DestinationHash>,
        maximum_operations: usize,
        maximum_links: usize,
    ) -> Result<Self, HostServiceScopeError> {
        Self::build(
            host,
            identity,
            destinations,
            maximum_operations,
            maximum_links,
            None,
        )
    }

    /// Bind the scope to the exact external owner's dispatch fence. The gate
    /// may initially be staged closed. Closing it does not release operations,
    /// outgoing links, or the endpoint's cleanup/LinkClosed routing.
    pub fn new_restricted(
        host: HostClient,
        identity: IdentityHash,
        destinations: Vec<DestinationHash>,
        maximum_operations: usize,
        maximum_links: usize,
        dispatch: Arc<dyn HostDispatchGate>,
    ) -> Result<Self, HostServiceScopeError> {
        Self::build(
            host,
            identity,
            destinations,
            maximum_operations,
            maximum_links,
            Some(dispatch),
        )
    }

    /// Validate the canonical owned operation/Link bounds before an embedding
    /// creates files or other resources.
    pub fn validate_capacity(
        maximum_operations: usize,
        maximum_links: usize,
    ) -> Result<(), HostServiceScopeError> {
        if maximum_operations == 0
            || maximum_operations > 1024
            || maximum_links == 0
            || maximum_links > 1024
        {
            Err(HostServiceScopeError::InvalidLimits)
        } else {
            Ok(())
        }
    }

    fn build(
        host: HostClient,
        identity: IdentityHash,
        destinations: Vec<DestinationHash>,
        maximum_operations: usize,
        maximum_links: usize,
        dispatch: Option<Arc<dyn HostDispatchGate>>,
    ) -> Result<Self, HostServiceScopeError> {
        Self::validate_capacity(maximum_operations, maximum_links)?;
        if destinations.is_empty()
            || destinations.len() > 64
            || destinations
                .iter()
                .enumerate()
                .any(|(index, item)| destinations[..index].contains(item))
        {
            return Err(HostServiceScopeError::InvalidLimits);
        }
        Ok(Self {
            host,
            identity,
            destinations: destinations.into(),
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    accepting: true,
                    operations: 0,
                    links: vec![None; maximum_links],
                    recently_closed: VecDeque::with_capacity(maximum_links),
                    closure_history_lost: false,
                }),
                drained: Notify::new(),
                maximum_operations,
            }),
            dispatch,
        })
    }

    /// Borrowed host for read-only queries. Mutations must use the scoped APIs.
    pub fn host(&self) -> &HostClient {
        &self.host
    }
    pub const fn identity_hash(&self) -> IdentityHash {
        self.identity
    }
    pub fn contains_destination(&self, destination: DestinationHash) -> bool {
        self.destinations.contains(&destination)
    }
    fn check_dispatch(&self) -> Result<(), HostServiceScopeError> {
        if self.dispatch.as_ref().is_some_and(|gate| !gate.is_open()) {
            Err(HostServiceScopeError::Fenced)
        } else {
            Ok(())
        }
    }
    pub fn check_open(&self) -> Result<(), HostServiceScopeError> {
        self.check_dispatch()?;
        self.shared.lock().check_open()
    }
    pub fn is_fenced(&self) -> bool {
        self.check_dispatch().is_err() || !self.shared.lock().accepting
    }

    /// Close admission immediately. Existing admitted operations remain owned
    /// until their actual host-side future completes, even after waiter loss.
    pub fn fence(&self) {
        self.shared.lock().accepting = false;
    }

    pub async fn wait_drained(&self) {
        loop {
            let notified = self.shared.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.shared.lock().operations == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Snapshot for the endpoint's actual Link/egress teardown barrier. Merely
    /// returning these IDs does not prove they have closed or left the queues.
    /// After closure-history overflow this conservatively includes candidates
    /// whose closure could not be recorded; endpoint retirement must handle them.
    pub fn outgoing_links(&self) -> Result<Vec<LinkId>, HostServiceScopeError> {
        let state = self.shared.lock();
        if state.accepting || state.operations != 0 || state.has_unpublished_links() {
            return Err(HostServiceScopeError::NotDrained);
        }
        Ok(state
            .links
            .iter()
            .filter_map(|slot| slot.flatten())
            .collect())
    }

    /// Feed the host lifecycle events to release bounded slots after an actual
    /// LinkClosed event, never merely after submitting a close command.
    pub fn on_event_ref(&self, event: &PrnsEvent<'_>) {
        if let PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, .. }) = event {
            let link = LinkId::new(*link_id.as_bytes());
            let mut state = self.shared.lock();
            for slot in &mut state.links {
                if *slot == Some(Some(link)) {
                    *slot = None;
                }
            }
            // A Link can close before its establishment waiter sees success.
            // Retain history only during that publication window. These events
            // may include other services' Links, so never evict and silently
            // lose closure evidence: bounded overflow requires scope recovery.
            if !state.has_unpublished_links() {
                state.recently_closed.clear();
            } else if !state.recently_closed.contains(&link) {
                if state.recently_closed.len() == state.links.len() {
                    state.closure_history_lost = true;
                    state.accepting = false;
                } else {
                    state.recently_closed.push_back(link);
                }
            }
        }
    }

    pub fn owns_link(&self, link: LinkId) -> bool {
        self.shared.lock().links.contains(&Some(Some(link)))
    }

    pub fn reserve_link(&self) -> Result<HostServiceLinkReservation, HostServiceScopeError> {
        self.check_dispatch()?;
        let mut state = self.shared.lock();
        state.check_open()?;
        state.clear_idle_closure_history();
        let index = state
            .links
            .iter()
            .position(Option::is_none)
            .ok_or(HostServiceScopeError::Capacity)?;
        state.links[index] = Some(None);
        Ok(HostServiceLinkReservation {
            shared: Arc::clone(&self.shared),
            index,
            published: false,
        })
    }

    fn admit(&self) -> Result<Operation, HostServiceScopeError> {
        self.check_dispatch()?;
        let mut state = self.shared.lock();
        state.check_open()?;
        if state.operations >= self.shared.maximum_operations {
            return Err(HostServiceScopeError::Capacity);
        }
        state.operations += 1;
        Ok(Operation(Arc::clone(&self.shared)))
    }

    /// Fence a synchronous queue submission and account for it through return.
    /// The endpoint teardown barrier must subsequently drain/purge queued bytes.
    pub fn with_admission<T>(
        &self,
        operation: impl FnOnce() -> T,
    ) -> Result<T, HostServiceScopeError> {
        let _lease = self.admit()?;
        Ok(operation())
    }

    /// Execute on the host's bounded operation lane. The lease travels inside
    /// that lane, so cancelling this waiter does not falsely report a drain.
    /// The callback must use this scope's identity, destinations and reserved
    /// Link slots; this low-level API does not authorize arbitrary node effects.
    pub async fn protocol_operation<T, F, Fut>(
        &self,
        operation: F,
    ) -> Result<T, HostServiceScopeError>
    where
        T: Send + 'static,
        F: FnOnce(personal_rns::PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let lease = self.admit()?;
        let scope = self.clone();
        self.host
            .protocol_operation(move |protocol| async move {
                let _lease = lease;
                scope.check_open()?;
                Ok(operation(protocol).await)
            })
            .await
            .map_err(HostServiceScopeError::Session)?
    }

    /// Scoped host commands. Infrastructure mutation and foreign local
    /// destinations/Links are rejected before submission and again at execution.
    pub async fn execute(
        &self,
        command: HostCommand,
    ) -> Result<Result<CommandOutcome, CommandFailure>, HostServiceScopeError> {
        self.validate_command(&command)?;
        let reservation = if matches!(command, HostCommand::EstablishLink { .. }) {
            Some(self.reserve_link()?)
        } else {
            None
        };
        let scope = self.clone();
        let (result, mut cleanup) = self
            .protocol_operation(move |_| async move {
                scope.validate_command(&command)?;
                let result = scope
                    .host
                    .execute(command)
                    .await
                    .map_err(HostServiceScopeError::Session)?;
                let cleanup = if let (
                    Some(reservation),
                    Ok(CommandOutcome::LinkEstablished { link_id, .. }),
                ) = (reservation, &result)
                {
                    reservation.track(*link_id);
                    Some(UndeliveredLink {
                        host: scope.host.clone(),
                        link: Some(*link_id),
                    })
                } else {
                    None
                };
                Ok((result, cleanup))
            })
            .await??;
        // The host owns this guard until the caller actually receives success.
        // A cancelled waiter cannot strand a late Link outside its bounded scope.
        if let Some(cleanup) = &mut cleanup {
            cleanup.link = None;
        }
        Ok(result)
    }

    fn validate_command(&self, command: &HostCommand) -> Result<(), HostServiceScopeError> {
        self.check_open()?;
        let owned = match command {
            HostCommand::EstablishLink { .. }
            | HostCommand::RequestPath { .. }
            | HostCommand::SendSinglePacket { .. } => true,
            HostCommand::Announce { destination, .. }
            | HostCommand::AllowRequester { destination, .. }
            | HostCommand::SetDestinationResourceStrategy { destination, .. } => {
                self.contains_destination(*destination)
            }
            HostCommand::Identify { link_id, identity } => {
                *identity == self.identity && self.owns_link(*link_id)
            }
            HostCommand::CloseLink { link_id }
            | HostCommand::SendLinkPacket { link_id, .. }
            | HostCommand::Request { link_id, .. }
            | HostCommand::Respond { link_id, .. }
            | HostCommand::SendResource { link_id, .. }
            | HostCommand::SetLinkResourceStrategy { link_id, .. }
            | HostCommand::SendChannelMessage { link_id, .. } => self.owns_link(*link_id),
            _ => false,
        };
        if owned {
            Ok(())
        } else {
            Err(HostServiceScopeError::WrongOwner)
        }
    }
}

struct UndeliveredLink {
    host: HostClient,
    link: Option<LinkId>,
}
impl Drop for UndeliveredLink {
    fn drop(&mut self) {
        if let Some(link) = self.link.take() {
            if let Ok(services) = self.host.service_client() {
                let _queued = services
                    .protocols()
                    .close_link(personal_rns::routing::links::LinkId::new(link.into_bytes()));
            }
        }
    }
}

struct Operation(Arc<Shared>);
impl Drop for Operation {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.operations -= 1;
        if state.operations == 0 {
            self.0.drained.notify_waiters();
        }
    }
}

/// Reserve before dispatching establishment; late successes must publish here
/// even if the original caller has stopped waiting. Dropping an unpublished
/// reservation releases only its empty slot, never an established Link.
pub struct HostServiceLinkReservation {
    shared: Arc<Shared>,
    index: usize,
    published: bool,
}
impl HostServiceLinkReservation {
    pub fn track(mut self, link: LinkId) {
        let mut state = self.shared.lock();
        state.links[self.index] = if state.recently_closed.contains(&link) {
            None
        } else {
            Some(Some(link))
        };
        state.clear_idle_closure_history();
        self.published = true;
    }
}
impl Drop for HostServiceLinkReservation {
    fn drop(&mut self) {
        if !self.published {
            let mut state = self.shared.lock();
            state.links[self.index] = None;
            state.clear_idle_closure_history();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::owner::OwnedSession;
    use prns_host::{HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits};
    use std::time::Duration;

    async fn host() -> Arc<OwnedSession> {
        OwnedSession::open(HostConfig {
            identity: IdentityConfig::GenerateEphemeral,
            persistence: PersistenceConfig::Ephemeral,
            role: HostRole::Endpoint,
            destinations: vec![],
            required_capabilities: vec![],
            limits: PrnsLimits::balanced(),
        })
        .await
        .unwrap()
    }
    fn scope(host: &OwnedSession, byte: u8) -> HostServiceScope {
        HostServiceScope::new(
            host.client(),
            IdentityHash::new([byte; 16]),
            vec![DestinationHash::new([byte; 16])],
            2,
            1,
        )
        .unwrap()
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn closed(scope: &HostServiceScope, link: LinkId) {
        scope.on_event_ref(&PrnsEvent::Diagnostic(Diagnostic::LinkClosed {
            link_id: personal_rns::routing::links::LinkId::new(link.into_bytes()),
            reason: personal_rns::engine::LinkClosedReason::LocallyClosed,
        }));
    }

    #[test]
    fn early_closure_history_overflow_fences_only_its_scope_until_recovery() {
        runtime().block_on(async {
            let host = host().await;
            let first = scope(&host, 1);
            let other = scope(&host, 2);
            // Idle history must not consume the bounded publication window.
            for byte in 3..10 {
                closed(&first, LinkId::new([byte; 16]));
            }
            assert_eq!(first.check_open(), Ok(()));
            let reservation = first.reserve_link().unwrap();
            let unrelated = LinkId::new([10; 16]);
            closed(&first, unrelated);
            closed(&first, unrelated);
            assert_eq!(first.check_open(), Ok(()));

            // The actual Link closes before its delayed establishment result.
            // Its ID cannot fit, so admitting further work would hide a leak.
            let link = LinkId::new([11; 16]);
            closed(&first, link);
            assert!(first.is_fenced());
            assert_eq!(
                first.check_open(),
                Err(HostServiceScopeError::RecoveryRequired)
            );
            assert!(matches!(
                first.reserve_link(),
                Err(HostServiceScopeError::RecoveryRequired)
            ));
            assert_eq!(
                first.protocol_operation(|_| async {}).await,
                Err(HostServiceScopeError::RecoveryRequired)
            );
            assert_eq!(
                first
                    .execute(HostCommand::CloseLink { link_id: link })
                    .await,
                Err(HostServiceScopeError::RecoveryRequired)
            );
            assert_eq!(
                first.outgoing_links(),
                Err(HostServiceScopeError::NotDrained)
            );

            // Preserve the late candidate so normal endpoint retirement can
            // purge its queued frames even though its close event was lost.
            reservation.track(link);
            first.wait_drained().await;
            assert_eq!(first.outgoing_links().unwrap(), vec![link]);
            assert_eq!(other.protocol_operation(|_| async { 7 }).await.unwrap(), 7);
            closed(&first, link);
            assert!(first.outgoing_links().unwrap().is_empty());
            assert_eq!(
                first.check_open(),
                Err(HostServiceScopeError::RecoveryRequired)
            );
            host.stop().await.unwrap();
        });
    }

    #[test]
    fn cancelled_waiter_does_not_release_admitted_operation_or_other_owner() {
        runtime().block_on(async {
            let host = host().await;
            let first = scope(&host, 1);
            let other = scope(&host, 2);
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (finish, wait) = tokio::sync::oneshot::channel();
            let actor = first.clone();
            let task = tokio::spawn(async move {
                actor
                    .protocol_operation(move |_| async move {
                        let _ = entered.send(());
                        let _ = wait.await;
                    })
                    .await
            });
            ready.await.unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            first.fence();
            assert_eq!(
                first.outgoing_links(),
                Err(HostServiceScopeError::NotDrained)
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(20), first.wait_drained())
                    .await
                    .is_err()
            );
            assert_eq!(other.protocol_operation(|_| async { 7 }).await.unwrap(), 7);
            assert_eq!(
                first.protocol_operation(|_| async {}).await,
                Err(HostServiceScopeError::Fenced)
            );
            finish.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), first.wait_drained())
                .await
                .unwrap();
            assert!(first.outgoing_links().unwrap().is_empty());
            host.stop().await.unwrap();
        });
    }

    #[test]
    fn link_slots_follow_actual_closure_and_scope_commands_reject_foreign_owners() {
        runtime().block_on(async {
            let host = host().await;
            let first = scope(&host, 1);
            let other = scope(&host, 2);
            let link = LinkId::new([9; 16]);
            let reserved = first.reserve_link().unwrap();
            assert!(matches!(
                first.reserve_link(),
                Err(HostServiceScopeError::Capacity)
            ));
            first.fence();
            assert_eq!(
                first.outgoing_links(),
                Err(HostServiceScopeError::NotDrained)
            );
            reserved.track(link);
            assert_eq!(first.outgoing_links().unwrap(), vec![link]);
            assert_eq!(
                other
                    .execute(HostCommand::CloseLink { link_id: link })
                    .await,
                Err(HostServiceScopeError::WrongOwner)
            );
            assert_eq!(
                other
                    .execute(HostCommand::Announce {
                        destination: DestinationHash::new([1; 16]),
                        interface: None
                    })
                    .await,
                Err(HostServiceScopeError::WrongOwner)
            );
            first.on_event_ref(&PrnsEvent::Diagnostic(Diagnostic::LinkClosed {
                link_id: personal_rns::routing::links::LinkId::new(link.into_bytes()),
                reason: personal_rns::engine::LinkClosedReason::LocallyClosed,
            }));
            assert!(first.outgoing_links().unwrap().is_empty());
            let reservation = other.reserve_link().unwrap();
            other.on_event_ref(&PrnsEvent::Diagnostic(Diagnostic::LinkClosed {
                link_id: personal_rns::routing::links::LinkId::new(link.into_bytes()),
                reason: personal_rns::engine::LinkClosedReason::LocallyClosed,
            }));
            reservation.track(link);
            assert!(!other.owns_link(link));
            assert!(other.reserve_link().is_ok());
            host.stop().await.unwrap();
        });
    }

    #[test]
    fn restricted_gate_is_checked_at_dispatch_outside_scope_locks() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Gate {
            checks: AtomicUsize,
            state: Mutex<Option<std::sync::Weak<Shared>>>,
        }
        impl HostDispatchGate for Gate {
            fn is_open(&self) -> bool {
                let shared = self
                    .state
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade);
                if let Some(shared) = shared {
                    // A native gate can inspect its embedding; never call it
                    // while holding the scope's own resource lock.
                    assert!(shared.state.try_lock().is_ok());
                }
                self.checks.fetch_add(1, Ordering::SeqCst) == 0
            }
        }
        runtime().block_on(async {
            let host = host().await;
            let gate = Arc::new(Gate {
                checks: AtomicUsize::new(0),
                state: Mutex::new(None),
            });
            let scope = HostServiceScope::new_restricted(
                host.client(),
                IdentityHash::new([1; 16]),
                vec![DestinationHash::new([1; 16])],
                2,
                1,
                gate.clone(),
            )
            .unwrap();
            *gate.state.lock().unwrap() = Some(Arc::downgrade(&scope.shared));
            // Admission observes open; actual host-lane dispatch observes closed.
            let executed = Arc::new(AtomicUsize::new(0));
            let inside = executed.clone();
            assert_eq!(
                scope
                    .protocol_operation(move |_| async move {
                        inside.fetch_add(1, Ordering::SeqCst);
                    })
                    .await,
                Err(HostServiceScopeError::Fenced)
            );
            assert_eq!(executed.load(Ordering::SeqCst), 0);
            assert_eq!(gate.checks.load(Ordering::SeqCst), 2);
            scope.fence();
            scope.wait_drained().await;
            assert!(scope.outgoing_links().unwrap().is_empty());
            host.stop().await.unwrap();
        });
    }

    #[test]
    fn restricted_gate_denies_new_work_but_preserves_link_cleanup() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Gate(AtomicBool);
        impl HostDispatchGate for Gate {
            fn is_open(&self) -> bool {
                self.0.load(Ordering::Acquire)
            }
        }
        runtime().block_on(async {
            let host = host().await;
            let gate = Arc::new(Gate(AtomicBool::new(false)));
            let first = HostServiceScope::new_restricted(
                host.client(),
                IdentityHash::new([1; 16]),
                vec![DestinationHash::new([1; 16])],
                2,
                1,
                gate.clone(),
            )
            .unwrap();
            let other = scope(&host, 2);
            assert!(first.is_fenced());
            assert!(matches!(
                first.reserve_link(),
                Err(HostServiceScopeError::Fenced)
            ));
            gate.0.store(true, Ordering::Release);
            let reservation = first.reserve_link().unwrap();
            let link = LinkId::new([9; 16]);
            gate.0.store(false, Ordering::Release);
            assert!(first.is_fenced());
            assert!(matches!(
                first.with_admission(|| ()),
                Err(HostServiceScopeError::Fenced)
            ));
            // An already accepted late establishment remains owned after fencing.
            reservation.track(link);
            first.fence();
            first.wait_drained().await;
            assert_eq!(first.outgoing_links().unwrap(), vec![link]);
            closed(&first, link);
            assert!(first.outgoing_links().unwrap().is_empty());
            assert_eq!(other.protocol_operation(|_| async { 7 }).await.unwrap(), 7);
            host.stop().await.unwrap();
        });
    }
}
