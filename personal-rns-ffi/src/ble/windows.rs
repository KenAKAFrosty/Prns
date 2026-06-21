//! The Windows (WinRT) backend for the native Reticulum-over-BLE auto-interface.
//!
//! GATT-only by mandate: WinRT exposes no app-level L2CAP, so [`arrangement`] pins every Windows
//! pair to `GattOnly` and `upgrade()` is a permanent no-op. The GATT-data floor carries every
//! frame; this is a complete, correct backend, not a degraded one. The shared brain (discovery
//! dedup, orientation, make-before-break, and the Hello/Welcome handshake) lives in the engine's
//! `BluetoothAuto` supervisor — this backend only drives the radio and the seam, mirroring
//! `personal-rns-ffi/src/ble/macos.rs`.
//!
//! WinRT activation and async completion need an initialized COM apartment, so a dedicated thread
//! joins the process MTA, brings the adapter up, publishes the GATT service, and parks for the
//! backend's lifetime. WinRT event handlers (which fire on the system threadpool) post into tokio
//! channels the async consumer drains — the callback-world-to-reactor bridge the macOS backend uses
//! with GCD. The WinRT runtime classes are agile, so the published service + the dialled GATT client
//! objects are driven from the async side.
//!
//! Implemented: power-up, advertise (peripheral role), scan (central role), and the **central** link
//! — when the supervisor dials a sighted peer we connect, discover the control + data
//! characteristics, subscribe to their notifications, carry the control handshake, and ride the data
//! floor. The peripheral-side inbound link (a peer dialling us) is the remaining role.
#![allow(dead_code)] // TODO(ble-windows): peripheral/inbound role still to land.

use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::core::{
    fragments_of, BleAddress, BleUuid, Control, Dialect, Fragment, L2capPlan, Reassembler,
    BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;

use windows::core::GUID;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementFilter, BluetoothLEAdvertisementReceivedEventArgs,
    BluetoothLEAdvertisementWatcher, BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Bluetooth::BluetoothError;
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicProperties,
    GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
    GattDeviceService, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattLocalService, GattProtectionLevel, GattServiceProvider,
    GattServiceProviderAdvertisingParameters, GattValueChangedEventArgs, GattWriteOption,
};
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
use windows::Win32::System::Com::CoIncrementMTAUsage;

/// How long to wait for the radio thread to bring the adapter up before giving up.
const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-write GATT-data-floor payload, leaving room for the fragment header under a typical
/// negotiated ATT MTU (matches the macOS backend's conservative 180).
const GATT_FRAGMENT_PAYLOAD: usize = 180;
/// Reassembly ceiling for an inbound frame spread across data-floor fragments.
const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;

#[derive(Debug)]
pub enum WindowsBleError {
    /// No Bluetooth adapter is present on this machine.
    NoAdapter,
    /// The adapter cannot act as a BLE peripheral (advertise the service), so it cannot host a link.
    PeripheralRoleUnsupported,
    /// The adapter is present but the radio is switched off (airplane mode / hardware toggle).
    RadioOff,
    /// Publishing the GATT service or one of its characteristics failed.
    ServicePublishFailed,
    /// A dial could not connect, discover the service, or subscribe to a characteristic.
    DialFailed,
    /// The radio thread or a channel went away.
    Closed,
    /// The adapter did not come up within [`POWER_ON_TIMEOUT`].
    PowerOnTimeout,
    /// A control PDU exceeded `CONTROL_MAX_LEN`.
    ControlTooLarge,
    /// A data frame exceeded the negotiated link MTU.
    FrameTooLarge,
    /// A GATT write completed with a non-success status.
    WriteFailed,
    /// An underlying WinRT call failed.
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
}

/// Events from the WinRT callback world into the async consumer.
enum Event {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(WinGattLink),
}

/// One GATT link to a peer — the control connection plus the GATT-data floor (the only data plane on
/// Windows). This is the central (we-dialled) variant: it holds the peer's control + data
/// characteristics to write to, and the receivers fed by their notification handlers. The device and
/// service are held only to keep the GATT connection alive.
pub struct WinGattLink {
    address: BleAddress,
    control_char: GattCharacteristic,
    data_char: GattCharacteristic,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    data_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
    _device: BluetoothLEDevice,
    _service: GattDeviceService,
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
        gatt_write(
            self.control_char.clone(),
            bytes,
            GattWriteOption::WriteWithResponse,
        )
        .await?;
        log::debug!("bluetooth: {:02x?} -> {msg:?}", self.address.octets());
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, WindowsBleError> {
        let control = self
            .control_rx
            .recv()
            .await
            .ok_or(WindowsBleError::Closed)?;
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
                        continue;
                    };
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        if merged_tx.send(Box::from(frame)).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        (
            WinGattSource { inbound: merged_rx },
            WinGattSink {
                data_char: self.data_char,
                address: self.address,
            },
        )
    }
}

/// The receive half of the data floor: whole frames reassembled from the data characteristic's
/// notifications.
pub struct WinGattSource {
    inbound: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl BleSource for WinGattSource {
    type Error = WindowsBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, WindowsBleError> {
        let frame = self.inbound.recv().await.ok_or(WindowsBleError::Closed)?;
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
    data_char: GattCharacteristic,
    address: BleAddress,
}

impl BleSink for WinGattSink {
    type Error = WindowsBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), WindowsBleError> {
        for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
            let mut buf = [0u8; GATT_FRAGMENT_PAYLOAD + FRAGMENT_SCRATCH];
            let len = fragment
                .encode(&mut buf)
                .ok_or(WindowsBleError::FrameTooLarge)?;
            let bytes = buf
                .get(..len)
                .ok_or(WindowsBleError::FrameTooLarge)?
                .to_vec();
            gatt_write(
                self.data_char.clone(),
                bytes,
                GattWriteOption::WriteWithoutResponse,
            )
            .await?;
        }
        Ok(())
    }
}

/// Headroom over the payload for the fragment's 5-byte header when sizing the encode scratch buffer.
const FRAGMENT_SCRATCH: usize = 8;

/// The Windows native-BLE backend handed to the engine's `BluetoothAuto` supervisor.
pub struct WindowsBleBackend {
    /// Dropping this signals the radio thread to unpark and tear the WinRT objects down.
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    radio: Radio,
    /// In-flight central dials; each resolves to the formed link (or `None` on failure).
    dials: JoinSet<Option<WinGattLink>>,
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
            Ok(Ok(Ok(radio))) => Ok(Self {
                _keepalive: keepalive,
                events: events_rx,
                radio,
                dials: JoinSet::new(),
            }),
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

/// Build a WinRT `IBuffer` holding `bytes`.
fn ibuffer_from(bytes: &[u8]) -> Result<IBuffer, WindowsBleError> {
    let writer = DataWriter::new()?;
    writer.WriteBytes(bytes)?;
    Ok(writer.DetachBuffer()?)
}

/// Read a WinRT `IBuffer` into an owned byte vector.
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
) -> Result<BluetoothLEAdvertisementWatcher, WindowsBleError> {
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    // Active scanning pulls the scan response too and yields RSSI per sighting.
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

    let filter = BluetoothLEAdvertisementFilter::new()?;
    filter
        .Advertisement()?
        .ServiceUuids()?
        .Append(guid_of(BLE_SERVICE_UUID))?;
    watcher.SetAdvertisementFilter(&filter)?;

    watcher.Received(&TypedEventHandler::new(
        move |_sender, args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
            if let Some(args) = args.as_ref() {
                if let Some(sighting) = sighting_from(args) {
                    // The consumer may be gone (node shutting down); a closed channel is benign.
                    let _ = events_tx.send(sighting);
                }
            }
            Ok(())
        },
    ))?;
    Ok(watcher)
}

/// Convert a WinRT advertisement-received event into a `Sighting`.
fn sighting_from(args: &BluetoothLEAdvertisementReceivedEventArgs) -> Option<Event> {
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

    let adapter: BluetoothAdapter = BluetoothAdapter::GetDefaultAsync()?.get()?;
    if !adapter.IsLowEnergySupported()? {
        return Err(WindowsBleError::PeripheralRoleUnsupported);
    }
    if !adapter.IsPeripheralRoleSupported()? {
        return Err(WindowsBleError::PeripheralRoleUnsupported);
    }
    let radio = adapter.GetRadioAsync()?.get()?;
    if radio.State()? != RadioState::On {
        return Err(WindowsBleError::RadioOff);
    }

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

    let watcher = build_watcher(events_tx)?;
    watcher.Start()?;

    log::info!(
        "bluetooth: WinRT adapter powered on; GATT service published, scanning for Prns peers"
    );
    Ok(Radio {
        provider,
        control,
        data,
        watcher,
    })
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

    let control_char = discover_characteristic(&device, NATIVE_CONTROL_UUID)?;
    let data_char = discover_characteristic(&device, NATIVE_DATA_UUID)?;
    // Both characteristics live on the same service; hold it (and the device) to keep the link up.
    let service = control_char.Service()?;

    let (control_tx, control_rx) = tokio_mpsc::unbounded_channel::<Control>();
    subscribe(&control_char, move |bytes| {
        if let Some(control) = Control::decode(&bytes) {
            let _ = control_tx.send(control);
        }
    })?;

    let (data_tx, data_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
    subscribe(&data_char, move |bytes| {
        let _ = data_tx.send(Box::from(bytes.as_slice()));
    })?;

    log::debug!(
        "bluetooth: dialled {:02x?} — control + data characteristics subscribed",
        address.octets()
    );
    Ok(WinGattLink {
        address,
        control_char,
        data_char,
        control_rx,
        data_rx: Some(data_rx),
        _device: device,
        _service: service,
    })
}

/// Discover the first characteristic with `uuid` under our service on the connected device.
fn discover_characteristic(
    device: &BluetoothLEDevice,
    uuid: BleUuid,
) -> Result<GattCharacteristic, WindowsBleError> {
    let services = device
        .GetGattServicesForUuidAsync(guid_of(BLE_SERVICE_UUID))?
        .get()?;
    if services.Status()? != GattCommunicationStatus::Success {
        return Err(WindowsBleError::DialFailed);
    }
    let service = services
        .Services()?
        .into_iter()
        .next()
        .ok_or(WindowsBleError::DialFailed)?;
    let chars = service
        .GetCharacteristicsForUuidAsync(guid_of(uuid))?
        .get()?;
    if chars.Status()? != GattCommunicationStatus::Success {
        return Err(WindowsBleError::DialFailed);
    }
    chars
        .Characteristics()?
        .into_iter()
        .next()
        .ok_or(WindowsBleError::DialFailed)
}

/// Subscribe to a characteristic's notifications, routing each value to `on_value`, then enable the
/// CCCD so the peer actually notifies.
fn subscribe<F>(characteristic: &GattCharacteristic, on_value: F) -> Result<(), WindowsBleError>
where
    F: Fn(Vec<u8>) + Send + 'static,
{
    characteristic.ValueChanged(&TypedEventHandler::new(
        move |_sender, args: &Option<GattValueChangedEventArgs>| {
            if let Some(args) = args.as_ref() {
                if let Ok(buffer) = args.CharacteristicValue() {
                    if let Ok(bytes) = bytes_from(&buffer) {
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

impl BleBackend for WindowsBleBackend {
    const MAX_PEERS: usize = 8;
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
                    if let Ok(Some(link)) = joined {
                        return BleEvent::LinkReady {
                            link,
                            origin: Origin::Dialed,
                            peer_rssi: None,
                        };
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
                Ok(link) => Some(link),
                Err(error) => {
                    log::warn!(
                        "bluetooth: dial to {:02x?} failed ({error:?})",
                        address.octets()
                    );
                    None
                }
            });
    }
}
