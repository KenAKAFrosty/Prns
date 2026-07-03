//! The Windows (WinRT) backend for the native Reticulum-over-BLE auto-interface.
//!
//! GATT-only by mandate: WinRT exposes no app-level L2CAP, so [`arrangement`] pins every Windows
//! pair to `GattOnly` and `upgrade()` is a permanent no-op. The GATT-data floor carries every
//! frame; this is a complete, correct backend, not a degraded one. The shared brain lives in the
//! engine's `BluetoothAuto` supervisor; this backend only drives the radio and the seam,
//! mirroring `prns-ffi/src/ble/macos.rs`.
//!
//! WinRT activation and async completion need an initialized COM apartment, so a dedicated thread
//! joins the process MTA, brings the adapter up, publishes the GATT service, and parks for the
//! backend's lifetime. WinRT event handlers (firing on the system threadpool) post into tokio
//! channels the async consumer drains, the callback-world-to-reactor bridge the macOS backend
//! uses with GCD. The runtime classes are agile, so the published service and dialled GATT
//! clients are driven from the async side.
//!
//! Both roles are implemented: as **central** we dial a sighted peer, discover its control + data
//! characteristics, subscribe, and write to them; as **peripheral** a peer's writes to our local
//! characteristics feed the link's channels and we answer by notifying those same characteristics,
//! targeting the peer's subscribed client. One [`WinGattLink`] serves both, inverting
//! send/receive by [`LinkPlane`].
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::core::{
    fragments_of, BleAddress, BleUuid, Control, Dialect, Fragment, L2capPlan, Reassembler,
    BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
use prns_core::interfaces::bluetooth_auto::limits;
use prns_core::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

use tokio::sync::{mpsc as tokio_mpsc, oneshot, watch};
use tokio::task::JoinSet;

use windows::core::IInspectable;
use windows::core::GUID;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStatus, BluetoothLEAdvertisementWatcherStoppedEventArgs,
    BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Bluetooth::BluetoothCacheMode;
use windows::Devices::Bluetooth::BluetoothConnectionStatus;
use windows::Devices::Bluetooth::BluetoothError;
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::BluetoothLEPreferredConnectionParameters;
use windows::Devices::Bluetooth::BluetoothLEPreferredConnectionParametersRequest;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicProperties,
    GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
    GattDeviceService, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattLocalService, GattProtectionLevel, GattServiceProvider,
    GattServiceProviderAdvertisingParameters, GattSession, GattSubscribedClient,
    GattValueChangedEventArgs, GattWriteOption, GattWriteRequestedEventArgs,
};
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
use windows::Win32::System::Com::CoIncrementMTAUsage;

/// How long to wait for the radio thread to bring the adapter up before giving up. Generous enough
/// to cover the adapter-acquisition retries below (a just-restarted radio enumerates slowly).
const POWER_ON_TIMEOUT: Duration = Duration::from_secs(35);
/// Retries for acquiring a ready adapter: WinRT's default adapter is briefly absent right after a
/// Bluetooth toggle/restart or at cold boot, so poll instead of disabling BLE on the first miss.
const ADAPTER_ATTEMPTS: usize = 12;
/// Delay between adapter-acquisition attempts.
const ADAPTER_RETRY_DELAY: Duration = Duration::from_secs(2);
const DIAL_DISCOVERY_ATTEMPTS: usize = 4;
const DIAL_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(400);
/// Per-write GATT-data-floor payload, leaving room for the fragment header under a typical
/// negotiated ATT MTU (matches the macOS backend's conservative 180).
const GATT_FRAGMENT_PAYLOAD: usize = 180;
/// Reassembly ceiling for an inbound frame spread across data-floor fragments.
const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const GATT_FRAGMENT_OVERHEAD: usize = 8;

fn central_fragment_payload(session: &GattSession) -> usize {
    match session.MaxPduSize() {
        Ok(pdu) => {
            let payload = (pdu as usize)
                .saturating_sub(GATT_FRAGMENT_OVERHEAD)
                .clamp(GATT_FRAGMENT_PAYLOAD, BLE_HW_MTU);
            log::debug!("bluetooth: negotiated MaxPduSize={pdu}, data fragment payload={payload}");
            payload
        }
        Err(_) => GATT_FRAGMENT_PAYLOAD,
    }
}

fn request_throughput(
    device: &BluetoothLEDevice,
) -> Option<BluetoothLEPreferredConnectionParametersRequest> {
    let preferred = BluetoothLEPreferredConnectionParameters::ThroughputOptimized().ok()?;
    match device.RequestPreferredConnectionParameters(&preferred) {
        Ok(request) => {
            log::debug!("bluetooth: requested throughput-optimized connection parameters");
            Some(request)
        }
        Err(error) => {
            log::debug!("bluetooth: connection-parameter request rejected ({error:?})");
            None
        }
    }
}

#[derive(Debug)]
pub enum WindowsBleError {
    NoAdapter,
    /// The adapter cannot act as a BLE peripheral (advertise the service), so it cannot host a link.
    PeripheralRoleUnsupported,
    /// The adapter is present but the radio is switched off (airplane mode / hardware toggle).
    RadioOff,
    ServicePublishFailed,
    DialFailed,
    Closed,
    /// The adapter did not come up within [`POWER_ON_TIMEOUT`].
    PowerOnTimeout,
    /// A control PDU exceeded `CONTROL_MAX_LEN`.
    ControlTooLarge,
    /// A data frame exceeded the negotiated link MTU.
    FrameTooLarge,
    WriteFailed,
    Winrt(windows::core::Error),
}

impl From<windows::core::Error> for WindowsBleError {
    fn from(error: windows::core::Error) -> Self {
        WindowsBleError::Winrt(error)
    }
}

/// The published GATT peripheral: the service provider (which advertises) and the two characteristics
/// the link rides. WinRT runtime classes are agile, so these are driven from the async side after the
/// radio thread creates them.
struct Radio {
    provider: GattServiceProvider,
    control: GattLocalCharacteristic,
    data: GattLocalCharacteristic,
    /// The central-role scanner. Held so it keeps running (and keeps its Received handler — which
    /// owns the only live sender into the event channel — alive) for the backend's lifetime.
    watcher: BluetoothLEAdvertisementWatcher,
    /// Total BLE advertisements the radio has delivered (any device, not just Prns). Lets the
    /// heartbeat distinguish "radio is dead / not delivering" from "no Prns peer is advertising."
    adverts: Arc<AtomicU64>,
}

/// Events from the WinRT callback world into the async consumer.
enum Event {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(WinGattLink),
}

type ClientSlot = Arc<Mutex<Option<GattSubscribedClient>>>;

/// One GATT link to a peer. The two planes are symmetric in the seam but inverted on the wire: a
/// `Central` link (we dialled) writes to the peer's characteristics and holds the connection up via
/// the device/service/session; a `Peripheral` link (a peer dialled us) notifies our own local
/// characteristics, targeting the peer's subscribed client. Receive is a channel either way.
enum LinkPlane {
    Central {
        control_char: GattCharacteristic,
        data_char: GattCharacteristic,
        device: BluetoothLEDevice,
        service: GattDeviceService,
        session: GattSession,
        connection_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    },
    Peripheral {
        control_char: GattLocalCharacteristic,
        data_char: GattLocalCharacteristic,
        control_client: ClientSlot,
        data_client: ClientSlot,
    },
}

pub struct WinGattLink {
    address: BleAddress,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    data_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
    /// Goes `true` when the link drops (central: ConnectionStatusChanged; peripheral: the peer's
    /// client leaves SubscribedClients), so the recv paths fail (Closed) and the supervisor retires
    /// the link. Level-triggered (a watch), so a disconnect that fires between reads is never missed.
    closed: watch::Receiver<bool>,
    plane: LinkPlane,
}

impl BleLink for WinGattLink {
    type Error = WindowsBleError;
    type Source = WinGattSource;
    type Sink = WinGattSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), WindowsBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg
            .encode(&mut buf)
            .ok_or(WindowsBleError::ControlTooLarge)?;
        let bytes = buf
            .get(..len)
            .ok_or(WindowsBleError::ControlTooLarge)?
            .to_vec();
        match &self.plane {
            LinkPlane::Central { control_char, .. } => {
                gatt_write(
                    control_char.clone(),
                    bytes,
                    GattWriteOption::WriteWithResponse,
                )
                .await?;
            }
            LinkPlane::Peripheral {
                control_char,
                control_client,
                ..
            } => {
                notify_local(control_char.clone(), control_client.clone(), bytes).await?;
            }
        }
        log::debug!("bluetooth: {:02x?} -> {msg:?}", self.address.octets());
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, WindowsBleError> {
        if *self.closed.borrow() {
            return Err(WindowsBleError::Closed);
        }
        let control = tokio::select! {
            msg = self.control_rx.recv() => msg.ok_or(WindowsBleError::Closed)?,
            _ = self.closed.changed() => return Err(WindowsBleError::Closed),
        };
        log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), WindowsBleError> {
        // GATT-only: WinRT has no app-level L2CAP, so the upgrade is a permanent no-op. The floor
        // carries every frame; never failing keeps the link alive (the seam contract for upgrade).
        Ok(())
    }

    fn into_data(self) -> (WinGattSource, WinGattSink) {
        // Reassemble inbound data-floor fragments into whole frames on a background task, the same
        // shape as the macOS GATT floor.
        let (merged_tx, merged_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
        if let Some(mut inbound_rx) = self.data_rx {
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = inbound_rx.recv().await {
                    let Some(fragment) = Fragment::decode(&message) else {
                        log::warn!(
                            "bluetooth: data fragment decode failed ({} bytes)",
                            message.len()
                        );
                        continue;
                    };
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        log::debug!("bluetooth: reassembled data frame {} bytes", frame.len());
                        if merged_tx.send(Box::from(frame)).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        // Split the plane into the source's keepalive and the sink's send target. A central holds the
        // device/service/session both halves (so the connection — and its ConnectionStatusChanged
        // handler — outlives any source/sink drop order); a peripheral's local service lives in the
        // backend's Radio, so the source needs no keepalive and the sink just notifies our local char.
        let (keepalive, sink_plane, fragment_payload) = match self.plane {
            LinkPlane::Central {
                data_char,
                device,
                service,
                session,
                connection_request,
                ..
            } => {
                let sink_session = session.clone();
                let fragment_payload = central_fragment_payload(&session);
                (
                    SourceKeepalive::Central {
                        _device: device,
                        _service: service,
                        _session: session,
                        _connection_request: connection_request,
                    },
                    SinkPlane::Central {
                        data_char,
                        _session: sink_session,
                    },
                    fragment_payload,
                )
            }
            LinkPlane::Peripheral {
                data_char,
                data_client,
                ..
            } => (
                SourceKeepalive::Peripheral,
                SinkPlane::Peripheral {
                    data_char,
                    data_client,
                },
                GATT_FRAGMENT_PAYLOAD,
            ),
        };
        (
            WinGattSource {
                inbound: merged_rx,
                closed: self.closed,
                _keepalive: keepalive,
            },
            WinGattSink {
                plane: sink_plane,
                address: self.address,
                fragment_payload,
            },
        )
    }
}

/// Keeps a central link's GATT connection up for the source's whole life; a peripheral source needs
/// no hold (the local service lives in the backend's Radio).
enum SourceKeepalive {
    Central {
        _device: BluetoothLEDevice,
        _service: GattDeviceService,
        _session: GattSession,
        _connection_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    },
    Peripheral,
}

/// The send half's wire target: a central writes the peer's data characteristic; a peripheral
/// notifies our local data characteristic, aimed at the peer's subscribed client.
enum SinkPlane {
    Central {
        data_char: GattCharacteristic,
        _session: GattSession,
    },
    Peripheral {
        data_char: GattLocalCharacteristic,
        data_client: ClientSlot,
    },
}

/// The receive half of the data floor: whole frames reassembled from the notifications.
pub struct WinGattSource {
    inbound: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
    /// Shared with the link: goes `true` on disconnect so a dead link fails the read, not hangs.
    closed: watch::Receiver<bool>,
    _keepalive: SourceKeepalive,
}

impl BleSource for WinGattSource {
    type Error = WindowsBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, WindowsBleError> {
        if *self.closed.borrow() {
            return Err(WindowsBleError::Closed);
        }
        let frame = tokio::select! {
            frame = self.inbound.recv() => frame.ok_or(WindowsBleError::Closed)?,
            _ = self.closed.changed() => return Err(WindowsBleError::Closed),
        };
        let len = frame.len().min(out.len());
        let dst = out.get_mut(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        let src = frame.get(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        dst.copy_from_slice(src);
        Ok(len)
    }
}

/// The send half of the data floor: fragments a frame across writes to the peer's data
/// characteristic (write-without-response, the unacknowledged floor).
pub struct WinGattSink {
    plane: SinkPlane,
    address: BleAddress,
    fragment_payload: usize,
}

impl BleSink for WinGattSink {
    type Error = WindowsBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), WindowsBleError> {
        for fragment in fragments_of(frame, self.fragment_payload) {
            let mut buf = vec![0u8; self.fragment_payload + FRAGMENT_SCRATCH];
            let len = fragment
                .encode(&mut buf)
                .ok_or(WindowsBleError::FrameTooLarge)?;
            buf.truncate(len);
            let bytes = buf;
            match &self.plane {
                SinkPlane::Central { data_char, .. } => {
                    gatt_write(
                        data_char.clone(),
                        bytes,
                        GattWriteOption::WriteWithoutResponse,
                    )
                    .await?;
                }
                SinkPlane::Peripheral {
                    data_char,
                    data_client,
                } => {
                    notify_local(data_char.clone(), data_client.clone(), bytes).await?;
                }
            }
        }
        Ok(())
    }
}

/// Headroom over the payload for the fragment's 5-byte header when sizing the encode scratch buffer.
const FRAGMENT_SCRATCH: usize = 8;

/// One inbound (peer-dialled) peer, keyed by its GATT session device id. Write handlers feed its
/// control/data channels; subscription handlers fill its per-characteristic notify targets; a
/// dropped subscription flips its close signal so the supervisor retires the accepted link.
struct InboundPeer {
    control_tx: tokio_mpsc::UnboundedSender<Control>,
    data_tx: tokio_mpsc::UnboundedSender<Box<[u8]>>,
    closed_tx: watch::Sender<bool>,
    control_client: ClientSlot,
    data_client: ClientSlot,
}

type InboundRegistry = Arc<Mutex<HashMap<String, InboundPeer>>>;

enum ClientKind {
    Control,
    Data,
}

/// Notify one of our local characteristics. With a known subscribed client, target it directly so
/// concurrent inbound peers never cross-talk; before the client is observed, fall back to notifying
/// every subscriber (a lone early Welcome). Runs the blocking WinRT `get()` off the reactor.
async fn notify_local(
    characteristic: GattLocalCharacteristic,
    client: ClientSlot,
    bytes: Vec<u8>,
) -> Result<(), WindowsBleError> {
    tokio::task::spawn_blocking(move || -> Result<(), WindowsBleError> {
        let buffer = ibuffer_from(&bytes)?;
        let target = client.lock().ok().and_then(|guard| guard.clone());
        match target {
            Some(client) => {
                characteristic
                    .NotifyValueForSubscribedClientAsync(&buffer, &client)?
                    .get()?;
            }
            None => {
                characteristic.NotifyValueAsync(&buffer)?.get()?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| WindowsBleError::Closed)?
}

/// Wire the local characteristics so a peer dialling us becomes a `BleEvent::Inbound` link (the
/// listener role). Mirrors the macOS CBPeripheralManager delegate: a first control write from a new
/// peer mints the link; later writes feed its channels; subscriptions track the notify target; an
/// unsubscribe retires it. The registry lives as long as the handlers, which the characteristics —
/// held in `Radio` for the backend's life — own.
fn wire_inbound(
    control: &GattLocalCharacteristic,
    data: &GattLocalCharacteristic,
    events_tx: tokio_mpsc::UnboundedSender<Event>,
) -> Result<(), WindowsBleError> {
    let registry: InboundRegistry = Arc::new(Mutex::new(HashMap::new()));

    let control_writes = registry.clone();
    let control_for_link = control.clone();
    let data_for_link = data.clone();
    control.WriteRequested(&TypedEventHandler::new(
        move |_sender: &Option<GattLocalCharacteristic>,
              args: &Option<GattWriteRequestedEventArgs>| {
            handle_control_write(
                args.as_ref(),
                &control_writes,
                &events_tx,
                &control_for_link,
                &data_for_link,
            );
            Ok(())
        },
    ))?;

    let data_writes = registry.clone();
    data.WriteRequested(&TypedEventHandler::new(
        move |_sender: &Option<GattLocalCharacteristic>,
              args: &Option<GattWriteRequestedEventArgs>| {
            handle_data_write(args.as_ref(), &data_writes);
            Ok(())
        },
    ))?;

    let control_subs = registry.clone();
    control.SubscribedClientsChanged(&TypedEventHandler::new(
        move |sender: &Option<GattLocalCharacteristic>, _args: &Option<IInspectable>| {
            if let Some(characteristic) = sender.as_ref() {
                sync_subscribed_clients(characteristic, &control_subs, ClientKind::Control);
            }
            Ok(())
        },
    ))?;

    let data_subs = registry.clone();
    data.SubscribedClientsChanged(&TypedEventHandler::new(
        move |sender: &Option<GattLocalCharacteristic>, _args: &Option<IInspectable>| {
            if let Some(characteristic) = sender.as_ref() {
                sync_subscribed_clients(characteristic, &data_subs, ClientKind::Data);
            }
            Ok(())
        },
    ))?;

    Ok(())
}

fn handle_control_write(
    args: Option<&GattWriteRequestedEventArgs>,
    registry: &InboundRegistry,
    events_tx: &tokio_mpsc::UnboundedSender<Event>,
    control_char: &GattLocalCharacteristic,
    data_char: &GattLocalCharacteristic,
) {
    let Some(args) = args else { return };
    if let Err(error) = process_control_write(args, registry, events_tx, control_char, data_char) {
        log::warn!("bluetooth: inbound control write failed ({error:?})");
    }
}

fn process_control_write(
    args: &GattWriteRequestedEventArgs,
    registry: &InboundRegistry,
    events_tx: &tokio_mpsc::UnboundedSender<Event>,
    control_char: &GattLocalCharacteristic,
    data_char: &GattLocalCharacteristic,
) -> Result<(), WindowsBleError> {
    let deferral = args.GetDeferral()?;
    let outcome = (|| -> Result<(), WindowsBleError> {
        let device_id = args.Session()?.DeviceId()?.Id()?.to_string();
        let request = args.GetRequestAsync()?.get()?;
        let bytes = bytes_from(&request.Value()?)?;
        let control_tx =
            ensure_inbound_peer(registry, events_tx, control_char, data_char, &device_id)?;
        if let Some(control) = Control::decode(&bytes) {
            let _ = control_tx.send(control);
        }
        if request.Option()? == GattWriteOption::WriteWithResponse {
            request.Respond()?;
        }
        Ok(())
    })();
    deferral.Complete()?;
    outcome
}

fn ensure_inbound_peer(
    registry: &InboundRegistry,
    events_tx: &tokio_mpsc::UnboundedSender<Event>,
    control_char: &GattLocalCharacteristic,
    data_char: &GattLocalCharacteristic,
    device_id: &str,
) -> Result<tokio_mpsc::UnboundedSender<Control>, WindowsBleError> {
    let mut map = registry.lock().map_err(|_| WindowsBleError::Closed)?;
    if let Some(peer) = map.get(device_id) {
        return Ok(peer.control_tx.clone());
    }
    let (control_tx, control_rx) = tokio_mpsc::unbounded_channel::<Control>();
    let (data_tx, data_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
    let (closed_tx, closed_rx) = watch::channel(false);
    let control_client: ClientSlot = Arc::new(Mutex::new(None));
    let data_client: ClientSlot = Arc::new(Mutex::new(None));
    set_slot_from_subscribers(control_char, device_id, &control_client);
    set_slot_from_subscribers(data_char, device_id, &data_client);
    let address = BleAddress::new(address_standin(device_id));
    let link = WinGattLink {
        address,
        control_rx,
        data_rx: Some(data_rx),
        closed: closed_rx,
        plane: LinkPlane::Peripheral {
            control_char: control_char.clone(),
            data_char: data_char.clone(),
            control_client: control_client.clone(),
            data_client: data_client.clone(),
        },
    };
    map.insert(
        device_id.to_string(),
        InboundPeer {
            control_tx: control_tx.clone(),
            data_tx,
            closed_tx,
            control_client,
            data_client,
        },
    );
    log::info!(
        "bluetooth: inbound peer {:02x?} connected (accepted role)",
        address.octets()
    );
    let _ = events_tx.send(Event::Inbound(link));
    Ok(control_tx)
}

fn handle_data_write(args: Option<&GattWriteRequestedEventArgs>, registry: &InboundRegistry) {
    let Some(args) = args else { return };
    if let Err(error) = process_data_write(args, registry) {
        log::warn!("bluetooth: inbound data write failed ({error:?})");
    }
}

fn process_data_write(
    args: &GattWriteRequestedEventArgs,
    registry: &InboundRegistry,
) -> Result<(), WindowsBleError> {
    let deferral = args.GetDeferral()?;
    let outcome = (|| -> Result<(), WindowsBleError> {
        let device_id = args.Session()?.DeviceId()?.Id()?.to_string();
        let request = args.GetRequestAsync()?.get()?;
        let bytes = bytes_from(&request.Value()?)?;
        if let Ok(map) = registry.lock() {
            if let Some(peer) = map.get(&device_id) {
                let _ = peer.data_tx.send(bytes.into_boxed_slice());
            }
        }
        if request.Option()? == GattWriteOption::WriteWithResponse {
            request.Respond()?;
        }
        Ok(())
    })();
    deferral.Complete()?;
    outcome
}

fn sync_subscribed_clients(
    characteristic: &GattLocalCharacteristic,
    registry: &InboundRegistry,
    kind: ClientKind,
) {
    if let Err(error) = sync_clients(characteristic, registry, kind) {
        log::warn!("bluetooth: inbound subscription sync failed ({error:?})");
    }
}

fn sync_clients(
    characteristic: &GattLocalCharacteristic,
    registry: &InboundRegistry,
    kind: ClientKind,
) -> Result<(), WindowsBleError> {
    let mut current: HashMap<String, GattSubscribedClient> = HashMap::new();
    for client in characteristic.SubscribedClients()? {
        let id = client.Session()?.DeviceId()?.Id()?.to_string();
        current.insert(id, client);
    }
    let mut map = registry.lock().map_err(|_| WindowsBleError::Closed)?;
    let mut dropped = std::vec::Vec::new();
    for (device_id, peer) in map.iter() {
        let slot = match kind {
            ClientKind::Control => &peer.control_client,
            ClientKind::Data => &peer.data_client,
        };
        let now = current.get(device_id).cloned();
        if let Ok(mut guard) = slot.lock() {
            let was_subscribed = guard.is_some();
            if matches!(kind, ClientKind::Control) && was_subscribed && now.is_none() {
                let _ = peer.closed_tx.send(true);
                dropped.push(device_id.clone());
            }
            *guard = now;
        }
    }
    for device_id in dropped {
        map.remove(&device_id);
    }
    Ok(())
}

fn set_slot_from_subscribers(
    characteristic: &GattLocalCharacteristic,
    device_id: &str,
    slot: &ClientSlot,
) {
    let Ok(clients) = characteristic.SubscribedClients() else {
        return;
    };
    for client in clients {
        let matches = client
            .Session()
            .ok()
            .and_then(|session| session.DeviceId().ok())
            .and_then(|id| id.Id().ok())
            .is_some_and(|id| id == device_id);
        if matches {
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(client);
            }
            return;
        }
    }
}

/// A stable 6-octet stand-in address for an inbound peer, hashed from its GATT device id (WinRT gives
/// no MAC on the peripheral side). Mirrors the macOS backend's CBCentral-UUID stand-in: the
/// supervisor dedups settled peers by their handshake identity, so the address only needs to be
/// stable per connection, which the device id is.
fn address_standin(device_id: &str) -> [u8; 6] {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let bytes = hash.to_be_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
}

/// The Windows native-BLE backend handed to the engine's `BluetoothAuto` supervisor.
pub struct WindowsBleBackend {
    /// Dropping this signals the radio thread to unpark and tear the WinRT objects down.
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    radio: Radio,
    /// In-flight central dials; each resolves to the formed link (or `None` on failure).
    dials: JoinSet<Result<WinGattLink, BleAddress>>,
}

impl WindowsBleBackend {
    /// Bring the WinRT adapter up on a dedicated MTA thread, publish the GATT service + start the
    /// scanner, and return once ready. Fails (so the node runs without BLE) if there is no adapter,
    /// the radio is off, the peripheral role is unsupported, or the service cannot be published.
    pub async fn new() -> Result<Self, WindowsBleError> {
        // The watcher's Received handler (built in winrt_setup) owns the sending half; the channel
        // stays open as long as the watcher lives in `Radio`.
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<Radio, WindowsBleError>>();

        std::thread::Builder::new()
            .name("prns-ble-winrt".into())
            .spawn(move || {
                let _ = ready_tx.send(winrt_setup(events_tx));
                // Park so the MTA stays joined for this process's BLE lifetime.
                let _ = shutdown_rx.recv();
            })
            .map_err(|_| WindowsBleError::Closed)?;

        match tokio::time::timeout(POWER_ON_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(radio))) => {
                spawn_watcher_heartbeat(radio.watcher.clone(), radio.adverts.clone());
                Ok(Self {
                    _keepalive: keepalive,
                    events: events_rx,
                    radio,
                    dials: JoinSet::new(),
                })
            }
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(WindowsBleError::Closed),
            Err(_) => Err(WindowsBleError::PowerOnTimeout),
        }
    }
}

/// Map a core [`BleUuid`] to a WinRT [`GUID`]. The core stores 128-bit UUIDs big-endian (RFC-4122
/// display order), and `from_u128` reads a big-endian value, so `from_be_bytes` lines them up. A
/// 16-bit UUID expands through the Bluetooth base UUID `0000xxxx-0000-1000-8000-00805F9B34FB`.
fn guid_of(uuid: BleUuid) -> GUID {
    let bytes = match uuid {
        BleUuid::Bit128(bytes) => bytes,
        BleUuid::Bit16(short) => [
            0x00,
            0x00,
            (short >> 8) as u8,
            short as u8,
            0x00,
            0x00,
            0x10,
            0x00,
            0x80,
            0x00,
            0x00,
            0x80,
            0x5f,
            0x9b,
            0x34,
            0xfb,
        ],
    };
    GUID::from_u128(u128::from_be_bytes(bytes))
}

/// The 48-bit BLE address WinRT works in `u64`s; the sighting kept the low six bytes big-endian, so
/// rebuild the same `u64` to reconnect.
fn address_to_u64(address: BleAddress) -> u64 {
    let o = address.octets();
    u64::from_be_bytes([0, 0, o[0], o[1], o[2], o[3], o[4], o[5]])
}

fn ibuffer_from(bytes: &[u8]) -> Result<IBuffer, WindowsBleError> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(bytes)?;
    Ok(writer.DetachBuffer()?)
}

fn bytes_from(buffer: &IBuffer) -> Result<Vec<u8>, WindowsBleError> {
    let len = buffer.Length()?;
    let reader = DataReader::FromBuffer(buffer)?;
    let mut bytes = std::vec![0u8; len as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// Perform a GATT characteristic write off the async executor. WinRT's `IAsyncOperation` has no
/// `IntoFuture` in this `windows` version, so the blocking `get()` runs on a `spawn_blocking` thread
/// (which joins the process MTA), keeping the reactor unblocked while the write completes.
async fn gatt_write(
    characteristic: GattCharacteristic,
    bytes: Vec<u8>,
    option: GattWriteOption,
) -> Result<(), WindowsBleError> {
    let status = tokio::task::spawn_blocking(
        move || -> Result<GattCommunicationStatus, WindowsBleError> {
            let buffer = ibuffer_from(&bytes)?;
            Ok(characteristic
                .WriteValueWithOptionAsync(&buffer, option)?
                .get()?)
        },
    )
    .await
    .map_err(|_| WindowsBleError::Closed)??;
    if status != GattCommunicationStatus::Success {
        return Err(WindowsBleError::WriteFailed);
    }
    Ok(())
}

/// Build the central scanner: a Received handler that forwards every Prns advertisement as a
/// `Sighting` (the supervisor does identity-keyed dedup, so the backend forwards raw), filtered at
/// the OS level to our service UUID so unrelated BLE traffic never wakes the handler.
fn build_watcher(
    events_tx: tokio_mpsc::UnboundedSender<Event>,
    adverts: Arc<AtomicU64>,
) -> Result<BluetoothLEAdvertisementWatcher, WindowsBleError> {
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    // Active scanning pulls the scan response too and yields RSSI per sighting.
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

    // No OS-level service-UUID filter: it only matches the *primary* advertisement, so a peer that
    // carries the 128-bit UUID in its scan response slips past it (a likely cause of missed
    // sightings). Instead, count every advert (radio-liveness signal) and match the service UUID in
    // the handler. Compute the target GUID once here, not per packet (this fires for every BLE
    // advertisement in range).
    let target = guid_of(BLE_SERVICE_UUID);
    watcher.Received(&TypedEventHandler::new(
        move |_sender, args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
            if let Some(args) = args.as_ref() {
                adverts.fetch_add(1, Ordering::Relaxed);
                if let Some(sighting) = sighting_from(args, target) {
                    // The consumer may be gone (node shutting down); a closed channel is benign.
                    let _ = events_tx.send(sighting);
                }
            }
            Ok(())
        },
    ))?;

    // The OS can Stop/Abort the watcher on a radio hiccup; without this it dies silently and the node
    // looks dormant forever. Log the cause and restart it. (The handler holds a clone of the watcher,
    // a deliberate cycle — the watcher is process-lifetime anyway.)
    let restart = watcher.clone();
    watcher.Stopped(&TypedEventHandler::new(
        move |_sender: &Option<BluetoothLEAdvertisementWatcher>,
              args: &Option<BluetoothLEAdvertisementWatcherStoppedEventArgs>| {
            let error = args.as_ref().and_then(|args| args.Error().ok());
            log::warn!("bluetooth: advertisement watcher stopped (error {error:?}) — restarting");
            if let Err(err) = restart.Start() {
                log::error!("bluetooth: watcher restart failed ({err:?})");
            }
            Ok(())
        },
    ))?;
    Ok(watcher)
}

/// Heartbeat the scanner so its health is visible, and watchdog it: a `Started` watcher that has
/// delivered no new advertisements across the stall window is almost certainly soft-wedged (the OS
/// scan stopped feeding us despite reporting running), so kick it with `Stop()` — the `Stopped`
/// handler re-`Start()`s it. A genuinely quiet RF environment restarts harmlessly. A deeper HCI wedge
/// survives this (needs a radio reset), but the heartbeat makes that case obvious in the log.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// Consecutive heartbeats with zero new adverts before the watchdog kicks the scanner (~45s).
const SCAN_STALL_TICKS: u32 = 3;

fn spawn_watcher_heartbeat(watcher: BluetoothLEAdvertisementWatcher, adverts: Arc<AtomicU64>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut last_seen = adverts.load(Ordering::Relaxed);
        let mut quiet_ticks = 0u32;
        loop {
            tick.tick().await;
            let seen = adverts.load(Ordering::Relaxed);
            let status = match watcher.Status() {
                Ok(status) => status,
                Err(error) => {
                    log::warn!("bluetooth: scanner status unreadable ({error:?})");
                    break;
                }
            };
            log::info!("bluetooth: scanner status {status:?}, {seen} adverts seen so far");

            let started = status == BluetoothLEAdvertisementWatcherStatus::Started;
            quiet_ticks = if started && seen == last_seen {
                quiet_ticks + 1
            } else {
                0
            };
            last_seen = seen;

            if quiet_ticks >= SCAN_STALL_TICKS {
                log::warn!(
                    "bluetooth: scanner delivered no adverts for ~{}s while Started — kicking it",
                    SCAN_STALL_TICKS * HEARTBEAT_INTERVAL.as_secs() as u32
                );
                // Stop() drives the Stopped handler, which restarts the watcher cleanly (calling
                // Start() here directly would race the Stopping->Stopped transition).
                if let Err(error) = watcher.Stop() {
                    log::error!("bluetooth: scanner kick (Stop) failed ({error:?})");
                }
                quiet_ticks = 0;
            }
        }
    });
}

fn sighting_from(args: &BluetoothLEAdvertisementReceivedEventArgs, target: GUID) -> Option<Event> {
    // Active scanning delivers a peer's primary advertisement and its scan response as separate
    // Received events; we check the service UUID on each (rather than via the OS filter, which only
    // matches the primary packet), so a UUID carried only in the scan response is still caught.
    let advertised = args
        .Advertisement()
        .ok()?
        .ServiceUuids()
        .ok()?
        .into_iter()
        .any(|uuid| uuid == target);
    if !advertised {
        return None;
    }
    let address = args.BluetoothAddress().ok()?;
    let bytes = address.to_be_bytes();
    let octets = [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]];
    let rssi = args
        .RawSignalStrengthInDBm()
        .ok()
        .and_then(|dbm| i8::try_from(dbm).ok());
    Some(Event::Sighting {
        address: BleAddress::new(octets),
        rssi,
    })
}

/// Run on the radio thread: join the MTA, verify the adapter is a powered-on BLE peripheral, publish
/// the GATT service (control + data characteristics, write + notify, plain security), and start the
/// central scanner.
fn winrt_setup(events_tx: tokio_mpsc::UnboundedSender<Event>) -> Result<Radio, WindowsBleError> {
    // WinRT factory activation and async completion need an initialized apartment. CoIncrementMTAUsage
    // joins (or starts) the process-wide implicit MTA and keeps it alive for this thread's lifetime
    // with no matching decrement — the right shape for a long-lived radio thread.
    // SAFETY: a plain COM call with no preconditions; the returned cookie only matters if we wanted
    // to later decrement MTA usage, which a lifelong radio thread never does.
    unsafe {
        CoIncrementMTAUsage()?;
    }

    acquire_adapter()?;

    let service_result = GattServiceProvider::CreateAsync(guid_of(BLE_SERVICE_UUID))?.get()?;
    if service_result.Error()? != BluetoothError::Success {
        return Err(WindowsBleError::ServicePublishFailed);
    }
    let provider = service_result.ServiceProvider()?;
    let service = provider.Service()?;

    // Control + data both write (with and without response) and notify; security stays plain so a
    // central never triggers a bond/pairing prompt (the macOS learning, mirrored here).
    let properties = GattCharacteristicProperties::Write
        | GattCharacteristicProperties::WriteWithoutResponse
        | GattCharacteristicProperties::Notify;
    let control = publish_characteristic(&service, guid_of(NATIVE_CONTROL_UUID), properties)?;
    let data = publish_characteristic(&service, guid_of(NATIVE_DATA_UUID), properties)?;
    wire_inbound(&control, &data, events_tx.clone())?;

    let adverts = Arc::new(AtomicU64::new(0));
    let watcher = build_watcher(events_tx, adverts.clone())?;
    watcher.Start()?;

    log::info!(
        "bluetooth: WinRT adapter powered on; GATT service published, scanning for Prns peers"
    );
    Ok(Radio {
        provider,
        control,
        data,
        watcher,
        adverts,
    })
}

/// Poll for a powered-on, BLE-peripheral-capable adapter. WinRT's default adapter is transiently
/// absent right after a Bluetooth toggle/restart or at cold boot (`GetDefaultAsync` yields a null
/// adapter, surfaced as an odd `HRESULT(0)` error), so retry instead of disabling BLE on the first
/// miss. A genuinely absent/off radio exhausts the retries and returns the precise reason.
fn acquire_adapter() -> Result<(), WindowsBleError> {
    let mut last = WindowsBleError::NoAdapter;
    for attempt in 1..=ADAPTER_ATTEMPTS {
        match try_adapter() {
            Ok(()) => return Ok(()),
            Err(error) => {
                log::warn!(
                    "bluetooth: adapter not ready (attempt {attempt}/{ADAPTER_ATTEMPTS}): {error:?}"
                );
                last = error;
                if attempt < ADAPTER_ATTEMPTS {
                    std::thread::sleep(ADAPTER_RETRY_DELAY);
                }
            }
        }
    }
    Err(last)
}

/// One adapter probe: present, BLE + peripheral capable, radio on.
fn try_adapter() -> Result<(), WindowsBleError> {
    let adapter: BluetoothAdapter = BluetoothAdapter::GetDefaultAsync()?.get()?;
    if !adapter.IsLowEnergySupported()? || !adapter.IsPeripheralRoleSupported()? {
        return Err(WindowsBleError::PeripheralRoleUnsupported);
    }
    let radio = adapter.GetRadioAsync()?.get()?;
    if radio.State()? != RadioState::On {
        return Err(WindowsBleError::RadioOff);
    }
    Ok(())
}

/// Create one local characteristic on the service with the given UUID and properties, plain security.
fn publish_characteristic(
    service: &GattLocalService,
    uuid: GUID,
    properties: GattCharacteristicProperties,
) -> Result<GattLocalCharacteristic, WindowsBleError> {
    let parameters = GattLocalCharacteristicParameters::new()?;
    parameters.SetCharacteristicProperties(properties)?;
    parameters.SetReadProtectionLevel(GattProtectionLevel::Plain)?;
    parameters.SetWriteProtectionLevel(GattProtectionLevel::Plain)?;
    let result = service
        .CreateCharacteristicAsync(uuid, &parameters)?
        .get()?;
    if result.Error()? != BluetoothError::Success {
        return Err(WindowsBleError::ServicePublishFailed);
    }
    Ok(result.Characteristic()?)
}

/// Connect to a sighted peer as GATT client: discover our service's control + data characteristics,
/// subscribe to their notifications, and assemble the central link. Each notification is decoded
/// (control) or forwarded raw (data) into the link's receivers. Runs on a `spawn_blocking` thread
/// (joined to the MTA) because the WinRT GATT calls are blocking `get()`s in this `windows` version.
fn connect_blocking(address: BleAddress) -> Result<WinGattLink, WindowsBleError> {
    let device = BluetoothLEDevice::FromBluetoothAddressAsync(address_to_u64(address))?.get()?;

    // Pin the connection up. WinRT otherwise drops an idle GATT client link shortly after discovery,
    // which is the "connected then dormant" flakiness; MaintainConnection holds it for the session's
    // (== link's) lifetime.
    let session = GattSession::FromDeviceIdAsync(&device.BluetoothDeviceId()?)?.get()?;
    session.SetMaintainConnection(true)?;
    let connection_request = request_throughput(&device);

    // Detect drops: when the device disconnects, flip `closed` so the recv paths fail and the
    // supervisor retires + re-dials, instead of the link hanging as a zombie forever.
    let (closed_tx, closed_rx) = watch::channel(false);
    device.ConnectionStatusChanged(&TypedEventHandler::new(
        move |sender: &Option<BluetoothLEDevice>, _args: &Option<windows::core::IInspectable>| {
            let disconnected = sender
                .as_ref()
                .and_then(|device| device.ConnectionStatus().ok())
                .map(|status| status == BluetoothConnectionStatus::Disconnected)
                .unwrap_or(true);
            if disconnected {
                log::info!(
                    "bluetooth: {:02x?} disconnected — dropping link",
                    address.octets()
                );
                let _ = closed_tx.send(true);
            }
            Ok(())
        },
    ))?;

    let (control_char, data_char) = {
        let mut attempt = 1;
        loop {
            let discovered =
                discover_characteristic(&device, NATIVE_CONTROL_UUID).and_then(|control| {
                    Ok((control, discover_characteristic(&device, NATIVE_DATA_UUID)?))
                });
            match discovered {
                Ok(pair) => break pair,
                Err(error) if attempt < DIAL_DISCOVERY_ATTEMPTS => {
                    log::debug!(
                        "bluetooth: discovery attempt {attempt}/{DIAL_DISCOVERY_ATTEMPTS} for {:02x?} failed ({error:?}); retrying",
                        address.octets()
                    );
                    attempt += 1;
                    std::thread::sleep(DIAL_DISCOVERY_RETRY_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
    };
    // Both characteristics live on the same service; hold it (and the device) to keep the link up.
    let service = control_char.Service()?;

    let (control_tx, control_rx) = tokio_mpsc::unbounded_channel::<Control>();
    subscribe(&control_char, "control", move |bytes| {
        if let Some(control) = Control::decode(&bytes) {
            let _ = control_tx.send(control);
        }
    })?;

    let (data_tx, data_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
    subscribe(&data_char, "data", move |bytes| {
        let _ = data_tx.send(Box::from(bytes.as_slice()));
    })?;

    log::debug!(
        "bluetooth: dialled {:02x?} — control + data characteristics subscribed",
        address.octets()
    );
    Ok(WinGattLink {
        address,
        control_rx,
        data_rx: Some(data_rx),
        closed: closed_rx,
        plane: LinkPlane::Central {
            control_char,
            data_char,
            device,
            service,
            session,
            connection_request,
        },
    })
}

/// Discover the first characteristic with `uuid` under our service on the connected device.
fn discover_characteristic(
    device: &BluetoothLEDevice,
    uuid: BleUuid,
) -> Result<GattCharacteristic, WindowsBleError> {
    // Uncached forces a fresh GATT discovery instead of trusting the OS cache, which on Windows can
    // return Success with a stale/empty service list right after connecting — the usual cause of a
    // first dial failing before a retry succeeds.
    let connection = device.ConnectionStatus().ok();
    let services = device
        .GetGattServicesForUuidWithCacheModeAsync(
            guid_of(BLE_SERVICE_UUID),
            BluetoothCacheMode::Uncached,
        )?
        .get()?;
    let service_status = services.Status()?;
    if service_status != GattCommunicationStatus::Success {
        log::warn!(
            "bluetooth: service discovery failed (connection={connection:?}, status={service_status:?})"
        );
        return Err(WindowsBleError::DialFailed);
    }
    let service = match services.Services()?.into_iter().next() {
        Some(service) => service,
        None => {
            log::warn!("bluetooth: service discovery succeeded but the Prns service was absent");
            return Err(WindowsBleError::DialFailed);
        }
    };
    let chars = service
        .GetCharacteristicsForUuidWithCacheModeAsync(guid_of(uuid), BluetoothCacheMode::Uncached)?
        .get()?;
    let char_status = chars.Status()?;
    if char_status != GattCommunicationStatus::Success {
        log::warn!("bluetooth: characteristic discovery failed (status={char_status:?})");
        return Err(WindowsBleError::DialFailed);
    }
    match chars.Characteristics()?.into_iter().next() {
        Some(characteristic) => Ok(characteristic),
        None => {
            log::warn!(
                "bluetooth: characteristic discovery succeeded but the characteristic was absent"
            );
            Err(WindowsBleError::DialFailed)
        }
    }
}

/// Subscribe to a characteristic's notifications, routing each value to `on_value`, then enable the
/// CCCD so the peer actually notifies.
fn subscribe<F>(
    characteristic: &GattCharacteristic,
    label: &'static str,
    on_value: F,
) -> Result<(), WindowsBleError>
where
    F: Fn(Vec<u8>) + Send + 'static,
{
    characteristic.ValueChanged(&TypedEventHandler::new(
        move |_sender, args: &Option<GattValueChangedEventArgs>| {
            if let Some(args) = args.as_ref() {
                if let Ok(buffer) = args.CharacteristicValue() {
                    if let Ok(bytes) = bytes_from(&buffer) {
                        log::debug!("bluetooth: notify in {label} {} bytes", bytes.len());
                        on_value(bytes);
                    }
                }
            }
            Ok(())
        },
    ))?;
    let status = characteristic
        .WriteClientCharacteristicConfigurationDescriptorAsync(
            GattClientCharacteristicConfigurationDescriptorValue::Notify,
        )?
        .get()?;
    if status != GattCommunicationStatus::Success {
        return Err(WindowsBleError::DialFailed);
    }
    Ok(())
}

impl Drop for WindowsBleBackend {
    fn drop(&mut self) {
        // Best-effort cleanup on a clean exit: stop advertising so the OS is not left holding our
        // peripheral advert intent. We deliberately do NOT Stop() the watcher here — its Stopped
        // handler would just restart it — and the scan dies with the process anyway. A hard process
        // kill reclaims everything regardless; this only tidies the graceful path.
        let _ = self.radio.provider.StopAdvertising();
    }
}

impl BleBackend for WindowsBleBackend {
    const MAX_PEERS: usize = limits::WINDOWS_MAX_PEERS;
    type Error = WindowsBleError;
    type Link = WinGattLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), WindowsBleError> {
        if enabled {
            // Connectable + discoverable: WinRT folds the service's 128-bit UUID into the
            // advertisement automatically when discoverable, so we do not hand-roll the AD bytes.
            let parameters = GattServiceProviderAdvertisingParameters::new()?;
            parameters.SetIsConnectable(true)?;
            parameters.SetIsDiscoverable(true)?;
            self.radio
                .provider
                .StartAdvertisingWithParameters(&parameters)?;
            log::info!("bluetooth: advertising the Prns service (connectable + discoverable)");
        } else {
            self.radio.provider.StopAdvertising()?;
            log::info!("bluetooth: stopped advertising");
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<WinGattLink> {
        loop {
            let pending_dials = !self.dials.is_empty();
            tokio::select! {
                event = self.events.recv() => match event {
                    Some(Event::Sighting { address, rssi }) => {
                        log::debug!(
                            "bluetooth: sighted Prns peer {:02x?} rssi={rssi:?}",
                            address.octets()
                        );
                        return BleEvent::Sighting { address, rssi };
                    }
                    Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                    // Watcher gone (backend shutting down): nothing more arrives, so park.
                    None => core::future::pending().await,
                },
                Some(joined) = self.dials.join_next(), if pending_dials => {
                    match joined {
                        Ok(Ok(link)) => {
                            return BleEvent::LinkReady {
                                link,
                                origin: Origin::Dialed,
                                peer_rssi: None,
                            };
                        }
                        Ok(Err(address)) => return BleEvent::DialFailed { address },
                        Err(_) => {}
                    }
                }
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        log::debug!(
            "bluetooth: dialling {:02x?} over LE (central role)",
            address.octets()
        );
        // The WinRT GATT connect/discover/subscribe are blocking get()s, so run them off the reactor.
        self.dials
            .spawn_blocking(move || match connect_blocking(address) {
                Ok(link) => Ok(link),
                Err(error) => {
                    log::warn!(
                        "bluetooth: dial to {:02x?} failed ({error:?})",
                        address.octets()
                    );
                    Err(address)
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::bluetooth_auto::core::FRAGMENT_HEADER_LEN;

    /// The core stores the service UUID big-endian; `guid_of` must yield the canonical WinRT GUID.
    /// Built field-wise so the assertion is independent of `guid_of`'s own conversion path (a
    /// little-endian slip would change the value and fail here).
    #[test]
    fn service_uuid_maps_to_canonical_guid() {
        let expected = GUID::from_values(
            0x37145b00,
            0x442d,
            0x4a94,
            [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe3],
        );
        assert_eq!(guid_of(BLE_SERVICE_UUID), expected);
    }

    /// The control/data characteristics differ from the service only in the last byte (e7 / e8).
    #[test]
    fn control_and_data_uuids_match_the_spec() {
        let control = GUID::from_values(
            0x37145b00,
            0x442d,
            0x4a94,
            [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe7],
        );
        let data = GUID::from_values(
            0x37145b00,
            0x442d,
            0x4a94,
            [0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28, 0xe8],
        );
        assert_eq!(guid_of(NATIVE_CONTROL_UUID), control);
        assert_eq!(guid_of(NATIVE_DATA_UUID), data);
    }

    /// A 16-bit UUID expands through the Bluetooth base UUID `0000xxxx-0000-1000-8000-00805F9B34FB`.
    #[test]
    fn bit16_uuid_expands_through_the_bluetooth_base() {
        let expected = GUID::from_values(
            0x0000_180f, // battery service, as an example short UUID
            0x0000,
            0x1000,
            [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        );
        assert_eq!(guid_of(BleUuid::Bit16(0x180f)), expected);
    }

    /// A sighting keeps the low six bytes of WinRT's 48-bit `u64` address (big-endian); `dial` must
    /// rebuild the identical `u64` via `address_to_u64`, or it would reconnect to the wrong device.
    #[test]
    fn winrt_address_round_trips_through_sighting_octets() {
        for winrt_addr in [
            0x0000_5998_43cb_137c_u64,
            0x0000_ffff_ffff_ffff,
            0x0000_0000_0000_0001,
            0x0000_1234_5678_9abc,
        ] {
            let bytes = winrt_addr.to_be_bytes();
            // Mirrors the octet extraction in `sighting_from`.
            let octets = [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]];
            assert_eq!(address_to_u64(BleAddress::new(octets)), winrt_addr);
        }
    }

    /// The send-path encode buffer (`GATT_FRAGMENT_PAYLOAD + FRAGMENT_SCRATCH`) must hold a fragment
    /// carrying a full payload plus its header — otherwise `send_frame` would error on large frames.
    #[test]
    fn scratch_buffer_holds_a_max_payload_fragment() {
        const { assert!(FRAGMENT_SCRATCH >= FRAGMENT_HEADER_LEN) };
        let payload = [0xAB_u8; GATT_FRAGMENT_PAYLOAD * 3];
        let mut buf = [0u8; GATT_FRAGMENT_PAYLOAD + FRAGMENT_SCRATCH];
        let mut fragments = 0;
        for fragment in fragments_of(&payload, GATT_FRAGMENT_PAYLOAD) {
            let len = fragment
                .encode(&mut buf)
                .expect("a full-payload fragment fits the scratch buffer");
            assert!(len <= buf.len());
            fragments += 1;
        }
        assert!(fragments >= 3);
    }
}
