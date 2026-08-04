use cfg_if::cfg_if;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiscoveryWindow {
    Foreground,
    #[cfg(not(target_arch = "riscv32"))]
    Background,
}

impl DiscoveryWindow {
    pub(super) fn advertising_duration(self) -> Duration {
        match self {
            Self::Foreground => ADV_WINDOW,
            #[cfg(not(target_arch = "riscv32"))]
            Self::Background => CONNECTED_DISCOVERY_WINDOW,
        }
    }

    pub(super) fn scanning_duration(self) -> Duration {
        match self {
            Self::Foreground => SCAN_WINDOW,
            #[cfg(not(target_arch = "riscv32"))]
            Self::Background => CONNECTED_DISCOVERY_WINDOW,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DiscoveryRole {
    Advertise,
    Scan,
}

cfg_if! {
    if #[cfg(not(target_arch = "riscv32"))] {
        use portable_atomic::{AtomicU64, AtomicU8};

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        enum DiscoveryDecision {
            Foreground,
            Suspended,
            Wait(u64),
            Background,
        }

        fn discovery_decision(
            live_links: u8,
            busy_operations: u8,
            last_activity_ms: u64,
            last_turn_end_ms: u64,
            now_ms: u64,
        ) -> DiscoveryDecision {
            if live_links == 0 {
                return DiscoveryDecision::Foreground;
            }
            if usize::from(live_links) >= PEER_CAPACITY || busy_operations > 0 {
                return DiscoveryDecision::Suspended;
            }
            let ready_at = last_activity_ms
                .saturating_add(CONNECTED_DISCOVERY_QUIET_MS)
                .max(last_turn_end_ms.saturating_add(CONNECTED_DISCOVERY_REST_MS));
            if now_ms < ready_at {
                DiscoveryDecision::Wait(ready_at - now_ms)
            } else {
                DiscoveryDecision::Background
            }
        }
    }
}

cfg_if! {
    if #[cfg(target_arch = "riscv32")] {
        pub(super) fn advertisement_parameters(
            window: DiscoveryWindow,
        ) -> AdvertisementParameters {
            let _ = window;
            let mut params = AdvertisementParameters::default();
            params.interval_min = Duration::from_millis(240);
            params.interval_max = Duration::from_millis(320);
            params
        }

        pub(super) fn idle_scan_parameters(window: DiscoveryWindow) -> (Duration, Duration) {
            let _ = window;
            (IDLE_SCAN_INTERVAL, IDLE_SCAN_WINDOW)
        }

        pub(super) fn connect_scan_parameters(
            window: DiscoveryWindow,
        ) -> (Duration, Duration, Duration) {
            let _ = window;
            (CONNECT_TIMEOUT, CONNECT_SCAN_INTERVAL, CONNECT_SCAN_WINDOW)
        }
    } else {
        pub(super) fn advertisement_parameters(
            window: DiscoveryWindow,
        ) -> AdvertisementParameters {
            let mut params = AdvertisementParameters::default();
            if window == DiscoveryWindow::Background {
                params.interval_min = CONNECTED_ADV_INTERVAL_MIN;
                params.interval_max = CONNECTED_ADV_INTERVAL_MAX;
            }
            params
        }

        pub(super) fn idle_scan_parameters(window: DiscoveryWindow) -> (Duration, Duration) {
            match window {
                DiscoveryWindow::Foreground => (IDLE_SCAN_INTERVAL, IDLE_SCAN_WINDOW),
                DiscoveryWindow::Background => (CONNECTED_SCAN_INTERVAL, CONNECTED_SCAN_WINDOW),
            }
        }

        pub(super) fn connect_scan_parameters(
            window: DiscoveryWindow,
        ) -> (Duration, Duration, Duration) {
            match window {
                DiscoveryWindow::Foreground => {
                    (CONNECT_TIMEOUT, CONNECT_SCAN_INTERVAL, CONNECT_SCAN_WINDOW)
                }
                DiscoveryWindow::Background => (
                    CONNECTED_CONNECT_TIMEOUT,
                    CONNECTED_SCAN_INTERVAL,
                    CONNECTED_SCAN_WINDOW,
                ),
            }
        }
    }
}

pub(super) struct LiveLinkGuard<'a> {
    state: &'a DiscoveryState,
}

impl Drop for LiveLinkGuard<'_> {
    fn drop(&mut self) {
        self.state.finish_live_link();
    }
}

pub(super) struct BusyOperationGuard<'a> {
    state: &'a DiscoveryState,
}

impl Drop for BusyOperationGuard<'_> {
    fn drop(&mut self) {
        self.state.finish_busy_operation();
    }
}

cfg_if! {
    if #[cfg(not(target_arch = "riscv32"))] {
        pub(super) struct DiscoveryState {
            live_links: AtomicU8,
            busy_operations: AtomicU8,
            last_activity_ms: AtomicU64,
            last_discovery_end_ms: AtomicU64,
            advertise_activity: Signal<BridgeMutex, ()>,
            scan_activity: Signal<BridgeMutex, ()>,
        }

        impl DiscoveryState {
    pub(super) const fn new() -> Self {
        Self {
            live_links: AtomicU8::new(0),
            busy_operations: AtomicU8::new(0),
            last_activity_ms: AtomicU64::new(0),
            last_discovery_end_ms: AtomicU64::new(0),
            advertise_activity: Signal::new(),
            scan_activity: Signal::new(),
        }
    }

    pub(super) fn track_live_link(&self) -> LiveLinkGuard<'_> {
        let _ = self
            .live_links
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_add(1).min(PEER_CAPACITY as u8))
            });
        self.note_link_activity();
        LiveLinkGuard { state: self }
    }

    fn finish_live_link(&self) {
        let _ = self
            .live_links
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
        self.note_link_activity();
    }

    pub(super) fn begin_busy_operation(&self) -> BusyOperationGuard<'_> {
        let _ = self
            .busy_operations
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_add(1))
            });
        self.note_link_activity();
        BusyOperationGuard { state: self }
    }

    fn finish_busy_operation(&self) {
        let _ = self
            .busy_operations
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
        self.note_link_activity();
    }

    pub(super) fn note_link_activity(&self) {
        self.last_activity_ms
            .store(Instant::now().as_millis(), Ordering::Release);
        self.advertise_activity.signal(());
        self.scan_activity.signal(());
    }

    fn activity_signal(&self, role: DiscoveryRole) -> &Signal<BridgeMutex, ()> {
        match role {
            DiscoveryRole::Advertise => &self.advertise_activity,
            DiscoveryRole::Scan => &self.scan_activity,
        }
    }

    pub(super) async fn await_turn(
        &self,
        enabled: &Signal<BridgeMutex, bool>,
        role: DiscoveryRole,
    ) -> Result<DiscoveryWindow, bool> {
        let activity = self.activity_signal(role);
        loop {
            activity.reset();
            let now_ms = Instant::now().as_millis();
            let decision = discovery_decision(
                self.live_links.load(Ordering::Acquire),
                self.busy_operations.load(Ordering::Acquire),
                self.last_activity_ms.load(Ordering::Acquire),
                self.last_discovery_end_ms.load(Ordering::Acquire),
                now_ms,
            );
            match decision {
                DiscoveryDecision::Foreground => return Ok(DiscoveryWindow::Foreground),
                DiscoveryDecision::Background => return Ok(DiscoveryWindow::Background),
                DiscoveryDecision::Suspended => {
                    match select(enabled.wait(), activity.wait()).await {
                        Either::First(state) => return Err(state),
                        Either::Second(()) => {}
                    }
                }
                DiscoveryDecision::Wait(wait_ms) => {
                    match select3(
                        Timer::after(Duration::from_millis(wait_ms)),
                        enabled.wait(),
                        activity.wait(),
                    )
                    .await
                    {
                        Either3::First(()) | Either3::Third(()) => {}
                        Either3::Second(state) => return Err(state),
                    }
                }
            }
        }
    }

    pub(super) async fn wait_for_activity(&self, role: DiscoveryRole) {
        self.activity_signal(role).wait().await;
    }

    pub(super) fn finish_turn(&self, window: DiscoveryWindow) {
        if window == DiscoveryWindow::Background {
            self.last_discovery_end_ms
                .store(Instant::now().as_millis(), Ordering::Release);
        }
    }
        }
    } else {
        pub(super) struct DiscoveryState;

        impl DiscoveryState {
    pub(super) const fn new() -> Self {
        Self
    }

    pub(super) fn track_live_link(&self) -> LiveLinkGuard<'_> {
        LiveLinkGuard { state: self }
    }

    fn finish_live_link(&self) {}

    pub(super) fn begin_busy_operation(&self) -> BusyOperationGuard<'_> {
        BusyOperationGuard { state: self }
    }

    fn finish_busy_operation(&self) {}

    pub(super) fn note_link_activity(&self) {}

    pub(super) async fn await_turn(
        &self,
        enabled: &Signal<BridgeMutex, bool>,
        role: DiscoveryRole,
    ) -> Result<DiscoveryWindow, bool> {
        let _ = (enabled, role);
        Ok(DiscoveryWindow::Foreground)
    }

    pub(super) async fn wait_for_activity(&self, role: DiscoveryRole) {
        let _ = role;
        core::future::pending::<()>().await;
    }

    pub(super) fn finish_turn(&self, window: DiscoveryWindow) {
        let _ = window;
    }
        }
    }
}

#[cfg(all(test, not(target_arch = "riscv32")))]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_foreground_without_a_live_link() {
        assert_eq!(
            discovery_decision(0, 0, 9_000, 9_000, 9_001),
            DiscoveryDecision::Foreground
        );
    }

    #[test]
    fn live_link_activity_extends_the_quiet_deadline() {
        assert_eq!(
            discovery_decision(1, 0, 1_000, 0, 1_749),
            DiscoveryDecision::Wait(1)
        );
        assert_eq!(
            discovery_decision(1, 0, 1_000, 0, 1_750),
            DiscoveryDecision::Background
        );
        assert_eq!(
            discovery_decision(1, 0, 1_700, 0, 1_750),
            DiscoveryDecision::Wait(700)
        );
    }

    #[test]
    fn discovery_roles_share_one_background_rest_deadline() {
        assert_eq!(
            discovery_decision(1, 0, 0, 2_000, 2_499),
            DiscoveryDecision::Wait(1)
        );
        assert_eq!(
            discovery_decision(1, 0, 0, 2_000, 2_500),
            DiscoveryDecision::Background
        );
    }

    #[test]
    fn busy_operations_and_full_capacity_suspend_discovery() {
        assert_eq!(
            discovery_decision(1, 1, 0, 0, 10_000),
            DiscoveryDecision::Suspended
        );
        for links in 1..PEER_CAPACITY as u8 {
            assert_eq!(
                discovery_decision(links, 0, 0, 0, 10_000),
                DiscoveryDecision::Background
            );
        }
        assert_eq!(
            discovery_decision(PEER_CAPACITY as u8, 0, 0, 0, 10_000),
            DiscoveryDecision::Suspended
        );
    }

    #[test]
    fn connected_windows_use_the_bounded_radio_budget() {
        let params = advertisement_parameters(DiscoveryWindow::Background);
        assert_eq!(params.interval_min, CONNECTED_ADV_INTERVAL_MIN);
        assert_eq!(params.interval_max, CONNECTED_ADV_INTERVAL_MAX);
        assert_eq!(
            DiscoveryWindow::Background.advertising_duration(),
            CONNECTED_DISCOVERY_WINDOW
        );
        assert_eq!(
            idle_scan_parameters(DiscoveryWindow::Background),
            (CONNECTED_SCAN_INTERVAL, CONNECTED_SCAN_WINDOW)
        );
    }

    #[test]
    fn live_and_busy_guards_release_their_counts_after_errors() {
        let state = DiscoveryState::new();

        fn fail_while_live(state: &DiscoveryState) -> Result<(), ()> {
            let _live = state.track_live_link();
            assert_eq!(state.live_links.load(Ordering::Acquire), 1);
            Err(())
        }

        fn fail_while_busy(state: &DiscoveryState) -> Result<(), ()> {
            let _busy = state.begin_busy_operation();
            assert_eq!(state.busy_operations.load(Ordering::Acquire), 1);
            Err(())
        }

        assert_eq!(fail_while_live(&state), Err(()));
        assert_eq!(state.live_links.load(Ordering::Acquire), 0);
        assert_eq!(fail_while_busy(&state), Err(()));
        assert_eq!(state.busy_operations.load(Ordering::Acquire), 0);
    }
}
