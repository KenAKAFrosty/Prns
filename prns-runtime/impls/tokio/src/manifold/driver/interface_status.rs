use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

use crate::interfaces::{
    AirtimeUtilization, ConnectionState, FrameAccounting, FrameAccountingEvent, InterfaceId,
    InterfaceStatus, RecordsFrameAccounting, TransferRates,
};

#[derive(Clone)]
pub struct TokioInterfaceStatus {
    inner: Arc<StatusCell>,
}

struct StatusCell {
    id: InterfaceId,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: watch::Sender<bool>,
    publishes_frame_accounting: bool,
    frames_in: AtomicU64,
    frames_malformed: AtomicU64,
    protocol_violations: AtomicU64,
    frames_undecodable: AtomicU64,
    frames_delivered: AtomicU64,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

fn pack_airtime(utilization: AirtimeUtilization) -> u32 {
    (u32::from(utilization.short_per_mille) << 16) | u32::from(utilization.long_per_mille)
}

fn unpack_airtime(packed: u32) -> Option<AirtimeUtilization> {
    if packed == AIRTIME_UNPUBLISHED {
        return None;
    }
    Some(AirtimeUtilization {
        short_per_mille: (packed >> 16) as u16,
        long_per_mille: packed as u16,
    })
}

impl TokioInterfaceStatus {
    #[must_use]
    pub fn new_accounted(id: InterfaceId, connection: ConnectionState) -> Self {
        Self::new(id, connection, true)
    }

    #[must_use]
    pub fn new_unaccounted(id: InterfaceId, connection: ConnectionState) -> Self {
        Self::new(id, connection, false)
    }

    fn new(id: InterfaceId, connection: ConnectionState, publishes_frame_accounting: bool) -> Self {
        let (enabled, _) = watch::channel(true);
        Self {
            inner: Arc::new(StatusCell {
                id,
                connection: AtomicU8::new(connection.as_u8()),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
                transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
                enabled,
                publishes_frame_accounting,
                frames_in: AtomicU64::new(0),
                frames_malformed: AtomicU64::new(0),
                protocol_violations: AtomicU64::new(0),
                frames_undecodable: AtomicU64::new(0),
                frames_delivered: AtomicU64::new(0),
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
        self.inner.enabled.send_if_modified(|current| {
            *current = !*current;
            true
        });
    }

    fn update_enabled(&self, enabled: bool) {
        self.inner.enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        *self.inner.enabled.borrow()
    }

    pub async fn wait_until_enabled(&self) {
        self.wait_for_enabled_state(true).await;
    }

    pub async fn wait_until_disabled(&self) {
        self.wait_for_enabled_state(false).await;
    }

    async fn wait_for_enabled_state(&self, enabled: bool) {
        let mut changed = self.inner.enabled.subscribe();
        let _ = changed.wait_for(|current| *current == enabled).await;
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        let previous = self
            .inner
            .connection
            .swap(connection.as_u8(), Ordering::Relaxed);
        #[cfg(feature = "tracing")]
        if previous != connection.as_u8() {
            tracing::info!(
                target: "prns.interface",
                event = "interface_connection_changed",
                interface_id = ?self.inner.id.as_bytes(),
                interface_kind = ?self.inner.id.kind(),
                previous = ?ConnectionState::from_u8(previous),
                connection = ?connection,
            );
        }
        #[cfg(not(feature = "tracing"))]
        let _ = previous;
    }

    pub fn add_rx(&self, bytes: u64) {
        self.inner.rx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_tx(&self, bytes: u64) {
        self.inner.tx.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_airtime(&self, utilization: AirtimeUtilization) {
        self.inner
            .airtime
            .store(pack_airtime(utilization), Ordering::Relaxed);
    }

    pub fn set_transfer_rates(&self, rates: TransferRates) {
        let packed = (u64::from(rates.rx_bps) << 32) | u64::from(rates.tx_bps);
        self.inner.transfer_rates.store(packed, Ordering::Relaxed);
    }

    pub fn count_frame_in(&self) {
        self.inner.frames_in.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_malformed(&self) {
        self.inner.frames_malformed.fetch_add(1, Ordering::Relaxed);
        self.count_protocol_violation();
    }

    pub fn count_protocol_violation(&self) {
        self.inner
            .protocol_violations
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_undecodable(&self) {
        self.inner
            .frames_undecodable
            .fetch_add(1, Ordering::Relaxed);
        self.count_protocol_violation();
    }

    pub fn count_frame_delivered(&self) {
        self.inner.frames_delivered.fetch_add(1, Ordering::Relaxed);
    }
}

impl InterfaceStatus for TokioInterfaceStatus {
    fn id(&self) -> InterfaceId {
        self.inner.id
    }

    fn connection(&self) -> ConnectionState {
        if !self.is_enabled() {
            return ConnectionState::Disabled;
        }
        ConnectionState::from_u8(self.inner.connection.load(Ordering::Relaxed))
    }

    fn rx_bytes(&self) -> u64 {
        self.inner.rx.load(Ordering::Relaxed)
    }

    fn tx_bytes(&self) -> u64 {
        self.inner.tx.load(Ordering::Relaxed)
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        unpack_airtime(self.inner.airtime.load(Ordering::Relaxed))
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let packed = self.inner.transfer_rates.load(Ordering::Relaxed);
        if packed == RATES_UNPUBLISHED {
            return None;
        }
        Some(TransferRates {
            rx_bps: (packed >> 32) as u32,
            tx_bps: packed as u32,
        })
    }

    fn frame_accounting(&self) -> Option<FrameAccounting> {
        if !self.inner.publishes_frame_accounting {
            return None;
        }
        Some(FrameAccounting {
            frames_in: self.inner.frames_in.load(Ordering::Relaxed),
            malformed: self.inner.frames_malformed.load(Ordering::Relaxed),
            protocol_violations: self.inner.protocol_violations.load(Ordering::Relaxed),
            undecodable: self.inner.frames_undecodable.load(Ordering::Relaxed),
            delivered: self.inner.frames_delivered.load(Ordering::Relaxed),
        })
    }
}

impl RecordsFrameAccounting for TokioInterfaceStatus {
    fn record_frame_event(&self, event: FrameAccountingEvent) {
        match event {
            FrameAccountingEvent::Received => self.count_frame_in(),
            FrameAccountingEvent::Malformed => self.count_frame_malformed(),
            FrameAccountingEvent::ProtocolViolation => self.count_protocol_violation(),
            FrameAccountingEvent::Undecodable => self.count_frame_undecodable(),
            FrameAccountingEvent::Delivered => self.count_frame_delivered(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airtime_reads_none_until_published_then_round_trips() {
        let status = TokioInterfaceStatus::new_unaccounted(
            InterfaceId::new([0x5A; 8]),
            ConnectionState::Initializing,
        );
        assert_eq!(status.airtime(), None);

        status.set_airtime(AirtimeUtilization {
            short_per_mille: 137,
            long_per_mille: 4,
        });
        assert_eq!(
            status.airtime(),
            Some(AirtimeUtilization {
                short_per_mille: 137,
                long_per_mille: 4,
            }),
        );
    }

    #[tokio::test]
    async fn enabled_state_changes_wake_waiters() {
        let status = TokioInterfaceStatus::new_unaccounted(
            InterfaceId::new([0x5A; 8]),
            ConnectionState::Initializing,
        );

        tokio::join!(
            status.wait_until_disabled(),
            status.wait_until_disabled(),
            async { status.disable() },
        );
        tokio::join!(
            status.wait_until_enabled(),
            status.wait_until_enabled(),
            async { status.toggle_enabled() },
        );
        assert!(status.is_enabled());
        status.toggle_enabled();
        assert!(!status.is_enabled());
        status.enable();
        assert!(status.is_enabled());
    }

    #[test]
    fn construction_decides_whether_frame_accounting_is_published() {
        let id = InterfaceId::new([0x5A; 8]);
        let unaccounted = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
        unaccounted.count_frame_in();
        assert_eq!(unaccounted.frame_accounting(), None);
        assert!(crate::interfaces::FrameAccountingRecorder::of(unaccounted).is_none());

        let accounted = TokioInterfaceStatus::new_accounted(id, ConnectionState::Connected);
        let recorder = crate::interfaces::FrameAccountingRecorder::of(accounted.clone())
            .expect("an accounted status exposes a recorder");
        for event in [
            FrameAccountingEvent::Received,
            FrameAccountingEvent::Malformed,
            FrameAccountingEvent::ProtocolViolation,
            FrameAccountingEvent::Undecodable,
            FrameAccountingEvent::Delivered,
        ] {
            recorder.record(event);
        }
        assert_eq!(
            accounted.frame_accounting(),
            Some(FrameAccounting {
                frames_in: 1,
                malformed: 1,
                protocol_violations: 3,
                undecodable: 1,
                delivered: 1,
            })
        );
    }
}
