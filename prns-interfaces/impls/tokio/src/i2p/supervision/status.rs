use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use prns_core::interfaces::{
    ConnectionState, FrameAccounting, FrameAccountingEvent, InterfaceId, InterfaceStatus,
    InterfaceVitals, RecordsFrameAccounting, TransferRates,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;

use super::super::sam::I2pBase32Address;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum I2pInterfaceIssue {
    None = 0,
    EntropyUnavailable = 1,
    DestinationStorage = 2,
    SamUnavailable = 3,
    PeerUnreachable = 4,
}

impl I2pInterfaceIssue {
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
            wire: TokioInterfaceStatus::new_accounted(id, connection),
            issue: Arc::new(AtomicU8::new(I2pInterfaceIssue::None as u8)),
        }
    }

    pub(crate) fn wire(&self) -> &TokioInterfaceStatus {
        &self.wire
    }

    pub(crate) fn set_connection(&self, connection: ConnectionState) {
        self.wire.set_connection(connection);
    }

    pub(crate) fn set_issue(&self, issue: I2pInterfaceIssue) {
        self.issue.store(issue as u8, Ordering::Relaxed);
    }

    pub(crate) fn clear_issue(&self) {
        self.set_issue(I2pInterfaceIssue::None);
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
        I2pInterfaceIssue::from_u8(self.issue.load(Ordering::Relaxed)).description()
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

    fn frame_accounting(&self) -> Option<FrameAccounting> {
        self.wire.frame_accounting()
    }
}

impl RecordsFrameAccounting for I2pPeerStatus {
    fn record_frame_event(&self, event: FrameAccountingEvent) {
        self.wire.record_frame_event(event);
    }
}

#[derive(Clone)]
pub struct I2pInterfaceStatus {
    shared: Arc<I2pInterfaceStatusShared>,
}

struct I2pInterfaceStatusShared {
    id: InterfaceId,
    enabled: watch::Sender<bool>,
    attempts_complete: AtomicBool,
    listener_online: AtomicBool,
    expects_activity: bool,
    issue: AtomicU8,
    published_destination: Mutex<Option<I2pBase32Address>>,
    members: Mutex<Vec<I2pPeerStatus>>,
}

impl I2pInterfaceStatus {
    pub(crate) fn new(id: InterfaceId, expects_activity: bool) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            shared: Arc::new(I2pInterfaceStatusShared {
                id,
                enabled,
                attempts_complete: AtomicBool::new(false),
                listener_online: AtomicBool::new(false),
                expects_activity,
                issue: AtomicU8::new(I2pInterfaceIssue::None as u8),
                published_destination: Mutex::new(None),
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
        self.set_issue(I2pInterfaceIssue::None);
        self.set_members(Vec::new());
    }

    pub(crate) fn complete_initial_attempts(&self) {
        self.shared.attempts_complete.store(true, Ordering::Relaxed);
    }

    pub(crate) fn mark_listener_online(&self) {
        self.shared.listener_online.store(true, Ordering::Relaxed);
    }

    pub(crate) fn mark_listener_offline(&self) {
        self.shared.listener_online.store(false, Ordering::Relaxed);
    }

    pub(crate) fn set_issue(&self, issue: I2pInterfaceIssue) {
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
        let mut changed = self.shared.enabled.subscribe();
        let _ = changed.wait_for(|current| *current == enabled).await;
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
        I2pInterfaceIssue::from_u8(self.shared.issue.load(Ordering::Relaxed)).description()
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
