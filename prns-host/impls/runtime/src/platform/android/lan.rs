//! One generation-bound Android DNS-SD session shared with the native service.

use personal_rns::wifi_auto::{
    AutoWifi, AutoWifiStatus, HostLanInventory, PlatformServiceDiscoveryBridge,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

struct State {
    revision: u64,
    next_generation: u64,
    current: Option<Arc<LanSession>>,
}
struct Owner {
    state: Mutex<State>,
    changed: Condvar,
}
fn owner() -> &'static Owner {
    static OWNER: OnceLock<Owner> = OnceLock::new();
    OWNER.get_or_init(|| Owner {
        state: Mutex::new(State {
            revision: 0,
            next_generation: 1,
            current: None,
        }),
        changed: Condvar::new(),
    })
}

pub struct LanSession {
    pub generation: u64,
    pub bridge: PlatformServiceDiscoveryBridge,
    pub inventory: HostLanInventory,
    retiring: AtomicBool,
    stopped: AtomicBool,
    changed: tokio::sync::Notify,
    detail: Mutex<Option<String>>,
    network_revision: AtomicU64,
    status: Mutex<Option<AutoWifiStatus>>,
    discovery_readiness: Mutex<DiscoveryReadiness>,
}

#[derive(Default)]
struct DiscoveryReadiness {
    session: u64,
    active: bool,
    tcp_browse_started: bool,
}

impl DiscoveryReadiness {
    fn observe_session(&mut self, session: u64, active: bool) {
        if active && session > self.session {
            self.session = session;
            self.active = true;
            self.tcp_browse_started = false;
        } else if !active && session == self.session {
            self.active = false;
            self.tcp_browse_started = false;
        }
    }

    fn observe_tcp_browse_started(&mut self, session: u64) {
        if self.active && session == self.session {
            self.tcp_browse_started = true;
        }
    }

    fn ready(&self) -> bool {
        self.active && self.tcp_browse_started
    }
}

pub fn prepare() -> Result<(AutoWifi, Arc<LanSession>), prns_host::CommandFailure> {
    let mut state = owner()
        .state
        .lock()
        .map_err(|_| unavailable("Android LAN owner is unavailable"))?;
    if state.current.is_some() {
        return Err(unavailable(
            "The previous Android LAN discovery session is still stopping",
        ));
    }
    let session = Arc::new(LanSession {
        generation: state.next_generation,
        bridge: PlatformServiceDiscoveryBridge::new(),
        inventory: HostLanInventory::authoritative(),
        retiring: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        changed: tokio::sync::Notify::new(),
        detail: Mutex::new(None),
        network_revision: AtomicU64::new(0),
        status: Mutex::new(None),
        discovery_readiness: Mutex::new(DiscoveryReadiness::default()),
    });
    let discovery = session
        .bridge
        .take_service_discovery()
        .map_err(|error| unavailable(&error.to_string()))?;
    let interface = AutoWifi::new()
        .with_host_discovery(discovery)
        .with_host_lan_inventory(session.inventory.clone());
    if let Ok(mut status) = session.status.lock() {
        *status = Some(interface.status());
    }
    state.next_generation = state.next_generation.saturating_add(1);
    state.current = Some(Arc::clone(&session));
    state.revision = state.revision.saturating_add(1);
    owner().changed.notify_all();
    Ok((interface, session))
}

fn unavailable(detail: &str) -> prns_host::CommandFailure {
    prns_host::CommandFailure::DeviceUnavailable {
        detail: detail.into(),
    }
}

impl LanSession {
    pub fn is_retiring(&self) -> bool {
        self.retiring.load(Ordering::Acquire)
    }
    pub fn active(&self) -> bool {
        !self.is_retiring() && !self.stopped.load(Ordering::Acquire)
    }
    pub fn set_detail(&self, detail: Option<String>) {
        if let Ok(mut current) = self.detail.lock() {
            *current = detail;
        }
    }
    pub fn network_changed(&self) {
        self.network_revision.fetch_add(1, Ordering::AcqRel);
        self.bridge.wake_waiters();
        if let Ok(status) = self.status.lock() {
            if let Some(status) = status.as_ref() {
                status.network_changed();
            }
        }
    }
    pub fn set_network_permission(&self, allowed: bool) {
        if !self.active() {
            return;
        }
        if let Ok(status) = self.status.lock() {
            if let Some(status) = status.as_ref() {
                if allowed {
                    status.enable();
                } else {
                    status.disable();
                }
            }
        }
    }
    pub fn observe_discovery_session(&self, session: u64, active: bool) {
        if self.active() {
            if let Ok(mut readiness) = self.discovery_readiness.lock() {
                readiness.observe_session(session, active);
            }
        }
    }
    pub fn observe_tcp_browse_started(&self, session: u64) {
        if self.active() {
            if let Ok(mut readiness) = self.discovery_readiness.lock() {
                readiness.observe_tcp_browse_started(session);
            }
        }
    }
    fn bonjour_ready(&self) -> bool {
        self.active()
            && self
                .discovery_readiness
                .lock()
                .is_ok_and(|readiness| readiness.ready())
    }
    pub async fn stop(&self) -> Result<(), String> {
        self.retiring.store(true, Ordering::Release);
        self.bridge.end_publication_session();
        self.bridge.wake_waiters();
        if let Ok(mut state) = owner().state.lock() {
            state.revision = state.revision.saturating_add(1);
            owner().changed.notify_all();
        }
        let joined = tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let changed = self.changed.notified();
                if self.stopped.load(Ordering::Acquire) {
                    return;
                }
                changed.await;
            }
        })
        .await;
        if joined.is_err() {
            return Err("Android LAN discovery shutdown has not been acknowledged".into());
        }
        let mut state = owner()
            .state
            .lock()
            .map_err(|_| "Android LAN owner is unavailable".to_string())?;
        if state
            .current
            .as_ref()
            .is_some_and(|current| current.generation == self.generation)
        {
            state.current = None;
            state.revision = state.revision.saturating_add(1);
            owner().changed.notify_all();
        }
        Ok(())
    }
}

pub fn session(generation: u64) -> Option<Arc<LanSession>> {
    owner()
        .state
        .lock()
        .ok()?
        .current
        .as_ref()
        .filter(|session| session.generation == generation)
        .cloned()
}
pub fn current_generation() -> u64 {
    owner()
        .state
        .lock()
        .ok()
        .and_then(|state| state.current.as_ref().map(|session| session.generation))
        .unwrap_or(0)
}
pub fn revision() -> u64 {
    owner().state.lock().map_or(0, |state| state.revision)
}
pub fn wait_for_change(observed: u64, timeout: Duration) -> u64 {
    let Ok(state) = owner().state.lock() else {
        return observed;
    };
    if state.revision != observed {
        return state.revision;
    }
    owner()
        .changed
        .wait_timeout_while(state, timeout, |state| state.revision == observed)
        .map_or(observed, |(state, _)| state.revision)
}
pub fn wake() {
    if let Ok(mut state) = owner().state.lock() {
        state.revision = state.revision.saturating_add(1);
        owner().changed.notify_all();
    }
}
pub fn acknowledge_stopped(generation: u64) {
    if let Some(session) = session(generation) {
        session.stopped.store(true, Ordering::Release);
        session.changed.notify_waiters();
    }
}
pub fn platform_detail() -> Option<String> {
    let current = owner().state.lock().ok()?.current.clone()?;
    let detail = current.detail.lock().ok()?.clone();
    detail.or_else(|| current.status.lock().ok()?.as_ref()?.activation_error())
}

pub fn platform_ready() -> Option<bool> {
    let current = owner().state.lock().ok()?.current.clone()?;
    let status = current.status.lock().ok()?.clone()?;
    let ready = status.discovery_ready()
        || (status.rendezvous_ready()
            && !current.inventory.snapshot().is_empty()
            && !status.local_addresses().is_empty()
            && current.bonjour_ready());
    Some(ready)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn bonjour_readiness_requires_a_current_actual_tcp_browse_callback() {
        let mut readiness = DiscoveryReadiness::default();
        readiness.observe_tcp_browse_started(1);
        assert!(!readiness.ready());
        readiness.observe_session(1, true);
        assert!(!readiness.ready());
        readiness.observe_tcp_browse_started(1);
        assert!(readiness.ready());
        readiness.observe_session(1, false);
        readiness.observe_tcp_browse_started(1);
        assert!(!readiness.ready());
        readiness.observe_session(2, true);
        readiness.observe_tcp_browse_started(1);
        assert!(!readiness.ready());
        readiness.observe_tcp_browse_started(2);
        assert!(readiness.ready());
        readiness.observe_session(1, false);
        assert!(readiness.ready());
        readiness.observe_session(1, true);
        assert!(readiness.ready());
        readiness.observe_session(2, false);
        assert!(!readiness.ready());
    }

    #[test]
    fn retirement_waits_for_its_exact_platform_generation_and_remains_retryable() {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        runtime.block_on(async {
            let (interface, session) = prepare().expect("first generation");
            let old = session.generation;
            let uncertain = tokio::time::timeout(Duration::from_millis(10), session.stop()).await;
            assert!(uncertain.is_err());
            assert!(session.is_retiring());
            assert!(prepare().is_err());
            acknowledge_stopped(old.saturating_add(1));
            assert!(!session.stopped.load(Ordering::Acquire));
            acknowledge_stopped(old);
            assert_eq!(session.stop().await, Ok(()));
            drop(interface);
            let (replacement, next) = prepare().expect("replacement after acknowledged shutdown");
            assert_ne!(old, next.generation);
            acknowledge_stopped(old);
            assert!(!next.stopped.load(Ordering::Acquire));
            acknowledge_stopped(next.generation);
            assert_eq!(next.stop().await, Ok(()));
            drop(replacement);
        });
    }
}
