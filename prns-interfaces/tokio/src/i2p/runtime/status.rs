use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use prns_core::interfaces::{
    ConnectionState, InterfaceId, InterfaceStatus, InterfaceVitals, TransferRates,
};
use prns_runtime::reactor::driver::TokioInterfaceStatus;

use super::super::sam::I2pBase32Address;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum I2pRuntimeIssue {
    None = 0,
    EntropyUnavailable = 1,
    DestinationStorage = 2,
    SamUnavailable = 3,
    PeerUnreachable = 4,
}

impl I2pRuntimeIssue {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::EntropyUnavailable,
            2 => Self::DestinationStorage,
            3 => Self::SamUnavailable,
            4 => Self::PeerUnreachable,
            _ => Self::None,
        }
    }

    fn description(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::EntropyUnavailable => Some("operating-system entropy unavailable"),
            Self::DestinationStorage => Some("persistent I2P destination unavailable"),
            Self::SamUnavailable => Some("I2P SAM bridge unavailable"),
            Self::PeerUnreachable => Some("I2P peer unreachable"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct I2pPeerStatus {
    wire: TokioInterfaceStatus,
    issue: Arc<AtomicU8>,
}

impl I2pPeerStatus {
    pub(crate) fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            wire: TokioInterfaceStatus::new(id, connection),
            issue: Arc::new(AtomicU8::new(I2pRuntimeIssue::None as u8)),
        }
    }

    pub(crate) fn wire(&self) -> &TokioInterfaceStatus {
        &self.wire
    }

    pub(crate) fn set_connection(&self, connection: ConnectionState) {
        self.wire.set_connection(connection);
    }

    pub(crate) fn set_issue(&self, issue: I2pRuntimeIssue) {
        self.issue.store(issue as u8, Ordering::Relaxed);
    }

    pub(crate) fn clear_issue(&self) {
        self.set_issue(I2pRuntimeIssue::None);
    }
}

impl InterfaceStatus for I2pPeerStatus {
    fn id(&self) -> InterfaceId {
        self.wire.id()
    }

    fn connection(&self) -> ConnectionState {
        self.wire.connection()
    }

    fn failure_reason(&self) -> Option<&'static str> {
        I2pRuntimeIssue::from_u8(self.issue.load(Ordering::Relaxed)).description()
    }

    fn rx_bytes(&self) -> u64 {
        self.wire.rx_bytes()
    }

    fn tx_bytes(&self) -> u64 {
        self.wire.tx_bytes()
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        self.wire.transfer_rates()
    }
}

#[derive(Clone)]
pub struct I2pInterfaceStatus {
    shared: Arc<I2pInterfaceStatusShared>,
}

struct I2pInterfaceStatusShared {
    id: InterfaceId,
    enabled: AtomicBool,
    enabled_changed: Notify,
    attempts_complete: AtomicBool,
    listener_online: AtomicBool,
    expects_activity: bool,
    issue: AtomicU8,
    published_destination: Mutex<Option<I2pBase32Address>>,
    members: Mutex<Vec<I2pPeerStatus>>,
}

impl I2pInterfaceStatus {
    pub(crate) fn new(id: InterfaceId, expects_activity: bool) -> Self {
        Self {
            shared: Arc::new(I2pInterfaceStatusShared {
                id,
                enabled: AtomicBool::new(true),
                enabled_changed: Notify::new(),
                attempts_complete: AtomicBool::new(false),
                listener_online: AtomicBool::new(false),
                expects_activity,
                issue: AtomicU8::new(I2pRuntimeIssue::None as u8),
                published_destination: Mutex::new(None),
                members: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
        self.shared.enabled_changed.notify_waiters();
    }

    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub fn published_destination(&self) -> Option<I2pBase32Address> {
        self.shared
            .published_destination
            .lock()
            .ok()
            .and_then(|destination| destination.clone())
    }

    pub fn member_vitals(&self) -> Vec<InterfaceVitals> {
        self.shared
            .members
            .lock()
            .map(|members| members.iter().map(InterfaceVitals::of).collect())
            .unwrap_or_default()
    }

    pub fn initial_attempts_complete(&self) -> bool {
        self.shared.attempts_complete.load(Ordering::Relaxed)
    }

    pub(crate) fn begin_cycle(&self) {
        self.shared
            .attempts_complete
            .store(false, Ordering::Relaxed);
        self.shared.listener_online.store(false, Ordering::Relaxed);
        self.set_issue(I2pRuntimeIssue::None);
        self.set_members(Vec::new());
    }

    pub(crate) fn set_attempts_complete(&self, complete: bool) {
        self.shared
            .attempts_complete
            .store(complete, Ordering::Relaxed);
    }

    pub(crate) fn set_listener_online(&self, online: bool) {
        self.shared.listener_online.store(online, Ordering::Relaxed);
    }

    pub(crate) fn set_issue(&self, issue: I2pRuntimeIssue) {
        self.shared.issue.store(issue as u8, Ordering::Relaxed);
    }

    pub(crate) fn set_published_destination(&self, destination: I2pBase32Address) {
        if let Ok(mut slot) = self.shared.published_destination.lock() {
            *slot = Some(destination);
        }
    }

    pub(crate) fn set_members(&self, members: Vec<I2pPeerStatus>) {
        if let Ok(mut slot) = self.shared.members.lock() {
            *slot = members;
        }
    }

    pub(crate) async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    pub(crate) async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        loop {
            if self.is_enabled() == enabled {
                return;
            }
            let changed = self.shared.enabled_changed.notified();
            if self.is_enabled() == enabled {
                return;
            }
            changed.await;
        }
    }

    fn members(&self) -> Vec<I2pPeerStatus> {
        self.shared
            .members
            .lock()
            .map(|members| members.clone())
            .unwrap_or_default()
    }
}

impl InterfaceStatus for I2pInterfaceStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        if !self.initial_attempts_complete() {
            return ConnectionState::Initializing;
        }
        if self.shared.listener_online.load(Ordering::Relaxed) {
            return ConnectionState::Connected;
        }
        let members = self.members();
        if members
            .iter()
            .any(|member| member.connection() == ConnectionState::Connected)
        {
            return ConnectionState::Connected;
        }
        if members
            .iter()
            .any(|member| member.connection() == ConnectionState::Degraded)
        {
            return ConnectionState::Degraded;
        }
        if self.shared.expects_activity {
            return ConnectionState::Reconnecting;
        }
        ConnectionState::Disconnected
    }

    fn failure_reason(&self) -> Option<&'static str> {
        I2pRuntimeIssue::from_u8(self.shared.issue.load(Ordering::Relaxed)).description()
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
