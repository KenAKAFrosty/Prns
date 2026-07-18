use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InterfaceId, InterfaceStatus, TransferRates,
};

/// A cheap-clone handle to one interface's live state: the interface holds a clone and writes it as the wire moves (connection on connect/disconnect, bytes as they cross); the app holds a clone and reads it lock-free via [`InterfaceStatus`] on its own render cadence.
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
    enabled: AtomicBool,
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
    pub fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            inner: Arc::new(StatusCell {
                id,
                connection: AtomicU8::new(connection.as_u8()),
                rx: AtomicU64::new(0),
                tx: AtomicU64::new(0),
                airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
                transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
                enabled: AtomicBool::new(true),
            }),
        }
    }

    /// Turn this interface off or back on from the application. The driver reads [`is_enabled`](Self::is_enabled) and tears its wires down — releasing any resource it holds, e.g. an open serial port — while off, standing them back up on resume.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the interface is enabled (the default). The driver polls this to leave or re-enter its dormant state.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.inner
            .connection
            .store(connection.as_u8(), Ordering::Relaxed);
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
}
