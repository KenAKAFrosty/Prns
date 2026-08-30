use super::backend::{BleHub, ScanFunnel, SlotJob};
use super::discovery::{
    advertisement_parameters, connect_scan_parameters, idle_scan_parameters, DiscoveryRole,
};
use super::sessions::{serve_central, serve_peripheral, CentralGattSetup};
use super::*;
use prns_core::interfaces::bluetooth_auto::{
    encode_advertisement, BleRoleCapabilities, MAX_ADVERTISEMENT_LEN,
};

pub async fn serve_slot<T: TroubleTransport>(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    server: &GattServer,
    characteristics: ReticulumGattCharacteristics<'_>,
    uuids: ReticulumGattUuids<'_>,
) {
    let slot = &hub.slots[idx];
    loop {
        let job = hub.assign[idx].receive().await;
        slot.clear_lanes();
        if !hub.radio_enabled.load(Ordering::Relaxed) {
            drop(job);
            continue;
        }
        let _live_link = hub.track_live_link();
        match job {
            SlotJob::Accept {
                connection,
                slot: lease,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                slot.set_peer_addr(connection.peer_address().into_inner());
                match connection.with_attribute_server(server) {
                    Ok(connection) => {
                        let _ = select(
                            slot.shutdown.wait(),
                            serve_peripheral(
                                hub,
                                stack,
                                slot,
                                link,
                                &worker,
                                &connection,
                                characteristics,
                            ),
                        )
                        .await;
                    }
                    Err(error) => {
                        crate::diagnostic_log::warn!("ble attribute server bind failed: {error:?}")
                    }
                }
            }
            SlotJob::Dial {
                connection,
                slot: lease,
                target,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                let _ = select(
                    slot.shutdown.wait(),
                    serve_central(
                        hub,
                        stack,
                        CentralGattSetup { server, uuids },
                        link,
                        &worker,
                        connection,
                        target,
                    ),
                )
                .await;
            }
        }
    }
}

pub async fn acceptor<T: TroubleTransport>(
    hub: &'static BleHub,
    peripheral: &mut Peripheral<'static, TroubleController<T>, DefaultPacketPool>,
) {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        let lease = match select(hub.connection_slots.acquire(), hub.advertise.wait()).await {
            Either::First(Ok(lease)) => lease,
            Either::First(Err(error)) => {
                crate::diagnostic_log::warn!("ble connection slot acquisition failed: {error:?}");
                Timer::after(Duration::from_millis(500)).await;
                continue;
            }
            Either::Second(state) => {
                enabled = state;
                continue;
            }
        };
        let idx = lease.index();
        let turn = hub.acquire_discovery_turn().await;
        let window = match hub
            .await_discovery_turn(&hub.advertise, DiscoveryRole::Advertise)
            .await
        {
            Ok(window) => window,
            Err(state) => {
                enabled = state;
                continue;
            }
        };
        let radio = hub.acquire_radio().await;
        let mut adv_data = [0u8; MAX_ADVERTISEMENT_LEN];
        let adv_len = encode_advertisement(
            &mut adv_data,
            BleRoleCapabilities::DualRole,
            hub.discovery_group_tag(),
        )
        .unwrap_or(0);
        let advertiser = match peripheral
            .advertise(
                &advertisement_parameters(window),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_data[..adv_len],
                    scan_data: &[],
                },
            )
            .await
        {
            Ok(advertiser) => advertiser,
            Err(error) => {
                crate::diagnostic_log::warn!("ble advertise failed: {error:?}");
                hub.finish_discovery_turn(window);
                drop(radio);
                drop(turn);
                Timer::after(Duration::from_millis(500)).await;
                continue;
            }
        };
        match select3(
            advertiser.accept(),
            Timer::after(window.advertising_duration()),
            hub.advertise.wait(),
        )
        .await
        {
            Either3::First(Ok(connection)) => {
                hub.assign[idx]
                    .send(SlotJob::Accept {
                        connection,
                        slot: lease,
                    })
                    .await;
            }
            Either3::First(Err(error)) => {
                crate::diagnostic_log::warn!("ble accept failed: {error:?}");
            }
            Either3::Second(()) => {}
            Either3::Third(state) => {
                enabled = state;
            }
        }
        hub.finish_discovery_turn(window);
        drop(radio);
        drop(turn);
        Timer::after(DISCOVERY_TURN_REST).await;
    }
}

pub async fn dialer<T: TroubleTransport>(
    hub: &'static BleHub,
    mut central: Central<'static, TroubleController<T>, DefaultPacketPool>,
) {
    let mut enabled = false;
    let mut scan_failure_reported = false;
    loop {
        if !enabled {
            enabled = hub.scan_enabled.wait().await;
            continue;
        }
        let turn = hub.acquire_discovery_turn().await;
        let window = match hub
            .await_discovery_turn(&hub.scan_enabled, DiscoveryRole::Scan)
            .await
        {
            Ok(window) => window,
            Err(state) => {
                enabled = state;
                continue;
            }
        };
        let radio = hub.acquire_radio().await;
        let (scan_interval, scan_window) = idle_scan_parameters(window);
        let mut scanner = Scanner::new(central);
        let target = {
            match scanner
                .scan(&ScanConfig {
                    active: false,
                    interval: scan_interval,
                    window: scan_window,
                    ..Default::default()
                })
                .await
            {
                Ok(_session) => {
                    scan_failure_reported = false;
                    match select3(
                        hub.dial_request.receive(),
                        Timer::after(window.scanning_duration()),
                        hub.scan_enabled.wait(),
                    )
                    .await
                    {
                        Either3::First(target) => Some(target),
                        Either3::Second(()) => None,
                        Either3::Third(state) => {
                            enabled = state;
                            None
                        }
                    }
                }
                Err(error) => {
                    if scan_failure_reported {
                        crate::diagnostic_log::debug!("ble scan still failing: {error:?}");
                    } else {
                        crate::diagnostic_log::warn!("ble scan failed: {error:?}");
                        scan_failure_reported = true;
                    }
                    Timer::after(Duration::from_millis(500)).await;
                    None
                }
            }
        };
        central = scanner.into_inner();
        if let Some(target) = target {
            let lease = match hub.connection_slots.try_acquire() {
                Ok(Some(lease)) => lease,
                Ok(None) => {
                    hub.dial_failed.send(target.addr.into_inner()).await;
                    hub.finish_discovery_turn(window);
                    drop(radio);
                    drop(turn);
                    Timer::after(DISCOVERY_TURN_REST).await;
                    continue;
                }
                Err(error) => {
                    crate::diagnostic_log::warn!("ble connection slot claim failed: {error:?}");
                    hub.dial_failed.send(target.addr.into_inner()).await;
                    hub.finish_discovery_turn(window);
                    drop(radio);
                    drop(turn);
                    Timer::after(DISCOVERY_TURN_REST).await;
                    continue;
                }
            };
            let idx = lease.index();
            let bd = target.addr;
            let whitelist = [(target.kind, &bd)];
            let mut config = ConnectConfig {
                scan_config: ScanConfig {
                    active: false,
                    filter_accept_list: &whitelist,
                    ..Default::default()
                },
                connect_params: preferred_conn_params(),
            };
            let (connect_timeout, connect_interval, connect_window) =
                connect_scan_parameters(window);
            config.scan_config.timeout = connect_timeout;
            config.scan_config.interval = connect_interval;
            config.scan_config.window = connect_window;
            match select(
                with_timeout(connect_timeout, central.connect(&config)),
                hub.scan_enabled.wait(),
            )
            .await
            {
                Either::First(Ok(Ok(connection))) => {
                    crate::diagnostic_log::debug!(
                        "ble: physical dial connected addr={:?}",
                        bd.into_inner(),
                    );
                    hub.assign[idx]
                        .send(SlotJob::Dial {
                            connection,
                            slot: lease,
                            target,
                        })
                        .await;
                }
                Either::First(Ok(Err(error))) => {
                    crate::diagnostic_log::warn!(
                        "ble: physical dial failed addr={:?}: {error:?}",
                        bd.into_inner(),
                    );
                    hub.dial_failed.send(bd.into_inner()).await;
                }
                Either::First(Err(_)) => {
                    crate::diagnostic_log::warn!(
                        "ble: physical dial timed out addr={:?} timeout_ms={}",
                        bd.into_inner(),
                        connect_timeout.as_millis(),
                    );
                    hub.dial_failed.send(bd.into_inner()).await;
                }
                Either::Second(state) => enabled = state,
            }
        }
        hub.finish_discovery_turn(window);
        drop(radio);
        drop(turn);
        Timer::after(DISCOVERY_TURN_REST).await;
    }
}

pub async fn host_runner<T: TroubleTransport>(
    hub: &'static BleHub,
    mut runner: Runner<'static, TroubleController<T>, DefaultPacketPool>,
) {
    let funnel = ScanFunnel {
        hub,
        local_address: BleAddress::from_hci_bytes(hub.local_address.lock(|cell| cell.get())),
    };
    loop {
        if let Err(error) = runner.run_with_handler(&funnel).await {
            crate::diagnostic_log::warn!("ble host runner exited: {error:?}");
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}
