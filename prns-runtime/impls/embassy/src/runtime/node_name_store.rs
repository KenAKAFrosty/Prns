use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;

use super::embedded_persistence::EmbeddedPersistenceFailure;
use crate::remote_control::{RemoteControlNodeName, NODE_NAME_SNAPSHOT_MAX_LEN};

struct RestoredNodeName {
    complete: bool,
    name: Option<RemoteControlNodeName>,
}

enum NodeNameStoreState {
    Idle,
    Pending(RemoteControlNodeName),
    Processing,
}

/// The node's durable display name and its single-slot persistence mailbox.
///
/// Mirrors the discovery-group exchange: one caller is admitted at a time, the embedded
/// persistence owner appends the record, and completion is published before admission reopens.
pub struct NodeNameStoreExchange {
    caller: AsyncMutex<CriticalSectionRawMutex, ()>,
    state: BlockingMutex<CriticalSectionRawMutex, RefCell<NodeNameStoreState>>,
    request_ready: Signal<CriticalSectionRawMutex, ()>,
    completed: Signal<CriticalSectionRawMutex, Result<(), EmbeddedPersistenceFailure>>,
    restored: BlockingMutex<CriticalSectionRawMutex, RefCell<RestoredNodeName>>,
    restored_changed: Signal<CriticalSectionRawMutex, ()>,
}

impl NodeNameStoreExchange {
    pub const fn new() -> Self {
        Self {
            caller: AsyncMutex::new(()),
            state: BlockingMutex::new(RefCell::new(NodeNameStoreState::Idle)),
            request_ready: Signal::new(),
            completed: Signal::new(),
            restored: BlockingMutex::new(RefCell::new(RestoredNodeName {
                complete: false,
                name: None,
            })),
            restored_changed: Signal::new(),
        }
    }

    pub fn store(
        &self,
        name: RemoteControlNodeName,
    ) -> impl core::future::Future<Output = Result<(), EmbeddedPersistenceFailure>> + Unpin + '_
    {
        let caller = self.caller.try_lock().ok();
        let admitted = caller.is_some() && self.submit(name);
        NodeNameStoreFuture {
            _caller: caller,
            immediate_failure: (!admitted).then_some(EmbeddedPersistenceFailure::Capacity),
            exchange: self,
        }
    }

    pub(super) fn publish_restored(&self, name: Option<RemoteControlNodeName>) {
        self.restored.lock(|state| {
            let mut state = state.borrow_mut();
            state.complete = true;
            state.name = name;
        });
        self.restored_changed.signal(());
    }

    /// `None` until restoration completes, then the optional durable name.
    #[must_use]
    pub fn restored_now(&self) -> Option<Option<RemoteControlNodeName>> {
        self.restored.lock(|state| {
            let state = state.borrow();
            state.complete.then_some(state.name)
        })
    }

    pub(super) fn encode_restored(
        &self,
        output: &mut [u8; NODE_NAME_SNAPSHOT_MAX_LEN],
    ) -> Option<usize> {
        self.restored
            .lock(|state| state.borrow().name.map(|name| name.encode_snapshot(output)))
    }

    pub(super) fn commit(&self, name: RemoteControlNodeName) {
        self.restored
            .lock(|state| state.borrow_mut().name = Some(name));
        self.restored_changed.signal(());
    }

    fn submit(&self, name: RemoteControlNodeName) -> bool {
        let submitted = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            match &*state {
                NodeNameStoreState::Idle => {
                    *state = NodeNameStoreState::Pending(name);
                    true
                }
                NodeNameStoreState::Pending(_) | NodeNameStoreState::Processing => false,
            }
        });
        if submitted {
            self.completed.reset();
            self.request_ready.signal(());
        }
        submitted
    }

    pub(super) fn try_take_request(&self) -> Option<RemoteControlNodeName> {
        let pending = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            match core::mem::replace(&mut *state, NodeNameStoreState::Idle) {
                NodeNameStoreState::Pending(name) => {
                    *state = NodeNameStoreState::Processing;
                    Some(name)
                }
                other => {
                    *state = other;
                    None
                }
            }
        });
        if pending.is_some() {
            self.request_ready.reset();
        }
        pending
    }

    pub(super) fn has_pending_request(&self) -> bool {
        self.state
            .lock(|state| matches!(&*state.borrow(), NodeNameStoreState::Pending(_)))
    }

    pub(super) async fn wait_until_request_ready(&self) {
        loop {
            if self.has_pending_request() {
                return;
            }
            self.request_ready.wait().await;
        }
    }

    pub(super) fn resignal_request(&self, name: RemoteControlNodeName) {
        let resignal = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if matches!(*state, NodeNameStoreState::Processing) {
                *state = NodeNameStoreState::Pending(name);
                true
            } else {
                false
            }
        });
        debug_assert!(resignal, "only a processing name change can be rescheduled");
        if resignal {
            self.request_ready.signal(());
        }
    }

    pub(super) fn settle(&self, result: Result<(), EmbeddedPersistenceFailure>) {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if matches!(*state, NodeNameStoreState::Processing) {
                // Publish completion before reopening admission, as the group exchange does.
                self.completed.signal(result);
                *state = NodeNameStoreState::Idle;
                true
            } else {
                false
            }
        });
        debug_assert!(settled, "only a processing name change can be settled");
    }

    #[cfg(test)]
    pub(super) fn reset_for_test(&self) {
        self.state
            .lock(|state| *state.borrow_mut() = NodeNameStoreState::Idle);
        self.request_ready.reset();
        self.completed.reset();
        self.restored.lock(|state| {
            *state.borrow_mut() = RestoredNodeName {
                complete: false,
                name: None,
            };
        });
        self.restored_changed.reset();
    }
}

impl Default for NodeNameStoreExchange {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) static NODE_NAME_STORE: NodeNameStoreExchange = NodeNameStoreExchange::new();

/// Waits for journal restoration, then returns the durable name if one was ever stored.
pub async fn restored_node_name() -> Option<RemoteControlNodeName> {
    loop {
        if let Some(name) = NODE_NAME_STORE.restored_now() {
            return name;
        }
        NODE_NAME_STORE.restored_changed.wait().await;
    }
}

/// `None` until restoration completes, then the optional durable name.
#[must_use]
pub fn restored_node_name_now() -> Option<Option<RemoteControlNodeName>> {
    NODE_NAME_STORE.restored_now()
}

/// Durably stores the node name. Resolves to `Capacity` while another change is outstanding;
/// dropping the future does not cancel an admitted write.
pub fn store_node_name(
    name: RemoteControlNodeName,
) -> impl core::future::Future<Output = Result<(), EmbeddedPersistenceFailure>> + Unpin {
    let caller = NODE_NAME_STORE.caller.try_lock().ok();
    let admitted = caller.is_some() && NODE_NAME_STORE.submit(name);
    NodeNameStoreFuture {
        _caller: caller,
        immediate_failure: (!admitted).then_some(EmbeddedPersistenceFailure::Capacity),
        exchange: GlobalNodeNameStore,
    }
}

struct NodeNameStoreFuture<'a, Store> {
    _caller: Option<embassy_sync::mutex::MutexGuard<'a, CriticalSectionRawMutex, ()>>,
    immediate_failure: Option<EmbeddedPersistenceFailure>,
    exchange: Store,
}

impl<Store: AsRef<NodeNameStoreExchange>> core::future::Future for NodeNameStoreFuture<'_, Store> {
    type Output = Result<(), EmbeddedPersistenceFailure>;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        if let Some(failure) = self.immediate_failure {
            return core::task::Poll::Ready(Err(failure));
        }
        let completed = self.exchange.as_ref().completed.wait();
        let mut completed = core::pin::pin!(completed);
        completed.as_mut().poll(context)
    }
}

/// The single-board exchange without a per-owner pointer.
pub struct GlobalNodeNameStore;

impl AsRef<NodeNameStoreExchange> for GlobalNodeNameStore {
    fn as_ref(&self) -> &NodeNameStoreExchange {
        &NODE_NAME_STORE
    }
}

impl AsRef<NodeNameStoreExchange> for NodeNameStoreExchange {
    fn as_ref(&self) -> &Self {
        self
    }
}
