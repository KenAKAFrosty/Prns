use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::engine::ProtocolViolationKind;
use crate::interfaces::{
    AirtimeUtilization, ConnectionState, FrameAccounting, InterfaceId, InterfaceStatus,
    TransferRates,
};

pub(super) fn account_protocol_violation(
    statuses: &[&EmbassyInterfaceStatus],
    source: InterfaceId,
    violation: Option<ProtocolViolationKind>,
) {
    let Some(violation) = violation else {
        return;
    };
    if let Some(status) = statuses.iter().find(|status| status.id() == source) {
        if violation.is_malformed() {
            status.count_frame_malformed();
        } else {
            status.count_protocol_violation();
        }
    }
}

pub struct EmbassyInterfaceStatus {
    id: AtomicU64,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: AtomicBool,
    enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    publishes_frame_accounting: bool,
    frames_in: AtomicU64,
    frames_malformed: AtomicU64,
    protocol_violations: AtomicU64,
    frames_undecodable: AtomicU64,
    frames_delivered: AtomicU64,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

impl EmbassyInterfaceStatus {
    #[must_use]
    pub const fn new_accounted(id: InterfaceId, connection: ConnectionState) -> Self {
        Self::new(id, connection, true)
    }

    #[must_use]
    pub const fn new_unaccounted(id: InterfaceId, connection: ConnectionState) -> Self {
        Self::new(id, connection, false)
    }

    const fn new(
        id: InterfaceId,
        connection: ConnectionState,
        publishes_frame_accounting: bool,
    ) -> Self {
        Self {
            id: AtomicU64::new(u64::from_be_bytes(*id.as_bytes())),
            connection: AtomicU8::new(connection.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
            transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
            enabled: AtomicBool::new(true),
            enabled_changed: Signal::new(),
            publishes_frame_accounting,
            frames_in: AtomicU64::new(0),
            frames_malformed: AtomicU64::new(0),
            protocol_violations: AtomicU64::new(0),
            frames_undecodable: AtomicU64::new(0),
            frames_delivered: AtomicU64::new(0),
        }
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.connection.store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn set_id(&self, id: InterfaceId) {
        self.id
            .store(u64::from_be_bytes(*id.as_bytes()), Ordering::Relaxed);
    }

    pub fn enable(&self) {
        self.update_enabled(true);
    }

    pub fn disable(&self) {
        self.update_enabled(false);
    }

    pub fn toggle_enabled(&self) {
        let enabled = !self.enabled.fetch_xor(true, Ordering::Relaxed);
        self.enabled_changed.signal(enabled);
    }

    fn update_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            self.enabled_changed.signal(enabled);
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    pub async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        loop {
            if self.is_enabled() == enabled {
                return;
            }
            if self.enabled_changed.wait().await == enabled {
                return;
            }
        }
    }

    pub fn add_rx(&self, bytes: u64) {
        self.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_tx(&self, bytes: u64) {
        self.tx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_airtime(&self, utilization: AirtimeUtilization) {
        let packed =
            (u32::from(utilization.short_per_mille) << 16) | u32::from(utilization.long_per_mille);
        self.airtime.store(packed, Ordering::Relaxed);
    }

    pub fn set_transfer_rates(&self, rates: TransferRates) {
        let packed = (u64::from(rates.rx_bps) << 32) | u64::from(rates.tx_bps);
        self.transfer_rates.store(packed, Ordering::Relaxed);
    }

    pub fn count_frame_in(&self) {
        self.frames_in.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_malformed(&self) {
        self.frames_malformed.fetch_add(1, Ordering::Relaxed);
        self.count_protocol_violation();
    }

    pub fn count_protocol_violation(&self) {
        self.protocol_violations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_undecodable(&self) {
        self.frames_undecodable.fetch_add(1, Ordering::Relaxed);
        self.count_protocol_violation();
    }

    pub fn count_frame_delivered(&self) {
        self.frames_delivered.fetch_add(1, Ordering::Relaxed);
    }
}

impl InterfaceStatus for EmbassyInterfaceStatus {
    fn id(&self) -> InterfaceId {
        InterfaceId::new(self.id.load(Ordering::Relaxed).to_be_bytes())
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        ConnectionState::from_u8(self.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.tx.load(Ordering::Relaxed)
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        let packed = self.airtime.load(Ordering::Relaxed);
        if packed == AIRTIME_UNPUBLISHED {
            return None;
        }
        Some(AirtimeUtilization {
            short_per_mille: (packed >> 16) as u16,
            long_per_mille: packed as u16,
        })
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let packed = self.transfer_rates.load(Ordering::Relaxed);
        if packed == RATES_UNPUBLISHED {
            return None;
        }
        Some(TransferRates {
            rx_bps: (packed >> 32) as u32,
            tx_bps: packed as u32,
        })
    }

    fn frame_accounting(&self) -> Option<FrameAccounting> {
        if !self.publishes_frame_accounting {
            return None;
        }
        Some(FrameAccounting {
            frames_in: self.frames_in.load(Ordering::Relaxed),
            malformed: self.frames_malformed.load(Ordering::Relaxed),
            protocol_violations: self.protocol_violations.load(Ordering::Relaxed),
            undecodable: self.frames_undecodable.load(Ordering::Relaxed),
            delivered: self.frames_delivered.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::{block_on, join::join};

    #[test]
    fn enabled_state_changes_wake_waiters() {
        let status = EmbassyInterfaceStatus::new_unaccounted(
            InterfaceId::new([0x5A; 8]),
            ConnectionState::Initializing,
        );

        block_on(async {
            join(status.wait_until_disabled(), async {
                status.disable();
            })
            .await;
            join(status.wait_until_enabled(), async {
                status.toggle_enabled();
            })
            .await;
        });
        assert!(status.is_enabled());
        status.toggle_enabled();
        assert!(!status.is_enabled());
        status.enable();
        assert!(status.is_enabled());
    }

    #[test]
    fn construction_decides_whether_frame_accounting_is_published() {
        let status = EmbassyInterfaceStatus::new_unaccounted(
            InterfaceId::new([0x5A; 8]),
            ConnectionState::Connected,
        );

        status.count_frame_in();
        assert_eq!(status.frame_accounting(), None);

        let status = EmbassyInterfaceStatus::new_accounted(
            InterfaceId::new([0x5A; 8]),
            ConnectionState::Connected,
        );
        assert_eq!(status.frame_accounting(), Some(FrameAccounting::default()));

        status.count_frame_in();
        status.count_frame_malformed();
        status.count_frame_undecodable();
        status.count_frame_delivered();
        let counts = status.frame_accounting().unwrap();
        assert_eq!(
            (
                counts.frames_in,
                counts.malformed,
                counts.protocol_violations,
                counts.undecodable,
                counts.delivered
            ),
            (1, 1, 2, 1, 1)
        );
    }

    #[test]
    fn protocol_violation_is_charged_to_its_source_interface() {
        let source = InterfaceId::new([0x5A; 8]);
        let other = InterfaceId::new([0x6B; 8]);
        let source_status =
            EmbassyInterfaceStatus::new_accounted(source, ConnectionState::Connected);
        let other_status = EmbassyInterfaceStatus::new_accounted(other, ConnectionState::Connected);
        account_protocol_violation(
            &[&other_status, &source_status],
            source,
            Some(ProtocolViolationKind::Malformed),
        );

        assert_eq!(
            source_status.frame_accounting(),
            Some(FrameAccounting {
                malformed: 1,
                protocol_violations: 1,
                ..FrameAccounting::default()
            })
        );
        assert_eq!(
            other_status.frame_accounting(),
            Some(FrameAccounting::default())
        );
    }
}
