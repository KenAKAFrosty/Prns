use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::interfaces::{
    AirtimeUtilization, ConnectionState, FrameAccounting, InterfaceId, InterfaceStatus,
    TransferRates,
};

pub struct EmbassyInterfaceStatus {
    id: AtomicU64,
    connection: AtomicU8,
    rx: AtomicU64,
    tx: AtomicU64,
    airtime: AtomicU32,
    transfer_rates: AtomicU64,
    enabled: AtomicBool,
    enabled_changed: Signal<CriticalSectionRawMutex, bool>,
    accounts_frames: AtomicBool,
    last_frame_in_at: AtomicU64,
    frames_in: AtomicU64,
    frames_out: AtomicU64,
    frames_malformed: AtomicU64,
    frames_undecodable: AtomicU64,
    frames_delivered: AtomicU64,
}

const AIRTIME_UNPUBLISHED: u32 = u32::MAX;
const RATES_UNPUBLISHED: u64 = u64::MAX;
const NO_FRAME_YET: u64 = u64::MAX;

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
            enabled_changed: Signal::new(),
            accounts_frames: AtomicBool::new(false),
            last_frame_in_at: AtomicU64::new(NO_FRAME_YET),
            frames_in: AtomicU64::new(0),
            frames_out: AtomicU64::new(0),
            frames_malformed: AtomicU64::new(0),
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

    /// Declare that this interface counts frames, so a reader can tell an idle accounted link
    /// (all-zero) from a family that never counted (`None`). A driver calls this once at bring-up,
    /// before the first frame, and the counters publish from then on.
    pub fn account_frames(&self) {
        self.accounts_frames.store(true, Ordering::Relaxed);
    }

    /// Count an accepted inbound frame and stamp when, on the driver's own monotonic clock.
    /// One call does both so a counted frame can never be an unstamped one — a counter that
    /// moves while the stamp stands still would recreate the ambiguity the stamp exists to end.
    pub fn count_frame_in(&self, at_ms: u64) {
        self.last_frame_in_at.store(at_ms, Ordering::Relaxed);
        self.frames_in.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_out(&self) {
        self.frames_out.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_malformed(&self) {
        self.frames_malformed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_frame_undecodable(&self) {
        self.frames_undecodable.fetch_add(1, Ordering::Relaxed);
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
        if !self.accounts_frames.load(Ordering::Relaxed) {
            return None;
        }
        Some(FrameAccounting {
            frames_in: self.frames_in.load(Ordering::Relaxed),
            frames_out: self.frames_out.load(Ordering::Relaxed),
            malformed: self.frames_malformed.load(Ordering::Relaxed),
            undecodable: self.frames_undecodable.load(Ordering::Relaxed),
            delivered: self.frames_delivered.load(Ordering::Relaxed),
        })
    }

    fn last_frame_in_at_ms(&self) -> Option<u64> {
        match self.last_frame_in_at.load(Ordering::Relaxed) {
            NO_FRAME_YET => None,
            at_ms => Some(at_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::{block_on, join::join};

    #[test]
    fn enabled_state_changes_wake_waiters() {
        let status =
            EmbassyInterfaceStatus::new(InterfaceId::new([0x5A; 8]), ConnectionState::Initializing);

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
    fn frame_accounting_stays_unpublished_until_declared() {
        let status =
            EmbassyInterfaceStatus::new(InterfaceId::new([0x5A; 8]), ConnectionState::Connected);

        // Counting before declaring must not publish: a family that never declares stays None,
        // and None must remain distinguishable from an idle link's all-zero.
        status.count_frame_in(7);
        assert_eq!(status.frame_accounting(), None);

        status.account_frames();
        let counts = status.frame_accounting().unwrap();
        assert_eq!(counts.frames_in, 1);

        status.count_frame_in(19);
        status.count_frame_out();
        status.count_frame_malformed();
        status.count_frame_undecodable();
        status.count_frame_delivered();
        let counts = status.frame_accounting().unwrap();
        assert_eq!(
            (
                counts.frames_in,
                counts.frames_out,
                counts.malformed,
                counts.undecodable,
                counts.delivered
            ),
            (2, 1, 1, 1, 1)
        );
    }

    #[test]
    fn the_inbound_stamp_tracks_the_latest_accepted_frame() {
        let status =
            EmbassyInterfaceStatus::new(InterfaceId::new([0x5A; 8]), ConnectionState::Connected);

        // No frame yet reads None, not zero: "never" and "at boot" are different answers.
        assert_eq!(status.last_frame_in_at_ms(), None);

        status.count_frame_in(1_000);
        assert_eq!(status.last_frame_in_at_ms(), Some(1_000));

        // Only accepted inbound frames move the stamp; discards and TX must not refresh it,
        // or a link delivering nothing but garbage would keep reading fresh.
        status.count_frame_out();
        status.count_frame_malformed();
        status.count_frame_undecodable();
        assert_eq!(status.last_frame_in_at_ms(), Some(1_000));

        status.count_frame_in(2_500);
        assert_eq!(status.last_frame_in_at_ms(), Some(2_500));
    }
}
