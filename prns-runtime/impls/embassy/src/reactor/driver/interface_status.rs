use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::interfaces::{
    AirtimeUtilization, ConnectionState, InterfaceId, InterfaceStatus, TransferRates,
};

/// Lock-free live interface state shared by reference between the wire and display tasks. `portable-atomic` supplies the `u64` counters on 32-bit targets.
pub struct EmbassyInterfaceStatus {
    id: AtomicU64,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: AtomicBool,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;

impl EmbassyInterfaceStatus {
    #[must_use]
    pub const fn new(id: InterfaceId, connection: ConnectionState) -> Self {
        Self {
            id: AtomicU64::new(u64::from_be_bytes(*id.as_bytes())),
            connection: AtomicU8::new(connection.as_u8()),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            airtime: AtomicU32::new(AIRTIME_UNPUBLISHED),
            transfer_rates: AtomicU64::new(RATES_UNPUBLISHED),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn set_connection(&self, connection: ConnectionState) {
        self.connection.store(connection.as_u8(), Ordering::Relaxed);
    }

    pub fn set_id(&self, id: InterfaceId) {
        self.id
            .store(u64::from_be_bytes(*id.as_bytes()), Ordering::Relaxed);
    }

    /// Turn this interface off or back on from the application. The driver reads [`is_enabled`](Self::is_enabled) and goes dormant (wire closed, nothing ingested, egress drained and discarded) while off, holding its slot and routes for an instant resume.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether the interface is enabled (the default). The driver polls this to leave or re-enter its dormant state.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
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
}
