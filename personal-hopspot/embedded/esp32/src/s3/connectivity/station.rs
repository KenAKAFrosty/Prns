use super::super::captive_portal::{ap_config, station_wifi_mode};
use super::super::*;
use crate::wifi_data_path_recovery::{
    StationDataPathAction, StationDataPathRecovery, StationDataPathWindow,
};

#[embassy_executor::task(pool_size = 2)]
pub(in crate::s3) async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub(super) async fn network_ready_task(stack: Stack<'static>) -> ! {
    let mut previous_state = None;
    let mut previous_data_path = None;
    let mut station_data_path_recovery = StationDataPathRecovery::new();
    let mut samples_until_report = 0;
    let mut internal_free_low_water = usize::MAX;
    loop {
        let associated = WIFI_STATION_JOINED.load(Ordering::Relaxed);
        let link_up = stack.is_link_up();
        let ipv4 = stack.config_v4();
        let has_ipv4 = ipv4.is_some();
        let state = (associated, link_up, has_ipv4);
        let state_changed = previous_state != Some(state);
        let was_ready = previous_state
            .map(|(_, previous_link, previous_ipv4)| previous_link && previous_ipv4)
            .unwrap_or(false);
        let ready = link_up && has_ipv4;
        let internal_free = esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into());
        let external_free = esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::External.into());
        internal_free_low_water = internal_free_low_water.min(internal_free);

        if ready && !was_ready {
            boot_stage(BootPhase::NetworkReady);
        }
        // The radio data path is live as soon as association raises the link. Do not wait for
        // IPv4 configuration: RX can wedge while DHCP is still settling, and an already observed
        // LAN peer is sufficient evidence that inbound beacons should continue.
        let station_ready = associated && link_up;
        let should_report = state_changed || samples_until_report == 0;
        if should_report {
            let heap = esp_alloc::HEAP.stats();
            log::info!(
                "wifi-health: associated={} link_up={} ipv4={:?} internal_free={} internal_low={} external_free={} heap_free={} heap_used={} heap_high={}",
                associated,
                link_up,
                ipv4,
                internal_free,
                internal_free_low_water,
                external_free,
                heap.size.saturating_sub(heap.current_usage),
                heap.current_usage,
                heap.max_usage
            );
            samples_until_report = WIFI_HEALTH_SAMPLES_BETWEEN_REPORTS;
        } else {
            samples_until_report = samples_until_report.saturating_sub(1);
        }
        inspect_station_data_path(
            &mut previous_data_path,
            &mut station_data_path_recovery,
            station_ready,
            should_report,
        );
        previous_state = Some(state);
        Timer::after(WIFI_LINK_CHECK_INTERVAL).await;
    }
}

// Keep the comparatively large current diagnostics snapshot in this synchronous frame instead of
// the Embassy task future. Task futures live in scarce internal RAM and reduce the main boot stack;
// only the previous snapshot needs to persist across the two-second sampling interval.
fn inspect_station_data_path(
    previous: &mut Option<esp_radio::wifi::DataPathDiagnostics>,
    recovery: &mut StationDataPathRecovery,
    station_ready: bool,
    should_report: bool,
) {
    let current = esp_radio::wifi::data_path_diagnostics();
    if station_ready {
        let window = previous.as_ref().map(|earlier| {
            if current.transmit_submission_stalled_since(earlier) {
                StationDataPathWindow::TransmitSubmissionStalled
            } else if current.receive_delivery_blocked_by_transmit_capacity_since(earlier) {
                StationDataPathWindow::TransmitCapacityBlocked
            } else if current.station_receive_progressed_since(earlier) {
                StationDataPathWindow::ReceiveProgress
            } else if current.transmit_progressed_without_station_receive_since(earlier) {
                StationDataPathWindow::TransmitWithoutReceive
            } else {
                StationDataPathWindow::NoProgress
            }
        });
        if let Some(window) = window {
            if matches!(&window, StationDataPathWindow::ReceiveProgress) {
                WIFI_STATION_DATA_PATH_DEGRADED.store(false, Ordering::Release);
            }
            match recovery.observe(window) {
                StationDataPathAction::Continue => {}
                StationDataPathAction::RestartDriver { count, cause } => {
                    WIFI_STATION_DATA_PATH_DEGRADED.store(true, Ordering::Release);
                    WIFI_DRIVER_RESTART_REQUESTED.store(true, Ordering::Release);
                    log::warn!("wifi-radio-trace: {current:?}");
                    log::warn!(
                        "wifi-health: station data path stalled cause={cause:?}; requested driver restart count={count}"
                    );
                }
            }
        }
    } else {
        recovery.station_unavailable();
    }
    if should_report {
        log::info!("wifi-data: {current}");
        log::info!("wifi-rtos: {:?}", esp_rtos::radio_wait_queue_diagnostics());
    }
    *previous = if station_ready { Some(current) } else { None };
}

const WIFI_HEALTH_SAMPLES_BETWEEN_REPORTS: u8 = 4;
const WIFI_LINK_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const WIFI_INTER_CHANNEL_DELAY: Duration = Duration::from_millis(25);
const WIFI_CHANNEL_SCAN_TIMEOUT: Duration = Duration::from_millis(500);
const WIFI_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const WIFI_SCAN_MIN_DWELL: HalDuration = HalDuration::from_millis(5);
const WIFI_SCAN_MAX_DWELL: HalDuration = HalDuration::from_millis(20);
const DRIVER_STOP_RETRY_DELAY: Duration = Duration::from_millis(25);
const ESP_OK: i32 = 0;
const ESP_ERR_WIFI_NOT_INIT: i32 = 12_289;
const ESP_ERR_WIFI_NOT_STARTED: i32 = 12_290;

pub(super) struct StationCredentials {
    pub(super) ssid: String,
    pub(super) password: String,
}

extern "C" {
    fn esp_wifi_disconnect_internal() -> i32;
    fn esp_wifi_scan_stop() -> i32;
}

#[allow(clippy::undocumented_unsafe_blocks)]
async fn stop_station_connection() {
    let mut reported = None;
    loop {
        let result = unsafe { esp_wifi_disconnect_internal() };
        if matches!(
            result,
            ESP_OK | ESP_ERR_WIFI_NOT_INIT | ESP_ERR_WIFI_NOT_STARTED
        ) {
            return;
        }
        if reported != Some(result) {
            log::warn!("wifi: station stop pending code={result}");
            reported = Some(result);
        }
        Timer::after(DRIVER_STOP_RETRY_DELAY).await;
    }
}

#[allow(clippy::undocumented_unsafe_blocks)]
async fn stop_station_scan() {
    let mut reported = None;
    loop {
        let result = unsafe { esp_wifi_scan_stop() };
        if matches!(
            result,
            ESP_OK | ESP_ERR_WIFI_NOT_INIT | ESP_ERR_WIFI_NOT_STARTED
        ) {
            return;
        }
        if reported != Some(result) {
            log::warn!("wifi: scan stop pending code={result}");
            reported = Some(result);
        }
        Timer::after(DRIVER_STOP_RETRY_DELAY).await;
    }
}

#[embassy_executor::task]
pub(super) async fn wifi_connect_task(
    mut controller: WifiController<'static>,
    status: AutoWifiStatus<MEMBERS>,
    credentials: StationCredentials,
    ap_enabled: bool,
) -> ! {
    let base = StationConfig::default()
        .with_ssid(credentials.ssid.clone())
        .with_password(credentials.password.clone());
    let mut recovery = StationRecovery::new(DiscoveryScope::FullBand);
    let mut soft_ap_active = ap_enabled;

    loop {
        let mut resumed = false;
        while !status.is_station_uplink_enabled() {
            WIFI_STATION_DATA_PATH_DEGRADED.store(false, Ordering::Release);
            WIFI_DRIVER_RESTART_REQUESTED.store(false, Ordering::Release);
            if controller.is_connected() {
                let _ = controller.disconnect_async().await;
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            status.wait_until_station_uplink_enabled().await;
            resumed = true;
        }
        if resumed {
            recovery.resume_now();
        }
        if WIFI_DRIVER_RESTART_REQUESTED.swap(false, Ordering::AcqRel) {
            log::warn!("wifi: restarting driver after data-path recovery escalation");
            // Do not disconnect first. `disconnect_async()` enters the vendor's synchronous
            // `esp_wifi_disconnect_internal()` call before it can yield, so an async timeout
            // cannot protect us when the TX-completion path itself is wedged. `restart()` already
            // stops and deinitializes the driver after disabling RX admission and draining queued
            // buffers, and is the recovery boundary we actually need.
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            if let Err(error) = controller.restart() {
                WIFI_DRIVER_RESTART_REQUESTED.store(true, Ordering::Release);
                log::warn!("wifi: data-path recovery driver restart failed: {error:?}");
                Timer::after(DRIVER_STOP_RETRY_DELAY).await;
                continue;
            }
            recovery.resume_now();
            continue;
        }
        if controller.is_connected() {
            WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
            match select3(
                controller.wait_for_disconnect_async(),
                status.wait_until_station_uplink_disabled(),
                Timer::after(WIFI_LINK_CHECK_INTERVAL),
            )
            .await
            {
                Either3::First(Ok(disconnected)) => {
                    log::warn!(
                        "wifi: station disconnected ({:?}, rssi {})",
                        disconnected.reason,
                        disconnected.rssi
                    );
                }
                Either3::First(Err(error)) => {
                    log::warn!("wifi: disconnect monitor failed: {error:?}");
                }
                Either3::Second(()) => {
                    let _ = controller.disconnect_async().await;
                }
                Either3::Third(()) => continue,
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
            continue;
        }
        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
        // APSTA has one physical 2.4 GHz radio, but the station must discover its uplink before
        // that shared channel is known. Pinning discovery to the SoftAP's provisional boot channel
        // makes every configured LAN on another channel permanently invisible. Scan the full band;
        // once the station associates, the Espressif driver moves the SoftAP onto that channel.
        recovery.set_discovery_scope(DiscoveryScope::FullBand);
        let Some(attempt) = recovery.begin_attempt() else {
            Timer::after(DRIVER_STOP_RETRY_DELAY).await;
            continue;
        };
        match attempt {
            StationAttempt::Connect(attempt) => {
                let access_point = attempt.access_point();
                let access_point_channel = access_point.channel;
                let station = base
                    .clone()
                    .with_bssid(access_point.bssid)
                    .with_channel(access_point_channel);
                let configured = {
                    let mode = station_wifi_mode(station, ap_enabled, Some(access_point_channel));
                    match controller.set_config(&mode) {
                        Ok(()) => {
                            soft_ap_active = ap_enabled;
                            true
                        }
                        Err(error) => {
                            log::warn!("wifi: station configuration failed: {error:?}");
                            false
                        }
                    }
                };
                if !configured {
                    let next = recovery.finish_connection(
                        attempt,
                        ConnectionOutcome::Failed(ConnectionFailure::Driver),
                    );
                    apply_station_yield(next, &status).await;
                    continue;
                }
                if !status.is_station_uplink_enabled() {
                    let next = recovery.finish_connection(attempt, ConnectionOutcome::Cancelled);
                    recovery.resume_now();
                    apply_station_yield(next, &status).await;
                    continue;
                }
                boot_stage(BootPhase::WifiConnectionBegin);
                let started_at = embassy_time::Instant::now().as_millis();
                log::info!(
                    "wifi: station connection begin channel={}",
                    access_point.channel
                );
                let connected = embassy_futures::select::select(
                    with_timeout(WIFI_CONNECT_TIMEOUT, controller.connect_async()),
                    status.wait_until_station_uplink_disabled(),
                )
                .await;
                let next = match connected {
                    embassy_futures::select::Either::First(Ok(Ok(connected))) => {
                        WIFI_STATION_JOINED.store(true, Ordering::Relaxed);
                        WIFI_STATION_DATA_PATH_DEGRADED.store(false, Ordering::Release);
                        boot_stage(BootPhase::WifiAssociated);
                        log::info!(
                            "wifi: station connected channel={} elapsed_ms={}",
                            connected.channel,
                            embassy_time::Instant::now()
                                .as_millis()
                                .saturating_sub(started_at)
                        );
                        let next = recovery.finish_connection(
                            attempt,
                            ConnectionOutcome::Connected(StationAccessPoint {
                                bssid: connected.bssid,
                                channel: connected.channel,
                            }),
                        );
                        if let Err(error) = controller.set_power_saving(PowerSaveMode::None) {
                            log::warn!("wifi: power-save configuration failed: {error:?}");
                        }
                        next
                    }
                    embassy_futures::select::Either::First(Ok(Err(error))) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        match error {
                            WifiError::Disconnected(disconnected) => log::warn!(
                                "wifi: station connection failed ({:?}, rssi {}) elapsed_ms={}",
                                disconnected.reason,
                                disconnected.rssi,
                                embassy_time::Instant::now()
                                    .as_millis()
                                    .saturating_sub(started_at)
                            ),
                            other => log::warn!(
                                "wifi: station connection failed: {other:?} elapsed_ms={}",
                                embassy_time::Instant::now()
                                    .as_millis()
                                    .saturating_sub(started_at)
                            ),
                        }
                        let failure = classify_connection_failure(error);
                        recovery.finish_connection(attempt, ConnectionOutcome::Failed(failure))
                    }
                    embassy_futures::select::Either::First(Err(_)) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        log::warn!(
                            "wifi: station connection timed out elapsed_ms={}",
                            embassy_time::Instant::now()
                                .as_millis()
                                .saturating_sub(started_at)
                        );
                        stop_station_connection().await;
                        recovery.finish_connection(
                            attempt,
                            ConnectionOutcome::Failed(ConnectionFailure::Timeout),
                        )
                    }
                    embassy_futures::select::Either::Second(()) => {
                        WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
                        stop_station_connection().await;
                        let next =
                            recovery.finish_connection(attempt, ConnectionOutcome::Cancelled);
                        recovery.resume_now();
                        next
                    }
                };
                apply_station_yield(next, &status).await;
            }
            StationAttempt::Scan(attempt) => {
                if ap_enabled && soft_ap_active {
                    // APSTA can use only one RF channel. Tear down the provisional/recovery AP
                    // while sweeping so a LAN on another channel remains discoverable; it comes
                    // back on the selected station channel before association begins.
                    let mode = WifiConfig::Station(base.clone());
                    if let Err(error) = controller.set_config(&mode) {
                        log::warn!("wifi: station discovery mode failed: {error:?}");
                        let next =
                            recovery.finish_scan(attempt, ScanOutcome::Failed(ScanFailure::Driver));
                        apply_station_yield(next, &status).await;
                        continue;
                    }
                    soft_ap_active = false;
                }
                let channel = attempt.channel();
                if attempt.starts_sweep() {
                    boot_stage(BootPhase::WifiDiscoveryBegin);
                    log::info!("wifi: discovery sweep begin");
                }
                let scan_config = ScanConfig::default()
                    .with_ssid(credentials.ssid.as_str())
                    .with_channel(channel)
                    .with_scan_type(ScanTypeConfig::Active {
                        min: WIFI_SCAN_MIN_DWELL,
                        max: WIFI_SCAN_MAX_DWELL,
                    })
                    .with_max(8);
                let scan = embassy_futures::select::select(
                    with_timeout(
                        WIFI_CHANNEL_SCAN_TIMEOUT,
                        controller.scan_async(&scan_config),
                    ),
                    status.wait_until_station_uplink_disabled(),
                )
                .await;
                let next = match scan {
                    embassy_futures::select::Either::First(Ok(Ok(networks))) => {
                        let best = networks
                            .iter()
                            .max_by_key(|access_point| access_point.signal_strength)
                            .map(|access_point| StationAccessPoint {
                                bssid: access_point.bssid,
                                channel: access_point.channel,
                            });
                        if best.is_some() || attempt.ends_sweep() {
                            boot_stage(BootPhase::WifiDiscoveryComplete);
                        }
                        if best.is_some() {
                            log::info!("wifi: discovery found channel={channel}");
                        } else if attempt.ends_sweep() {
                            log::warn!("wifi: configured network absent");
                        }
                        let outcome = best.map_or(ScanOutcome::NotFound, ScanOutcome::Found);
                        recovery.finish_scan(attempt, outcome)
                    }
                    embassy_futures::select::Either::First(Ok(Err(error))) => {
                        log::warn!("wifi: discovery scan failed channel={channel}: {error:?}");
                        stop_station_scan().await;
                        recovery.finish_scan(attempt, ScanOutcome::Failed(ScanFailure::Driver))
                    }
                    embassy_futures::select::Either::First(Err(_)) => {
                        log::warn!("wifi: discovery scan timed out channel={channel}");
                        stop_station_scan().await;
                        recovery.finish_scan(attempt, ScanOutcome::Failed(ScanFailure::Timeout))
                    }
                    embassy_futures::select::Either::Second(()) => {
                        stop_station_scan().await;
                        let next = recovery.finish_scan(attempt, ScanOutcome::Cancelled);
                        recovery.resume_now();
                        next
                    }
                };
                if ap_enabled && matches!(&next, StationYield::Retry(_) | StationYield::Disabled) {
                    // If the uplink is absent, keep the local provisioning/recovery AP available
                    // during backoff. The next sweep will briefly return to station-only mode.
                    match controller.set_config(&WifiConfig::AccessPoint(ap_config(None))) {
                        Ok(()) => soft_ap_active = true,
                        Err(error) => {
                            log::warn!("wifi: recovery SoftAP configuration failed: {error:?}")
                        }
                    }
                }
                apply_station_yield(next, &status).await;
            }
        }
    }
}

fn classify_connection_failure(error: WifiError) -> ConnectionFailure {
    match error {
        WifiError::InvalidPassword => ConnectionFailure::Authentication,
        WifiError::InvalidSsid => ConnectionFailure::NetworkNotFound,
        WifiError::Disconnected(disconnected) => match disconnected.reason {
            DisconnectReason::NoAccessPointFound
            | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
            | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
            | DisconnectReason::NoAccessPointFoundInRssiThreshold => {
                ConnectionFailure::NetworkNotFound
            }
            DisconnectReason::AuthenticationExpired
            | DisconnectReason::AssociationNotAuthenticated
            | DisconnectReason::FourWayHandshakeTimeout
            | DisconnectReason::GroupKeyUpdateTimeout
            | DisconnectReason::_802_1xAuthenticationFailed
            | DisconnectReason::AuthenticationFailed
            | DisconnectReason::HandshakeTimeout => ConnectionFailure::Authentication,
            DisconnectReason::Timeout | DisconnectReason::BeaconTimeout => {
                ConnectionFailure::Timeout
            }
            _ => ConnectionFailure::Driver,
        },
        _ => ConnectionFailure::Driver,
    }
}

async fn apply_station_yield(next: StationYield, status: &AutoWifiStatus<MEMBERS>) {
    match next {
        StationYield::Continue | StationYield::MonitorLink | StationYield::Disabled => {}
        StationYield::InterChannel => {
            let _ = embassy_futures::select::select(
                Timer::after(WIFI_INTER_CHANNEL_DELAY),
                status.wait_until_station_uplink_disabled(),
            )
            .await;
        }
        StationYield::Retry(delay) => {
            let delay_seconds = delay.seconds();
            log::info!("wifi: station recovery delay_secs={delay_seconds}");
            let _ = embassy_futures::select::select(
                Timer::after(Duration::from_secs(delay_seconds)),
                status.wait_until_station_uplink_disabled(),
            )
            .await;
        }
    }
}
