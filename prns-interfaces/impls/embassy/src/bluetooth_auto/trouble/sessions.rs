use super::backend::{BleHub, DialTarget, SlotChannels, TransientClientRetryOutcome};
use super::*;
use prns_core::interfaces::bluetooth_auto::StreamDeframer;

/// Trouble allocates an ATT packet before awaiting its bounded outbound queue. A short GATT burst
/// can therefore observe the shared packet pool between the controller returning one packet and
/// the host task reclaiming it. That is backpressure, not a broken connection: yield so the host
/// can drain, then try again under the caller's operation deadline. Every other error remains
/// terminal because retrying malformed state or a vanished connection cannot make progress.
async fn notify_with_backpressure(
    characteristic: &GattCharacteristic,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
    value: &GattVec<u8, GATT_VALUE_CAP>,
) -> Result<(), trouble_host::Error> {
    loop {
        match characteristic.notify(connection, value).await {
            Err(trouble_host::Error::OutOfMemory) => yield_now().await,
            result => return result,
        }
    }
}

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

pub fn service_uuid() -> Uuid {
    reticulum_uuid(SERVICE_UUID_LAST)
}

pub fn control_uuid() -> Uuid {
    reticulum_uuid(CONTROL_UUID_LAST)
}

pub fn data_uuid() -> Uuid {
    reticulum_uuid(DATA_UUID_LAST)
}

pub fn columba_tx_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_TX_UUID_LAST)
}

pub fn columba_rx_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_RX_UUID_LAST)
}

pub fn columba_identity_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_IDENTITY_UUID_LAST)
}

pub fn reticulum_attribute_table(
    control_store: &'static mut [u8; GATT_VALUE_CAP],
    data_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_rx_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_tx_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_identity_store: &'static mut [u8; GATT_VALUE_CAP],
    identity: BleIdentity,
) -> Option<(
    ReticulumAttributeTable,
    GattCharacteristic,
    GattCharacteristic,
    GattCharacteristic,
    GattCharacteristic,
)> {
    let mut table: ReticulumAttributeTable = AttributeTable::new();
    if let Err(error) = GapConfig::Peripheral(PeripheralConfig {
        name: "Prns",
        appearance: &appearance::UNKNOWN,
    })
    .build(&mut table)
    {
        crate::diagnostic_log::warn!("ble gap config failed: {error}");
        return None;
    }
    let props = [
        CharacteristicProp::Write,
        CharacteristicProp::WriteWithoutResponse,
        CharacteristicProp::Notify,
    ];
    let (control, data, columba_rx, columba_tx) = {
        let mut service = table.add_service(Service::new(service_uuid()));
        let control = service
            .add_characteristic(
                control_uuid(),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                control_store,
            )
            .build();
        let data = service
            .add_characteristic(
                data_uuid(),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                data_store,
            )
            .build();
        let columba_rx = service
            .add_characteristic(
                columba_rx_uuid(),
                [
                    CharacteristicProp::Write,
                    CharacteristicProp::WriteWithoutResponse,
                ],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                columba_rx_store,
            )
            .build();
        let columba_tx = service
            .add_characteristic(
                columba_tx_uuid(),
                [CharacteristicProp::Read, CharacteristicProp::Notify],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                columba_tx_store,
            )
            .build();
        let mut identity_value = GattVec::<u8, GATT_VALUE_CAP>::new();
        identity_value.extend_from_slice(identity.as_bytes()).ok()?;
        service
            .add_characteristic(
                columba_identity_uuid(),
                [CharacteristicProp::Read],
                identity_value,
                columba_identity_store,
            )
            .build();
        service.build();
        (control, data, columba_rx, columba_tx)
    };
    Some((table, control, data, columba_rx, columba_tx))
}

fn l2cap_config() -> L2capChannelConfig {
    L2capChannelConfig {
        mtu: Some(L2CAP_SDU_LEN as u16),
        mps: Some(L2CAP_MPS),
        flow_policy: CreditFlowPolicy::default(),
        initial_credits: Some(L2CAP_CREDITS),
    }
}

#[derive(Debug, Eq, PartialEq)]
enum InboundFrameAdmission {
    Queued,
    PoolFull,
    QueueFull,
}

fn try_queue_inbound_frame(
    pool: &'static BleFramePool,
    queue: &Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    frame: &[u8],
) -> Result<InboundFrameAdmission, FramePoolError> {
    let lease = match pool.try_lease() {
        Ok(Some(lease)) => lease,
        Ok(None) => return Ok(InboundFrameAdmission::PoolFull),
        Err(error) => return Err(error),
    };
    lease.try_fill(frame)?;
    if queue.try_send(lease).is_err() {
        return Ok(InboundFrameAdmission::QueueFull);
    }
    Ok(InboundFrameAdmission::Queued)
}

async fn queue_inbound_frame(
    pool: &'static BleFramePool,
    queue: &Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    frame: &[u8],
) -> Result<(), FramePoolError> {
    let lease = pool.lease().await?;
    lease.fill(frame).await?;
    queue.send(lease).await;
    Ok(())
}

fn note_inbound_admission(hub: &BleHub, result: Result<InboundFrameAdmission, FramePoolError>) {
    match result {
        Ok(InboundFrameAdmission::Queued) => {}
        Ok(InboundFrameAdmission::PoolFull | InboundFrameAdmission::QueueFull) => {
            hub.note_ingress_pressure();
        }
        Err(error) => {
            hub.note_ingress_pressure();
            crate::diagnostic_log::warn!("ble inbound frame admission failed: {error:?}");
        }
    }
}

async fn l2cap_pump<T: TroubleTransport>(
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    channel: L2capChannel<'static, DefaultPacketPool>,
    inbound_frames: &'static BleFramePool,
    data_out_rx: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    data_in_tx: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
) -> L2capPumpExit {
    let (mut writer, mut reader) = channel.split();
    let outbound = async {
        let mut tx = alloc::boxed::Box::new([0u8; L2CAP_SDU_LEN]);
        loop {
            let frame = data_out_rx.receive().await;
            let frame = frame.lock().await;
            let Some(len) = encode_stream_frame(&frame, tx.as_mut()) else {
                continue;
            };
            let activity = hub.begin_busy_operation();
            let sent = writer.send(stack, &tx[..len]).await;
            drop(activity);
            if let Err(error) = sent {
                crate::diagnostic_log::warn!("ble: L2CAP send failed: {error:?}");
                return L2capPumpExit::Outbound;
            }
        }
    };
    let inbound = receive_l2cap_frames(hub, inbound_frames, data_in_tx, async |rx| {
        reader.receive(stack, rx).await
    });
    match select(outbound, inbound).await {
        Either::First(exit) | Either::Second(exit) => exit,
    }
}

async fn receive_l2cap_frames<E: core::fmt::Debug>(
    hub: &BleHub,
    inbound_frames: &'static BleFramePool,
    data_in_tx: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    mut receive: impl AsyncFnMut(&mut [u8]) -> Result<usize, E>,
) -> L2capPumpExit {
    let mut rx = alloc::boxed::Box::new([0u8; L2CAP_SDU_LEN]);
    // Stream peers may split a frame across SDUs or coalesce several into one.
    // One incomplete maximum frame plus the next SDU fits this bounded buffer.
    let mut deframer = alloc::boxed::Box::new(StreamDeframer::<{ 2 * L2CAP_SDU_LEN }>::new());
    loop {
        let read = match receive(rx.as_mut()).await {
            Ok(read) => read,
            Err(error) => {
                crate::diagnostic_log::warn!("ble: L2CAP receive failed: {error:?}");
                return L2capPumpExit::Inbound;
            }
        };
        hub.note_link_activity();
        if !deframer.absorb(&rx[..read]) {
            crate::diagnostic_log::warn!("ble: L2CAP stream framing buffer exceeded");
            return L2capPumpExit::Inbound;
        }
        // absorb copied the SDU, so reuse its scratch buffer for each decoded frame.
        while let Some(len) = deframer.next_frame(rx.as_mut()) {
            if len > FRAME_CAP {
                crate::diagnostic_log::warn!("ble: L2CAP frame exceeds the interface MTU");
                return L2capPumpExit::Inbound;
            }
            let frame = match inbound_frames.lease().await {
                Ok(frame) => frame,
                Err(error) => {
                    crate::diagnostic_log::warn!("ble L2CAP frame lease failed: {error:?}");
                    return L2capPumpExit::FramePool;
                }
            };
            if let Err(error) = frame.fill(&rx[..len]).await {
                crate::diagnostic_log::warn!("ble L2CAP frame fill failed: {error:?}");
                return L2capPumpExit::FramePool;
            }
            data_in_tx.send(frame).await;
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum L2capPumpExit {
    Outbound,
    Inbound,
    FramePool,
}

#[derive(Debug, Eq, PartialEq)]
enum SessionExit {
    ClientTask,
    GattServerTask,
    Inbound,
    ControlOutbound,
    DataPlane,
    WorkerClosed,
}

#[derive(Debug, Eq, PartialEq)]
enum GattServerExit {
    Disconnected,
    RequestFailed,
}

#[derive(Clone, Copy)]
pub struct ReticulumGattCharacteristics<'a> {
    pub control: &'a GattCharacteristic,
    pub data: &'a GattCharacteristic,
    pub columba_rx: &'a GattCharacteristic,
    pub columba_tx: &'a GattCharacteristic,
}

#[derive(Clone, Copy)]
pub struct ReticulumGattUuids<'a> {
    pub service: &'a Uuid,
    pub control: &'a Uuid,
    pub data: &'a Uuid,
    pub columba_rx: &'a Uuid,
    pub columba_tx: &'a Uuid,
    pub columba_identity: &'a Uuid,
}

pub(super) struct CentralGattSetup<'a> {
    pub(super) server: &'a GattServer,
    pub(super) uuids: ReticulumGattUuids<'a>,
}

#[derive(Clone, Copy, Debug)]
enum CentralSetupFailure {
    AttributeServerBinding,
    ClientCreation,
    ServiceDiscovery,
    NativeCharacteristicDiscovery,
    ColumbaFallbackDiscovery,
    IdentityRead,
    Subscription,
}

async fn serve_peer_gatt_requests(
    hub: &'static BleHub,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
) -> GattServerExit {
    loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { .. } => return GattServerExit::Disconnected,
            GattConnectionEvent::Gatt { event } => {
                let activity = hub.begin_busy_operation();
                let reply = match event.accept() {
                    Ok(reply) => reply,
                    Err(error) => {
                        crate::diagnostic_log::warn!(
                            "ble: dialed peer GATT request failed: {error:?}"
                        );
                        return GattServerExit::RequestFailed;
                    }
                };
                reply.send().await;
                drop(activity);
            }
            _ => {}
        }
    }
}

pub(super) async fn serve_peripheral<T: TroubleTransport>(
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    slot: &'static SlotChannels,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
    characteristics: ReticulumGattCharacteristics<'_>,
) {
    let setup_activity = hub.begin_busy_operation();
    let ReticulumGattCharacteristics {
        control,
        data,
        columba_rx,
        columba_tx,
    } = characteristics;
    prepare_accepted_connection(stack, connection).await;

    let peer_protocol = loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::Gatt { event } => {
                let activity = hub.begin_busy_operation();
                let protocol = match &event {
                    GattEvent::Write(write) if write.handle() == control.handle => {
                        match Control::decode(write.data()) {
                            Some(message) => {
                                slot.control_in.send(message).await;
                                Some(PeerProtocol::Native)
                            }
                            None => None,
                        }
                    }
                    GattEvent::Write(write)
                        if write.handle() == columba_rx.handle && write.data().len() == 16 =>
                    {
                        let mut bytes = [0u8; 16];
                        bytes.copy_from_slice(write.data());
                        slot.identity_in.signal(BleIdentity::new(bytes));
                        Some(PeerProtocol::Columba)
                    }
                    _ => None,
                };
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
                if let Some(protocol) = protocol {
                    drop(activity);
                    break protocol;
                }
            }
            _ => {}
        }
    };
    slot.set_peer_protocol(peer_protocol);
    tune_accepted_connection(hub, stack, connection.raw()).await;
    let initial_params = connection.raw().params();
    crate::diagnostic_log::info!(
        "ble: accepted GATT link ready protocol={peer_protocol:?} interval_ms={} latency={} supervision_ms={} att_mtu={}",
        initial_params.conn_interval.as_millis(),
        initial_params.peripheral_latency,
        initial_params.supervision_timeout.as_millis(),
        connection.raw().att_mtu(),
    );
    hub.ready.send(link.into_ready(Origin::Accepted)).await;

    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();

    let inbound = async move {
        let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
        loop {
            match connection.next().await {
                GattConnectionEvent::Disconnected { .. } => break,
                GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                    crate::diagnostic_log::info!(
                        "ble: accepted PHY updated tx={tx_phy:?} rx={rx_phy:?}"
                    );
                }
                GattConnectionEvent::DataLengthUpdated {
                    max_tx_octets,
                    max_tx_time,
                    max_rx_octets,
                    max_rx_time,
                } => {
                    crate::diagnostic_log::info!(
                        "ble: accepted data length tx={max_tx_octets}B/{max_tx_time}us rx={max_rx_octets}B/{max_rx_time}us"
                    );
                }
                GattConnectionEvent::Gatt { event } => {
                    let _activity = hub.begin_busy_operation();
                    if let GattEvent::Write(write) = &event {
                        let acknowledged = matches!(
                            write.payload().incoming(),
                            AttClient::Request(AttReq::Write { .. })
                        );
                        if peer_protocol == PeerProtocol::Native && write.handle() == control.handle
                        {
                            if let Some(message) = Control::decode(write.data()) {
                                control_in_tx.send(message).await;
                            }
                        } else if (peer_protocol == PeerProtocol::Native
                            && write.handle() == data.handle)
                            || (peer_protocol == PeerProtocol::Columba
                                && write.handle() == columba_rx.handle)
                        {
                            if let Some(fragment) = Fragment::decode(write.data()) {
                                if let Some(frame) = reassembler.absorb(&fragment) {
                                    if acknowledged {
                                        if let Err(error) = queue_inbound_frame(
                                            &hub.inbound_frames,
                                            &data_in_tx,
                                            frame,
                                        )
                                        .await
                                        {
                                            hub.note_ingress_pressure();
                                            crate::diagnostic_log::warn!(
                                                "ble acknowledged frame admission failed: {error:?}"
                                            );
                                        }
                                    } else {
                                        note_inbound_admission(
                                            hub,
                                            try_queue_inbound_frame(
                                                &hub.inbound_frames,
                                                &data_in_tx,
                                                frame,
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                _ => {}
            }
        }
    };

    let control_outbound = async move {
        if peer_protocol == PeerProtocol::Columba {
            core::future::pending::<()>().await;
        }
        loop {
            let message = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(len) = message.encode(&mut buf) {
                let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                let _ = value.extend_from_slice(&buf[..len]);
                let activity = hub.begin_busy_operation();
                let _ = with_timeout(
                    GATT_OPERATION_TIMEOUT,
                    notify_with_backpressure(control, connection, &value),
                )
                .await;
                drop(activity);
            }
        }
    };

    // Heap allocation keeps the L2CAP state machine out of the main-task future arena, which otherwise steals core-0 stack space.
    let data_lane = alloc::boxed::Box::pin(async move {
        let plan = slot.data_plane.wait().await;
        crate::diagnostic_log::debug!("ble: plan (accepted) = {plan:?}");
        let channel = match (peer_protocol, plan) {
            (PeerProtocol::Native, L2capPlan::Accept) => match with_timeout(
                L2CAP_HANDSHAKE_WINDOW,
                L2capChannel::accept(stack, connection.raw(), &[L2CAP_PSM], &l2cap_config()),
            )
            .await
            {
                Ok(Ok(channel)) => Some(channel),
                Ok(Err(e)) => {
                    crate::diagnostic_log::debug!("ble: L2CAP accept err: {e:?}");
                    None
                }
                Err(_) => {
                    crate::diagnostic_log::debug!("ble: L2CAP accept timed out");
                    None
                }
            },
            _ => None,
        };
        drop(setup_activity);
        match channel {
            Some(channel) => {
                crate::diagnostic_log::debug!("ble: L2CAP up (accepted)");
                let exit = l2cap_pump(
                    hub,
                    stack,
                    channel,
                    &hub.inbound_frames,
                    data_out_rx,
                    data_in_tx,
                )
                .await;
                crate::diagnostic_log::warn!("ble: accepted L2CAP pump ended: {exit:?}");
            }
            None => {
                let mut profiled_first_frame = false;
                loop {
                    let frame = data_out_rx.receive().await;
                    let frame = frame.lock().await;
                    let frame_started = Instant::now();
                    let mut sent_fragments = 0usize;
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                        let Some(len) = fragment.encode(&mut buf) else {
                            continue;
                        };
                        let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                        let _ = value.extend_from_slice(&buf[..len]);
                        let characteristic = match peer_protocol {
                            PeerProtocol::Native => data,
                            PeerProtocol::Columba => columba_tx,
                        };
                        let activity = hub.begin_busy_operation();
                        match with_timeout(
                            GATT_OPERATION_TIMEOUT,
                            notify_with_backpressure(characteristic, connection, &value),
                        )
                        .await
                        {
                            Ok(Ok(())) => sent_fragments += 1,
                            Ok(Err(error)) => {
                                crate::diagnostic_log::warn!(
                                    "ble: accepted GATT notify failed after {sent_fragments} fragments: {error:?}"
                                );
                                return;
                            }
                            Err(_) => {
                                crate::diagnostic_log::warn!(
                                    "ble: accepted GATT notify timed out after {sent_fragments} fragments"
                                );
                                return;
                            }
                        }
                        drop(activity);
                    }
                    if !profiled_first_frame {
                        crate::diagnostic_log::info!(
                            "ble: first accepted GATT frame bytes={} fragments={} submit_ms={}",
                            frame.len(),
                            sent_fragments,
                            frame_started.elapsed().as_millis(),
                        );
                        profiled_first_frame = true;
                    }
                }
            }
        }
    });

    let exit = match select4(
        inbound,
        control_outbound,
        data_lane,
        worker.wait_for_close(),
    )
    .await
    {
        Either4::First(()) => SessionExit::Inbound,
        Either4::Second(()) => SessionExit::ControlOutbound,
        Either4::Third(()) => SessionExit::DataPlane,
        Either4::Fourth(()) => SessionExit::WorkerClosed,
    };
    crate::diagnostic_log::warn!("ble: accepted session ended: {exit:?}");
}

pub(super) async fn serve_central<T: TroubleTransport>(
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    gatt: CentralGattSetup<'_>,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    connection: Connection<'static, DefaultPacketPool>,
    target: DialTarget,
) {
    let setup_activity = hub.begin_busy_operation();
    let slot = &hub.slots[link.index()];
    let addr = connection.peer_address().into_inner();
    let peer_gatt = match connection.clone().with_attribute_server(gatt.server) {
        Ok(connection) => connection,
        Err(error) => {
            crate::diagnostic_log::warn!(
                "ble: dialed GATT setup failed addr={addr:?}: {:?}: {error:?}",
                CentralSetupFailure::AttributeServerBinding,
            );
            connection.disconnect();
            hub.dial_failed.send(addr).await;
            return;
        }
    };
    let client = match with_timeout(
        GATT_OPERATION_TIMEOUT,
        GattClient::<TroubleController<T>, DefaultPacketPool, MAX_SERVICES>::new(
            stack,
            &connection,
        ),
    )
    .await
    {
        Ok(Ok(client)) => alloc::boxed::Box::new(client),
        Ok(Err(trouble_host::BleHostError::BleHost(trouble_host::Error::Disconnected))) => {
            connection.disconnect();
            match hub.retry_transient_client_disconnect(target) {
                TransientClientRetryOutcome::Queued => {
                    crate::diagnostic_log::debug!(
                        "ble: transient dialed GATT client disconnect retry queued addr={addr:?}"
                    );
                }
                TransientClientRetryOutcome::Exhausted => {
                    crate::diagnostic_log::warn!(
                        "ble: dialed GATT setup failed addr={addr:?}: {:?}: Disconnected after transient retries exhausted",
                        CentralSetupFailure::ClientCreation,
                    );
                    hub.dial_failed.send(addr).await;
                }
                TransientClientRetryOutcome::QueueBusy => {
                    crate::diagnostic_log::warn!(
                        "ble: dialed GATT setup failed addr={addr:?}: {:?}: Disconnected with transient retry queue busy",
                        CentralSetupFailure::ClientCreation,
                    );
                    hub.dial_failed.send(addr).await;
                }
            }
            return;
        }
        Ok(Err(error)) => {
            crate::diagnostic_log::warn!(
                "ble: dialed GATT setup failed addr={addr:?}: {:?}: {error:?}",
                CentralSetupFailure::ClientCreation,
            );
            connection.disconnect();
            hub.dial_failed.send(addr).await;
            return;
        }
        Err(_) => {
            crate::diagnostic_log::warn!(
                "ble: dialed GATT setup failed addr={addr:?}: {:?}: Timeout",
                CentralSetupFailure::ClientCreation,
            );
            connection.disconnect();
            hub.dial_failed.send(addr).await;
            return;
        }
    };

    let setup_stage = Cell::new(CentralSetupFailure::ServiceDiscovery);
    let discovered = with_timeout(GATT_SETUP_TIMEOUT, async {
        let discover = async {
            let services = client
                .services_by_uuid(gatt.uuids.service)
                .await
                .map_err(|_| CentralSetupFailure::ServiceDiscovery)?;
            let service = services
                .first()
                .cloned()
                .ok_or(CentralSetupFailure::ServiceDiscovery)?;
            setup_stage.set(CentralSetupFailure::NativeCharacteristicDiscovery);
            let native_control: Result<
                Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
                trouble_host::BleHostError<T::Error>,
            > = client
                .characteristic_by_uuid(&service, gatt.uuids.control)
                .await;
            match native_control {
                Ok(control) => {
                    let data: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                        .characteristic_by_uuid(&service, gatt.uuids.data)
                        .await
                        .map_err(|_| CentralSetupFailure::NativeCharacteristicDiscovery)?;
                    setup_stage.set(CentralSetupFailure::Subscription);
                    let control_listener = client
                        .subscribe(&control, false)
                        .await
                        .map_err(|_| CentralSetupFailure::Subscription)?;
                    let data_listener = client
                        .subscribe(&data, false)
                        .await
                        .map_err(|_| CentralSetupFailure::Subscription)?;
                    Ok((
                        PeerProtocol::Native,
                        control,
                        data,
                        Some(control_listener),
                        data_listener,
                        None,
                    ))
                }
                Err(trouble_host::BleHostError::BleHost(trouble_host::Error::NotFound)) => {
                    setup_stage.set(CentralSetupFailure::ColumbaFallbackDiscovery);
                    let rx: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                        .characteristic_by_uuid(&service, gatt.uuids.columba_rx)
                        .await
                        .map_err(|_| CentralSetupFailure::ColumbaFallbackDiscovery)?;
                    let tx: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                        .characteristic_by_uuid(&service, gatt.uuids.columba_tx)
                        .await
                        .map_err(|_| CentralSetupFailure::ColumbaFallbackDiscovery)?;
                    let identity: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                        .characteristic_by_uuid(&service, gatt.uuids.columba_identity)
                        .await
                        .map_err(|_| CentralSetupFailure::ColumbaFallbackDiscovery)?;
                    setup_stage.set(CentralSetupFailure::IdentityRead);
                    let mut bytes = [0u8; 16];
                    let read = client
                        .read_characteristic(&identity, &mut bytes)
                        .await
                        .map_err(|_| CentralSetupFailure::IdentityRead)?;
                    if read != bytes.len() {
                        return Err(CentralSetupFailure::IdentityRead);
                    }
                    setup_stage.set(CentralSetupFailure::Subscription);
                    let data_listener = client
                        .subscribe(&tx, false)
                        .await
                        .map_err(|_| CentralSetupFailure::Subscription)?;
                    Ok((
                        PeerProtocol::Columba,
                        rx.clone(),
                        rx,
                        None,
                        data_listener,
                        Some(BleIdentity::new(bytes)),
                    ))
                }
                Err(_) => Err(CentralSetupFailure::NativeCharacteristicDiscovery),
            }
        };
        match select3(
            discover,
            client.task(),
            serve_peer_gatt_requests(hub, &peer_gatt),
        )
        .await
        {
            Either3::First(result) => result,
            Either3::Second(_) | Either3::Third(_) => Err(setup_stage.get()),
        }
    })
    .await;
    let (peer_protocol, control, data, control_listener, mut data_listener, peer_identity) =
        match discovered {
            Ok(Ok(parts)) => parts,
            Ok(Err(failure)) => {
                crate::diagnostic_log::warn!(
                    "ble: dialed GATT setup failed addr={addr:?}: {failure:?}"
                );
                connection.disconnect();
                hub.dial_failed.send(addr).await;
                return;
            }
            Err(_) => {
                let failure = setup_stage.get();
                crate::diagnostic_log::warn!(
                    "ble: dialed GATT setup failed addr={addr:?}: {failure:?}"
                );
                connection.disconnect();
                hub.dial_failed.send(addr).await;
                return;
            }
        };

    slot.set_peer_addr(addr);
    slot.set_peer_protocol(peer_protocol);
    if let Some(peer_identity) = peer_identity {
        slot.identity_in.signal(peer_identity);
    }
    tune_dialed_connection(hub, stack, &connection).await;
    let initial_params = connection.params();
    crate::diagnostic_log::info!(
        "ble: dialed GATT link ready addr={addr:?} protocol={peer_protocol:?} interval_ms={} latency={} supervision_ms={} att_mtu={}",
        initial_params.conn_interval.as_millis(),
        initial_params.peripheral_latency,
        initial_params.supervision_timeout.as_millis(),
        connection.att_mtu(),
    );
    let mut reassembler = alloc::boxed::Box::new(Reassembler::<GATT_REASSEMBLY_CAP>::new());
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let data_in_tx_l2cap = slot.data_in.sender();
    hub.ready.send(link.into_ready(Origin::Dialed)).await;

    let inbound = async {
        match control_listener {
            Some(mut control_listener) => loop {
                match select(control_listener.next(), data_listener.next()).await {
                    Either::First(notification) => {
                        hub.note_link_activity();
                        if let Some(message) = Control::decode(notification.as_ref()) {
                            control_in_tx.send(message).await;
                        }
                    }
                    Either::Second(notification) => {
                        hub.note_link_activity();
                        if let Some(fragment) = Fragment::decode(notification.as_ref()) {
                            if let Some(frame) = reassembler.absorb(&fragment) {
                                note_inbound_admission(
                                    hub,
                                    try_queue_inbound_frame(
                                        &hub.inbound_frames,
                                        &data_in_tx,
                                        frame,
                                    ),
                                );
                            }
                        }
                    }
                }
            },
            None => loop {
                let notification = data_listener.next().await;
                hub.note_link_activity();
                if let Some(fragment) = Fragment::decode(notification.as_ref()) {
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        note_inbound_admission(
                            hub,
                            try_queue_inbound_frame(&hub.inbound_frames, &data_in_tx, frame),
                        );
                    }
                }
            },
        }
    };

    let control_outbound = async {
        if peer_protocol == PeerProtocol::Columba {
            let identity = slot.identity_out.receive().await;
            let activity = hub.begin_busy_operation();
            let _ = with_timeout(
                GATT_OPERATION_TIMEOUT,
                client.write_characteristic(&control, identity.as_bytes()),
            )
            .await;
            drop(activity);
            core::future::pending::<()>().await;
        }
        loop {
            let message = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(len) = message.encode(&mut buf) {
                let activity = hub.begin_busy_operation();
                let _ = with_timeout(
                    GATT_OPERATION_TIMEOUT,
                    client.write_characteristic(&control, &buf[..len]),
                )
                .await;
                drop(activity);
            }
        }
    };

    // Heap allocation keeps the L2CAP state machine out of the main-task future arena and preserves core-0 stack space.
    let data_lane = alloc::boxed::Box::pin(async {
        let plan = slot.data_plane.wait().await;
        crate::diagnostic_log::debug!("ble: plan (dialed) = {plan:?}");
        let channel = match (peer_protocol, plan) {
            (PeerProtocol::Native, L2capPlan::Open { psm }) => {
                let opened = with_timeout(L2CAP_HANDSHAKE_WINDOW, async {
                    loop {
                        match L2capChannel::create(stack, &connection, psm.get(), &l2cap_config())
                            .await
                        {
                            Ok(channel) => break channel,
                            Err(e) => crate::diagnostic_log::debug!("ble: L2CAP create err: {e:?}"),
                        }
                        Timer::after(L2CAP_SETUP_RETRY).await;
                    }
                })
                .await;
                if opened.is_err() {
                    crate::diagnostic_log::debug!(
                        "ble: L2CAP create timed out (peer never accepted)"
                    );
                }
                opened.ok()
            }
            _ => None,
        };
        drop(setup_activity);
        match channel {
            Some(channel) => {
                crate::diagnostic_log::debug!("ble: L2CAP up (opened)");
                let exit = l2cap_pump(
                    hub,
                    stack,
                    channel,
                    &hub.inbound_frames,
                    data_out_rx,
                    data_in_tx_l2cap,
                )
                .await;
                crate::diagnostic_log::warn!("ble: dialed L2CAP pump ended: {exit:?}");
            }
            None => {
                let mut profiled_first_frame = false;
                loop {
                    let frame = data_out_rx.receive().await;
                    let frame = frame.lock().await;
                    let frame_started = Instant::now();
                    let mut sent_fragments = 0usize;
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                        let Some(len) = fragment.encode(&mut buf) else {
                            continue;
                        };
                        let written = match peer_protocol {
                            PeerProtocol::Native => {
                                let activity = hub.begin_busy_operation();
                                let written = with_timeout(
                                    GATT_OPERATION_TIMEOUT,
                                    client.write_characteristic(&data, &buf[..len]),
                                )
                                .await;
                                drop(activity);
                                written
                            }
                            PeerProtocol::Columba => {
                                let activity = hub.begin_busy_operation();
                                let written = with_timeout(
                                    GATT_OPERATION_TIMEOUT,
                                    client
                                        .write_characteristic_without_response(&data, &buf[..len]),
                                )
                                .await;
                                drop(activity);
                                written
                            }
                        };
                        match written {
                            Ok(Ok(())) => sent_fragments += 1,
                            Ok(Err(error)) => {
                                crate::diagnostic_log::warn!(
                                    "ble: dialed GATT write failed after {sent_fragments} fragments: {error:?}"
                                );
                                return;
                            }
                            Err(_) => {
                                crate::diagnostic_log::warn!(
                                    "ble: dialed GATT write timed out after {sent_fragments} fragments"
                                );
                                return;
                            }
                        }
                    }
                    if !profiled_first_frame {
                        crate::diagnostic_log::info!(
                            "ble: first dialed GATT frame bytes={} fragments={} submit_ms={}",
                            frame.len(),
                            sent_fragments,
                            frame_started.elapsed().as_millis(),
                        );
                        profiled_first_frame = true;
                    }
                }
            }
        }
    });

    let exit = match select4(
        select(client.task(), serve_peer_gatt_requests(hub, &peer_gatt)),
        inbound,
        select(control_outbound, data_lane),
        worker.wait_for_close(),
    )
    .await
    {
        Either4::First(Either::First(result)) => {
            crate::diagnostic_log::warn!(
                "ble: dialed GATT client task ended addr={addr:?}: {result:?}"
            );
            SessionExit::ClientTask
        }
        Either4::First(Either::Second(exit)) => {
            crate::diagnostic_log::warn!(
                "ble: dialed peer GATT server task ended addr={addr:?}: {exit:?}"
            );
            SessionExit::GattServerTask
        }
        Either4::Second(()) => SessionExit::Inbound,
        Either4::Third(Either::First(())) => SessionExit::ControlOutbound,
        Either4::Third(Either::Second(())) => SessionExit::DataPlane,
        Either4::Fourth(()) => SessionExit::WorkerClosed,
    };
    crate::diagnostic_log::warn!("ble: dialed session ended addr={addr:?}: {exit:?}");
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use prns_core::interfaces::InterfaceId;

    use super::*;
    use crate::bluetooth_auto::BluetoothAutoShared;

    fn receive_chunks(chunks: &[&[u8]]) -> (L2capPumpExit, std::vec::Vec<std::vec::Vec<u8>>) {
        static SHARED: BluetoothAutoShared<PEER_CAPACITY> =
            BluetoothAutoShared::new(InterfaceId::new([0x5a; 8]));
        let hub = BleHub::new(BluetoothAutoStatus::new(&SHARED));
        let pool = alloc::boxed::Box::leak(alloc::boxed::Box::new(BleFramePool::new()));
        let queue = alloc::boxed::Box::leak(alloc::boxed::Box::new(Channel::new()));
        let mut chunks = chunks.iter();
        let mut receive = pin!(receive_l2cap_frames(
            &hub,
            pool,
            queue.sender(),
            async |out| {
                let Some(chunk) = chunks.next() else {
                    return Err(trouble_host::Error::Disconnected);
                };
                out[..chunk.len()].copy_from_slice(chunk);
                Ok(chunk.len())
            }
        ));
        let mut context = Context::from_waker(Waker::noop());
        let mut frames = std::vec::Vec::new();
        for _ in 0..1024 {
            let result = receive.as_mut().poll(&mut context);
            while let Ok(frame) = queue.try_receive() {
                frames.push(embassy_futures::block_on(frame.lock()).to_vec());
            }
            if let Poll::Ready(exit) = result {
                return (exit, frames);
            }
        }
        panic!("bounded L2CAP receiver fixture did not finish");
    }

    fn stream_frame(payload: &[u8]) -> std::vec::Vec<u8> {
        let mut wire = std::vec![0; STREAM_FRAME_PREFIX_LEN + payload.len()];
        encode_stream_frame(payload, &mut wire).unwrap();
        wire
    }

    #[test]
    fn l2cap_receiver_preserves_coalesced_frames() {
        let first = [0x75; 100];
        let second = [0xa4; 100];
        let mut wire = stream_frame(&first);
        wire.extend_from_slice(&stream_frame(&second));
        let (exit, frames) = receive_chunks(&[&wire]);
        assert_eq!(exit, L2capPumpExit::Inbound);
        assert_eq!(frames, [first.to_vec(), second.to_vec()]);
    }

    #[test]
    fn l2cap_receiver_preserves_split_prefix() {
        let payload = [0x75; 100];
        let wire = stream_frame(&payload);
        let (_, frames) = receive_chunks(&[&wire[..1], &wire[1..]]);
        assert_eq!(frames, [payload.to_vec()]);
    }

    #[test]
    fn l2cap_receiver_preserves_split_body() {
        let payload = [0x75; 100];
        let wire = stream_frame(&payload);
        let (_, frames) = receive_chunks(&[&wire[..31], &wire[31..]]);
        assert_eq!(frames, [payload.to_vec()]);
    }

    #[test]
    fn l2cap_receiver_preserves_a_maximum_frame_at_every_split_boundary() {
        let payload = [0x42; FRAME_CAP];
        let wire = stream_frame(&payload);
        for split in 1..wire.len() {
            let (_, frames) = receive_chunks(&[&wire[..split], &wire[split..]]);
            assert_eq!(frames, [payload.to_vec()], "split at byte {split}");
        }
    }

    #[test]
    fn l2cap_receiver_fits_a_partial_maximum_frame_and_the_next_sdu() {
        let first = [0x42; FRAME_CAP];
        let second = [0x65; FRAME_CAP];
        let mut wire = stream_frame(&first);
        wire.extend_from_slice(&stream_frame(&second));
        let (_, frames) = receive_chunks(&[
            &wire[..L2CAP_SDU_LEN - 1],
            &wire[L2CAP_SDU_LEN - 1..2 * L2CAP_SDU_LEN - 1],
            &wire[2 * L2CAP_SDU_LEN - 1..],
        ]);
        assert_eq!(frames, [first.to_vec(), second.to_vec()]);
    }

    #[test]
    fn l2cap_receiver_discards_only_the_truncated_tail_when_the_reader_closes() {
        let first = stream_frame(b"complete");
        let second = stream_frame(b"incomplete");
        for end in 0..second.len() {
            let mut wire = first.clone();
            wire.extend_from_slice(&second[..end]);
            let (exit, frames) = receive_chunks(&[&wire]);
            assert_eq!(exit, L2capPumpExit::Inbound);
            assert_eq!(frames, [b"complete".to_vec()]);
        }
    }

    #[test]
    fn l2cap_receiver_keeps_empty_sdu_and_empty_frame_semantics() {
        let mut wire = stream_frame(b"");
        wire.extend_from_slice(&stream_frame(b"next"));
        let (_, frames) = receive_chunks(&[&[], &wire]);
        assert_eq!(frames, [b"".to_vec(), b"next".to_vec()]);
    }

    #[test]
    fn l2cap_receiver_rejects_a_frame_larger_than_the_pool_capacity() {
        let mut wire = stream_frame(&[0x42; FRAME_CAP + 1]);
        wire.extend_from_slice(&stream_frame(b"must not resynchronize"));
        let (exit, frames) = receive_chunks(&[&wire[..L2CAP_SDU_LEN], &wire[L2CAP_SDU_LEN..]]);
        assert_eq!(exit, L2capPumpExit::Inbound);
        assert!(frames.is_empty());
    }

    #[test]
    fn l2cap_receiver_closes_on_bounded_overflow_without_admitting_malformed_data() {
        // An impossible 65535-byte declaration must not grow storage or allow later
        // bytes to be interpreted as a fresh frame after the buffer is exhausted.
        let malformed = [0xff; L2CAP_SDU_LEN];
        let valid = stream_frame(b"must not resynchronize");
        let (exit, frames) = receive_chunks(&[&malformed, &malformed, &[0xff], &valid]);
        assert_eq!(exit, L2capPumpExit::Inbound);
        assert!(frames.is_empty());
    }

    #[test]
    fn l2cap_receiver_waits_for_pool_capacity_before_reading_more() {
        static SHARED: BluetoothAutoShared<PEER_CAPACITY> =
            BluetoothAutoShared::new(InterfaceId::new([0x5b; 8]));
        static POOL: BleFramePool = BleFramePool::new();
        static QUEUE: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH> = Channel::new();
        let hub = BleHub::new(BluetoothAutoStatus::new(&SHARED));
        let mut held = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        for _ in 0..PEER_CAPACITY {
            assert!(held.push(POOL.try_lease().unwrap().unwrap()).is_ok());
        }
        let reads = core::cell::Cell::new(0);
        let wire = stream_frame(b"wait for capacity");
        let mut receive = pin!(receive_l2cap_frames(
            &hub,
            &POOL,
            QUEUE.sender(),
            async |out| {
                reads.set(reads.get() + 1);
                if reads.get() != 1 {
                    return Err(trouble_host::Error::Disconnected);
                }
                out[..wire.len()].copy_from_slice(&wire);
                Ok(wire.len())
            }
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(receive.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(reads.get(), 1);
        assert!(QUEUE.try_receive().is_err());
        assert!(POOL.try_lease().unwrap().is_none());

        drop(held.pop());
        assert_eq!(
            receive.as_mut().poll(&mut context),
            Poll::Ready(L2capPumpExit::Inbound)
        );
        let frame = QUEUE.try_receive().unwrap();
        assert_eq!(
            &*embassy_futures::block_on(frame.lock()),
            b"wait for capacity"
        );
        drop(frame);
        drop(held);
        let mut recovered = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        for _ in 0..PEER_CAPACITY {
            assert!(recovered.push(POOL.try_lease().unwrap().unwrap()).is_ok());
        }
        assert!(POOL.try_lease().unwrap().is_none());
    }

    #[test]
    fn l2cap_receiver_preserves_coalesced_frames_across_queue_backpressure() {
        let mut wire = std::vec::Vec::new();
        let expected: std::vec::Vec<_> = (0..FRAME_QUEUE_DEPTH + 3)
            .map(|index| std::vec![index as u8; 20])
            .collect();
        for frame in &expected {
            wire.extend_from_slice(&stream_frame(frame));
        }
        // The fixture polls the real receiver, then consumes the real queue. More
        // coalesced frames than queue slots must block, not disappear or overflow.
        let (_, frames) = receive_chunks(&[&wire]);
        assert_eq!(frames, expected);
    }

    #[test]
    fn l2cap_receiver_cancellation_releases_a_frame_waiting_for_queue_capacity() {
        static SHARED: BluetoothAutoShared<PEER_CAPACITY> =
            BluetoothAutoShared::new(InterfaceId::new([0x5c; 8]));
        static POOL: BleFramePool = BleFramePool::new();
        static QUEUE: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH> = Channel::new();
        let hub = BleHub::new(BluetoothAutoStatus::new(&SHARED));
        for _ in 0..FRAME_QUEUE_DEPTH {
            assert!(QUEUE.try_send(POOL.try_lease().unwrap().unwrap()).is_ok());
        }
        let reads = core::cell::Cell::new(0);
        let wire = stream_frame(b"waiting for queue");
        let mut receive = alloc::boxed::Box::pin(receive_l2cap_frames(
            &hub,
            &POOL,
            QUEUE.sender(),
            async |out| {
                reads.set(reads.get() + 1);
                if reads.get() != 1 {
                    return Err(trouble_host::Error::Disconnected);
                }
                out[..wire.len()].copy_from_slice(&wire);
                Ok(wire.len())
            },
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(receive.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(reads.get(), 1);
        let mut held = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        while let Some(frame) = POOL.try_lease().unwrap() {
            assert!(held.push(frame).is_ok());
        }
        assert_eq!(held.len(), PEER_CAPACITY - FRAME_QUEUE_DEPTH - 1);

        drop(receive);
        assert!(held.push(POOL.try_lease().unwrap().unwrap()).is_ok());
        assert!(POOL.try_lease().unwrap().is_none());
        while let Ok(frame) = QUEUE.try_receive() {
            drop(frame);
        }
        drop(held);
        let mut recovered = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        for _ in 0..PEER_CAPACITY {
            assert!(recovered.push(POOL.try_lease().unwrap().unwrap()).is_ok());
        }
    }

    #[test]
    fn initial_credits_cover_exactly_one_largest_l2cap_sdu() {
        let required_wire_bytes = L2CAP_SDU_LEN + usize::from(L2CAP_SDU_LENGTH_PREFIX_LEN);
        let credited_wire_bytes = usize::from(L2CAP_CREDITS) * usize::from(L2CAP_MPS);
        let one_fewer_credit = usize::from(L2CAP_CREDITS - 1) * usize::from(L2CAP_MPS);

        assert!(credited_wire_bytes >= required_wire_bytes);
        assert!(one_fewer_credit < required_wire_bytes);
        assert_eq!(l2cap_config().initial_credits, Some(L2CAP_CREDITS));
    }

    #[test]
    fn acknowledged_admission_waits_for_owned_capacity() {
        static POOL: BleFramePool = BleFramePool::new();
        static QUEUE: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH> = Channel::new();

        let mut held = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        for _ in 0..PEER_CAPACITY {
            let lease = POOL.try_lease().unwrap().unwrap();
            assert!(held.push(lease).is_ok());
        }

        let sender = QUEUE.sender();
        let mut admission = pin!(queue_inbound_frame(&POOL, &sender, b"frame"));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            admission.as_mut().poll(&mut context),
            Poll::Pending
        ));

        drop(held.pop());
        embassy_futures::block_on(admission.as_mut()).unwrap();
        drop(QUEUE.try_receive().unwrap());
    }

    #[test]
    fn unacknowledged_admission_reports_pool_pressure() {
        static POOL: BleFramePool = BleFramePool::new();
        static QUEUE: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH> = Channel::new();

        let mut held = heapless_09::Vec::<_, PEER_CAPACITY>::new();
        for _ in 0..PEER_CAPACITY {
            let lease = POOL.try_lease().unwrap().unwrap();
            assert!(held.push(lease).is_ok());
        }

        assert_eq!(
            try_queue_inbound_frame(&POOL, &QUEUE.sender(), b"frame"),
            Ok(InboundFrameAdmission::PoolFull)
        );
    }

    #[test]
    fn legacy_pressure_reaches_status() {
        static SHARED: BluetoothAutoShared<PEER_CAPACITY> =
            BluetoothAutoShared::new(InterfaceId::new([0x55; 8]));

        let status = BluetoothAutoStatus::new(&SHARED);
        let hub = BleHub::new(status);
        note_inbound_admission(&hub, Ok(InboundFrameAdmission::QueueFull));

        assert_eq!(status.ingress_pressure_events(), 1);
    }
}
