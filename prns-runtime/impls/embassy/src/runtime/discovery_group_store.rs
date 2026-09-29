use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;

use super::embedded_persistence::{DiscoveryGroupConfigurationChange, EmbeddedPersistenceFailure};
use crate::interfaces::{
    DiscoveryGroupConfigurationSnapshot, DiscoveryGroupConfigurationSnapshotError,
    DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN,
};

struct RestoredDiscoveryGroupConfiguration {
    complete: bool,
    snapshot: Option<DiscoveryGroupConfigurationSnapshot>,
}

enum DiscoveryGroupConfigurationStoreState {
    Idle,
    Pending(DiscoveryGroupConfigurationChange),
    Processing,
}

/// One node's durable group snapshot and bounded persistence mailbox.
/// Attach exactly one persistence owner; callers may share the exchange.
pub struct DiscoveryGroupConfigurationStoreExchange {
    caller: AsyncMutex<CriticalSectionRawMutex, ()>,
    state: BlockingMutex<CriticalSectionRawMutex, RefCell<DiscoveryGroupConfigurationStoreState>>,
    request_ready: Signal<CriticalSectionRawMutex, ()>,
    completed: Signal<CriticalSectionRawMutex, Result<(), EmbeddedPersistenceFailure>>,
    restored: BlockingMutex<CriticalSectionRawMutex, RefCell<RestoredDiscoveryGroupConfiguration>>,
    restored_changed: Signal<CriticalSectionRawMutex, ()>,
}

impl DiscoveryGroupConfigurationStoreExchange {
    pub const fn new() -> Self {
        Self {
            caller: AsyncMutex::new(()),
            state: BlockingMutex::new(RefCell::new(DiscoveryGroupConfigurationStoreState::Idle)),
            request_ready: Signal::new(),
            completed: Signal::new(),
            restored: BlockingMutex::new(RefCell::new(RestoredDiscoveryGroupConfiguration {
                complete: false,
                snapshot: None,
            })),
            restored_changed: Signal::new(),
        }
    }

    pub(super) fn publish_restored(&self, snapshot: Option<DiscoveryGroupConfigurationSnapshot>) {
        self.restored.lock(|state| {
            let mut state = state.borrow_mut();
            state.complete = true;
            state.snapshot = snapshot;
        });
        self.restored_changed.signal(());
    }

    /// Returns `None` until this owner's journal restoration completes.
    #[must_use]
    pub fn restored_now(&self) -> Option<DiscoveryGroupConfigurationSnapshot> {
        self.restored.lock(|state| {
            let state = state.borrow();
            state.complete.then(|| state.snapshot.unwrap_or_default())
        })
    }

    /// Returns `None` before restoration, then this interface's optional durable set.
    #[must_use]
    pub fn groups_now(
        &self,
        interface_id: crate::interfaces::InterfaceId,
    ) -> Option<Option<crate::interfaces::DiscoveryGroupSet>> {
        self.restored.lock(|state| {
            let state = state.borrow();
            state.complete.then(|| {
                state
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.groups_for(interface_id))
                    .cloned()
            })
        })
    }

    pub(super) fn encode_restored(
        &self,
        output: &mut [u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN],
    ) -> Option<usize> {
        self.restored.lock(|state| {
            let state = state.borrow();
            state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.encode_into_max(output))
        })
    }

    pub(super) fn encode_projected(
        &self,
        change: &DiscoveryGroupConfigurationChange,
        output: &mut [u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN],
    ) -> Result<usize, DiscoveryGroupConfigurationSnapshotError> {
        self.restored.lock(|state| {
            let state = state.borrow();
            let mut projected = state.snapshot.unwrap_or_default();
            change.apply_to(&mut projected)?;
            Ok(projected.encode_into_max(output))
        })
    }

    pub(super) fn commit_change(
        &self,
        change: &DiscoveryGroupConfigurationChange,
    ) -> Result<(), DiscoveryGroupConfigurationSnapshotError> {
        let result = self.restored.lock(|state| {
            let mut state = state.borrow_mut();
            let snapshot = state.snapshot.get_or_insert_default();
            change.apply_to(snapshot)
        });
        if result.is_ok() {
            self.restored_changed.signal(());
        }
        result
    }

    pub(super) fn submit(&self, change: DiscoveryGroupConfigurationChange) -> bool {
        let submitted = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            match &*state {
                DiscoveryGroupConfigurationStoreState::Idle => {
                    *state = DiscoveryGroupConfigurationStoreState::Pending(change);
                    true
                }
                DiscoveryGroupConfigurationStoreState::Pending(_)
                | DiscoveryGroupConfigurationStoreState::Processing => false,
            }
        });
        if submitted {
            self.completed.reset();
            self.request_ready.signal(());
        }
        submitted
    }

    fn take_pending(&self) -> Option<DiscoveryGroupConfigurationChange> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let current =
                core::mem::replace(&mut *state, DiscoveryGroupConfigurationStoreState::Idle);
            match current {
                DiscoveryGroupConfigurationStoreState::Pending(change) => {
                    *state = DiscoveryGroupConfigurationStoreState::Processing;
                    Some(change)
                }
                other => {
                    *state = other;
                    None
                }
            }
        })
    }

    pub(super) fn try_take_request(&self) -> Option<DiscoveryGroupConfigurationChange> {
        let pending = self.take_pending();
        if pending.is_some() {
            self.request_ready.reset();
        }
        pending
    }

    pub(super) fn has_pending_request(&self) -> bool {
        self.state.lock(|state| {
            matches!(
                &*state.borrow(),
                DiscoveryGroupConfigurationStoreState::Pending(_)
            )
        })
    }

    pub(super) async fn wait_until_request_ready(&self) {
        loop {
            if self.has_pending_request() {
                return;
            }
            self.request_ready.wait().await;
        }
    }

    pub(super) fn resignal_request(&self, change: DiscoveryGroupConfigurationChange) {
        let resignal = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if matches!(*state, DiscoveryGroupConfigurationStoreState::Processing) {
                *state = DiscoveryGroupConfigurationStoreState::Pending(change);
                true
            } else {
                false
            }
        });
        debug_assert!(
            resignal,
            "only a processing group change can be rescheduled"
        );
        if resignal {
            self.request_ready.signal(());
        }
    }

    pub(super) fn settle(&self, result: Result<(), EmbeddedPersistenceFailure>) {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if matches!(*state, DiscoveryGroupConfigurationStoreState::Processing) {
                // Publish completion before reopening admission so a cancelled caller cannot let
                // its stale result race a newer request's reset.
                self.completed.signal(result);
                *state = DiscoveryGroupConfigurationStoreState::Idle;
                true
            } else {
                false
            }
        });
        debug_assert!(settled, "only a processing group change can be settled");
    }

    #[cfg(test)]
    pub(super) fn reset_for_test(&self) {
        self.state.lock(|state| {
            *state.borrow_mut() = DiscoveryGroupConfigurationStoreState::Idle;
        });
        self.request_ready.reset();
        self.completed.reset();
        self.restored.lock(|state| {
            *state.borrow_mut() = RestoredDiscoveryGroupConfiguration {
                complete: false,
                snapshot: None,
            };
        });
        self.restored_changed.reset();
    }
}

pub(super) static DISCOVERY_GROUP_CONFIGURATION_STORES: DiscoveryGroupConfigurationStoreExchange =
    DiscoveryGroupConfigurationStoreExchange::new();

pub async fn restored_discovery_group_configuration() -> DiscoveryGroupConfigurationSnapshot {
    loop {
        if let Some(snapshot) = DISCOVERY_GROUP_CONFIGURATION_STORES.restored_now() {
            return snapshot;
        }
        DISCOVERY_GROUP_CONFIGURATION_STORES
            .restored_changed
            .wait()
            .await;
    }
}

#[must_use]
pub fn restored_discovery_group_configuration_now() -> Option<DiscoveryGroupConfigurationSnapshot> {
    DISCOVERY_GROUP_CONFIGURATION_STORES.restored_now()
}

pub async fn restored_discovery_groups(
    interface_id: crate::interfaces::InterfaceId,
) -> Option<crate::interfaces::DiscoveryGroupSet> {
    loop {
        if let Some(groups) = DISCOVERY_GROUP_CONFIGURATION_STORES.groups_now(interface_id) {
            return groups;
        }
        DISCOVERY_GROUP_CONFIGURATION_STORES
            .restored_changed
            .wait()
            .await;
    }
}

/// Returns `None` until journal restoration is complete, then the optional durable set.
#[must_use]
pub fn restored_discovery_groups_now(
    interface_id: crate::interfaces::InterfaceId,
) -> Option<Option<crate::interfaces::DiscoveryGroupSet>> {
    DISCOVERY_GROUP_CONFIGURATION_STORES.groups_now(interface_id)
}

pub fn store_discovery_group_configuration(
    change: DiscoveryGroupConfigurationChange,
) -> impl core::future::Future<Output = Result<(), EmbeddedPersistenceFailure>> + Unpin {
    let caller = DISCOVERY_GROUP_CONFIGURATION_STORES.caller.try_lock().ok();
    let admitted = caller.is_some() && DISCOVERY_GROUP_CONFIGURATION_STORES.submit(change);
    let immediate_failure = (!admitted).then_some(EmbeddedPersistenceFailure::Capacity);
    DiscoveryGroupConfigurationStoreFuture {
        _caller: caller,
        immediate_failure,
        exchange: GlobalDiscoveryGroupStore,
    }
}

struct DiscoveryGroupConfigurationStoreFuture<'a, Store> {
    _caller: Option<embassy_sync::mutex::MutexGuard<'a, CriticalSectionRawMutex, ()>>,
    immediate_failure: Option<EmbeddedPersistenceFailure>,
    exchange: Store,
}

impl<Store: AsRef<DiscoveryGroupConfigurationStoreExchange>> core::future::Future
    for DiscoveryGroupConfigurationStoreFuture<'_, Store>
{
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

/// The existing single-board exchange, stored without a per-owner pointer.
pub struct GlobalDiscoveryGroupStore;

impl AsRef<DiscoveryGroupConfigurationStoreExchange> for GlobalDiscoveryGroupStore {
    fn as_ref(&self) -> &DiscoveryGroupConfigurationStoreExchange {
        &DISCOVERY_GROUP_CONFIGURATION_STORES
    }
}

impl AsRef<DiscoveryGroupConfigurationStoreExchange> for DiscoveryGroupConfigurationStoreExchange {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl Default for DiscoveryGroupConfigurationStoreExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryGroupConfigurationStoreExchange {
    /// Admits immediately, or resolves to `Capacity` while another change is outstanding.
    /// Dropping the returned future does not cancel an admitted durable write.
    pub fn store(
        &self,
        change: DiscoveryGroupConfigurationChange,
    ) -> impl core::future::Future<Output = Result<(), EmbeddedPersistenceFailure>> + Unpin + '_
    {
        let caller = self.caller.try_lock().ok();
        let admitted = caller.is_some() && self.submit(change);
        DiscoveryGroupConfigurationStoreFuture {
            _caller: caller,
            immediate_failure: (!admitted).then_some(EmbeddedPersistenceFailure::Capacity),
            exchange: self,
        }
    }
}
