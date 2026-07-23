use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use prns_core::interfaces::weave::{EndpointId, SwitchId};
use prns_core::interfaces::{
    ConnectionState, InterfaceId, InterfaceStatus, InterfaceVitals, TransferRates,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WeaveInterfaceIssue {
    None = 0,
    SerialUnavailable = 1,
    HandshakeTimedOut = 2,
    ConnectionLost = 3,
}

impl WeaveInterfaceIssue {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::SerialUnavailable,
            2 => Self::HandshakeTimedOut,
            3 => Self::ConnectionLost,
            _ => Self::None,
        }
    }

    fn description(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::SerialUnavailable => Some("Weave serial device unavailable"),
            Self::HandshakeTimedOut => Some("Weave device handshake timed out"),
            Self::ConnectionLost => Some("Weave device connection lost"),
        }
    }
}

#[derive(Clone)]
pub struct WeaveInterfaceStatus {
    shared: Arc<SharedStatus>,
}

struct SharedStatus {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    initial_attempt_complete: AtomicBool,
    connected: AtomicBool,
    issue: AtomicU8,
    remote_switch: Mutex<Option<SwitchId>>,
    host_endpoint: Mutex<Option<EndpointId>>,
    members: Mutex<Vec<TokioInterfaceStatus>>,
}

impl WeaveInterfaceStatus {
    pub(super) fn new(id: InterfaceId) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(SharedStatus {
                id,
                enabled,
                initial_attempt_complete: AtomicBool::new(false),
                connected: AtomicBool::new(false),
                issue: AtomicU8::new(WeaveInterfaceIssue::None as u8),
                remote_switch: Mutex::new(None),
                host_endpoint: Mutex::new(None),
                members: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        self.shared.enabled.send_if_modified(|current| {
            *current = !*current;
            true
        });
    }

    fn update_enabled(&self, enabled: bool) {
        self.shared.enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    pub fn is_enabled(&self) -> bool {
        *self.shared.enabled.borrow()
    }

    pub fn initial_attempt_complete(&self) -> bool {
        self.shared.initial_attempt_complete.load(Ordering::Relaxed)
    }

    pub fn remote_switch(&self) -> Option<SwitchId> {
        self.shared
            .remote_switch
            .lock()
            .ok()
            .and_then(|value| *value)
    }

    pub fn host_endpoint(&self) -> Option<EndpointId> {
        self.shared
            .host_endpoint
            .lock()
            .ok()
            .and_then(|value| *value)
    }

    pub fn member_vitals(&self) -> Vec<InterfaceVitals> {
        self.members().iter().map(InterfaceVitals::of).collect()
    }

    pub(super) fn begin_connection_attempt(&self) {
        self.shared.connected.store(false, Ordering::Relaxed);
        self.set_issue(WeaveInterfaceIssue::None);
        if let Ok(mut remote_switch) = self.shared.remote_switch.lock() {
            *remote_switch = None;
        }
        if let Ok(mut host_endpoint) = self.shared.host_endpoint.lock() {
            *host_endpoint = None;
        }
    }

    pub(super) fn complete_initial_attempt(&self) {
        self.shared
            .initial_attempt_complete
            .store(true, Ordering::Relaxed);
    }

    pub(super) fn mark_connected(&self) {
        self.shared.connected.store(true, Ordering::Relaxed);
    }

    pub(super) fn mark_disconnected(&self) {
        self.shared.connected.store(false, Ordering::Relaxed);
    }

    pub(super) fn set_issue(&self, issue: WeaveInterfaceIssue) {
        self.shared.issue.store(issue as u8, Ordering::Relaxed);
    }

    pub(super) fn set_remote_switch(&self, switch_id: SwitchId) {
        if let Ok(mut remote_switch) = self.shared.remote_switch.lock() {
            *remote_switch = Some(switch_id);
        }
    }

    pub(super) fn set_host_endpoint(&self, endpoint: EndpointId) {
        if let Ok(mut host_endpoint) = self.shared.host_endpoint.lock() {
            *host_endpoint = Some(endpoint);
        }
    }

    pub(super) fn set_members(&self, members: Vec<TokioInterfaceStatus>) {
        if let Ok(mut slot) = self.shared.members.lock() {
            *slot = members;
        }
    }

    pub(super) async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    pub(super) async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed.wait_for(|current| *current == enabled).await;
    }

    fn members(&self) -> Vec<TokioInterfaceStatus> {
        self.shared
            .members
            .lock()
            .map(|members| members.clone())
            .unwrap_or_default()
    }
}

impl InterfaceStatus for WeaveInterfaceStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        if !self.initial_attempt_complete() {
            return ConnectionState::Initializing;
        }
        if self.shared.connected.load(Ordering::Relaxed) {
            return ConnectionState::Connected;
        }
        ConnectionState::Reconnecting
    }

    fn failure_reason(&self) -> Option<&'static str> {
        WeaveInterfaceIssue::from_u8(self.shared.issue.load(Ordering::Relaxed)).description()
    }

    fn rx_bytes(&self) -> u64 {
        self.members().iter().map(InterfaceStatus::rx_bytes).sum()
    }

    fn tx_bytes(&self) -> u64 {
        self.members().iter().map(InterfaceStatus::tx_bytes).sum()
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        self.members()
            .iter()
            .filter_map(InterfaceStatus::transfer_rates)
            .reduce(|left, right| TransferRates {
                rx_bps: left.rx_bps.saturating_add(right.rx_bps),
                tx_bps: left.tx_bps.saturating_add(right.tx_bps),
            })
    }
}
