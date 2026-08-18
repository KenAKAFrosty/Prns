use super::super::captive_portal::station_wifi_mode;
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
        if state_changed || samples_until_report == 0 {
            let heap = esp_alloc::HEAP.stats();
            let data_path = esp_radio::wifi::data_path_diagnostics();
            let station_ready = associated && ready;
            let data_path_window = previous_data_path.as_ref().map(|earlier| {
                if data_path.transmit_submission_stalled_since(earlier) {
                    StationDataPathWindow::TransmitSubmissionStalled
                } else if data_path.receive_delivery_blocked_by_transmit_capacity_since(earlier) {
                    StationDataPathWindow::TransmitCapacityBlocked
                } else if data_path.station_receive_progressed_since(earlier) {
                    StationDataPathWindow::ReceiveProgress
                } else if data_path.transmit_progressed_without_station_receive_since(earlier) {
                    StationDataPathWindow::TransmitWithoutReceive
                } else {
                    StationDataPathWindow::NoProgress
                }
            });
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
            log::info!("wifi-data: {}", data_path);
            if station_ready {
                if let Some(data_path_window) = data_path_window {
                    if matches!(&data_path_window, StationDataPathWindow::ReceiveProgress) {
                        WIFI_STATION_DATA_PATH_DEGRADED.store(false, Ordering::Release);
                    }
                    match station_data_path_recovery.observe(data_path_window) {
                        StationDataPathAction::Continue => {}
                        StationDataPathAction::RestartDriver { count, cause } => {
                            WIFI_STATION_DATA_PATH_DEGRADED.store(true, Ordering::Release);
                            WIFI_DRIVER_RESTART_REQUESTED.store(true, Ordering::Release);
                            log::warn!("wifi-radio-trace: {data_path:?}");
                            log::warn!(
                                "wifi-health: station data path stalled cause={cause:?}; requested driver restart count={count}"
                            );
                        }
                    }
                }
            } else {
                station_data_path_recovery.station_unavailable();
            }
            previous_data_path = if station_ready { Some(data_path) } else { None };
            samples_until_report = WIFI_HEALTH_SAMPLES_BETWEEN_REPORTS;
        } else {
            samples_until_report = samples_until_report.saturating_sub(1);
        }
        previous_state = Some(state);
        Timer::after(WIFI_LINK_CHECK_INTERVAL).await;
    }
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
            if let Err(error) = controller.restart() {
                WIFI_DRIVER_RESTART_REQUESTED.store(true, Ordering::Release);
                log::warn!("wifi: data-path recovery driver restart failed: {error:?}");
                Timer::after(DRIVER_STOP_RETRY_DELAY).await;
                continue;
            }
            WIFI_STATION_JOINED.store(false, Ordering::Relaxed);
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
        if ap_enabled {
            let discovery_scope = match controller.channel() {
                Ok((channel, _)) => match DiscoveryScope::protected(channel) {
                    Some(discovery_scope) => Some(discovery_scope),
                    None => {
                        log::warn!("wifi: SoftAP channel is outside 2.4 GHz channel={channel}");
                        None
                    }
                },
                Err(error) => {
                    log::warn!("wifi: SoftAP channel query failed: {error:?}");
                    None
                }
            };
            let Some(discovery_scope) = discovery_scope else {
                apply_station_yield(StationYield::Retry(RecoveryDelay::TwoSeconds), &status).await;
                continue;
            };
            recovery.set_discovery_scope(discovery_scope);
        } else {
            recovery.set_discovery_scope(DiscoveryScope::FullBand);
        }
        let Some(attempt) = recovery.begin_attempt() else {
            Timer::after(DRIVER_STOP_RETRY_DELAY).await;
            continue;
        };
        match attempt {
            StationAttempt::Connect(attempt) => {
                let access_point = attempt.access_point();
                let station = base
                    .clone()
                    .with_bssid(access_point.bssid)
                    .with_channel(access_point.channel);
                let configured = {
                    let mode = station_wifi_mode(station, ap_enabled);
                    match controller.set_config(&mode) {
                        Ok(()) => true,
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
