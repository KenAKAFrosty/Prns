use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc as sync_mpsc;
use std::sync::Arc;
use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, DialOutcome, Origin, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleIdentity, BLE_SERVICE_UUID, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID,
    COLUMBA_TX_UUID, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattServiceProvider, GattServiceProviderAdvertisingParameters,
};
use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothAddressType, BluetoothError};
use windows::Devices::Radios::RadioState;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use super::central::connect_blocking;
use super::data_plane::WinGattLink;
use super::peripheral::{publish_characteristic, publish_static_characteristic, wire_inbound};
use super::watcher::{build_watcher, scan_action, spawn_watcher_heartbeat, ScanAction};
use super::{guid_of, Event, Radio, ScanIntent, WindowsBleError};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(35);
const ADAPTER_ATTEMPTS: usize = 12;
const ADAPTER_RETRY_DELAY: Duration = Duration::from_secs(2);

pub struct WindowsBleBackend {
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    radio: Radio,
    watcher_heartbeat: JoinHandle<()>,
    dials: JoinSet<Result<WinGattLink, BleAddress>>,
    dial_queue: VecDeque<(BleAddress, BluetoothAddressType)>,
    seen_address_types: HashMap<BleAddress, BluetoothAddressType>,
}

impl WindowsBleBackend {
    pub const MAX_PEERS: usize = 8;

    pub async fn new(identity: BleIdentity) -> Result<Self, WindowsBleError> {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<Radio, WindowsBleError>>();

        std::thread::Builder::new()
            .name("prns-ble-winrt".into())
            .spawn(move || {
                let _ = ready_tx.send(winrt_setup(events_tx, identity));
                let _ = shutdown_rx.recv();
            })
            .map_err(|_| WindowsBleError::Closed)?;

        match tokio::time::timeout(POWER_ON_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(radio))) => {
                let watcher_heartbeat = spawn_watcher_heartbeat(
                    radio.watcher.clone(),
                    radio.adverts.clone(),
                    radio.scan_intent.clone(),
                );
                Ok(Self {
                    _keepalive: keepalive,
                    events: events_rx,
                    radio,
                    watcher_heartbeat,
                    dials: JoinSet::new(),
                    dial_queue: VecDeque::new(),
                    seen_address_types: HashMap::new(),
                })
            }
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(WindowsBleError::Closed),
            Err(_) => Err(WindowsBleError::PowerOnTimeout),
        }
    }
}

fn winrt_setup(
    events_tx: tokio_mpsc::UnboundedSender<Event>,
    identity: BleIdentity,
) -> Result<Radio, WindowsBleError> {
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

    let properties = GattCharacteristicProperties::Write
        | GattCharacteristicProperties::WriteWithoutResponse
        | GattCharacteristicProperties::Notify;
    let control = publish_characteristic(&service, guid_of(NATIVE_CONTROL_UUID), properties)?;
    let data = publish_characteristic(&service, guid_of(NATIVE_DATA_UUID), properties)?;
    let columba_rx = publish_characteristic(
        &service,
        guid_of(COLUMBA_RX_UUID),
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse,
    )?;
    let columba_tx = publish_characteristic(
        &service,
        guid_of(COLUMBA_TX_UUID),
        GattCharacteristicProperties::Read | GattCharacteristicProperties::Notify,
    )?;
    let columba_identity = publish_static_characteristic(
        &service,
        guid_of(COLUMBA_IDENTITY_UUID),
        identity.as_bytes(),
    )?;
    wire_inbound(&control, &data, &columba_rx, &columba_tx, events_tx.clone())?;

    let adverts = Arc::new(AtomicU64::new(0));
    let scan_intent = ScanIntent::new();
    let watcher = build_watcher(events_tx, adverts.clone(), scan_intent.clone())?;

    crate::diagnostic_log::debug!("bluetooth: WinRT adapter powered on; GATT service published");
    Ok(Radio {
        provider,
        _control: control,
        _data: data,
        _columba_rx: columba_rx,
        _columba_tx: columba_tx,
        _columba_identity: columba_identity,
        watcher,
        adverts,
        scan_intent,
    })
}

fn acquire_adapter() -> Result<(), WindowsBleError> {
    let mut last = WindowsBleError::NoAdapter;
    for attempt in 1..=ADAPTER_ATTEMPTS {
        match try_adapter() {
            Ok(()) => return Ok(()),
            Err(error) => {
                crate::diagnostic_log::warn!(
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

impl Drop for WindowsBleBackend {
    fn drop(&mut self) {
        self.radio.scan_intent.request(ScanningMode::Off);
        self.watcher_heartbeat.abort();
        let _ = self.radio.watcher.Stop();
        let _ = self.radio.provider.StopAdvertising();
    }
}

impl BleBackend<{ WindowsBleBackend::MAX_PEERS }> for WindowsBleBackend {
    type Error = WindowsBleError;
    type Link = WinGattLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), WindowsBleError> {
        if mode.is_on() {
            // Connectable + discoverable: WinRT folds the service's 128-bit UUID into the
            // advertisement automatically when discoverable, so we do not hand-roll the AD bytes.
            let parameters = GattServiceProviderAdvertisingParameters::new()?;
            parameters.SetIsConnectable(true)?;
            parameters.SetIsDiscoverable(true)?;
            self.radio
                .provider
                .StartAdvertisingWithParameters(&parameters)?;
            crate::diagnostic_log::debug!(
                "bluetooth: advertising the Prns service (connectable + discoverable)"
            );
        } else {
            self.radio.provider.StopAdvertising()?;
            crate::diagnostic_log::debug!("bluetooth: stopped advertising");
        }
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), WindowsBleError> {
        self.radio.scan_intent.request(mode);
        self.apply_scan_state()
    }

    async fn next_event(&mut self) -> BleEvent<WinGattLink> {
        loop {
            let pending_dials = !self.dials.is_empty();
            tokio::select! {
                event = self.events.recv() => match event {
                    Some(Event::Sighting {
                        address,
                        address_type,
                        rssi,
                    }) => {
                        self.seen_address_types.insert(address, address_type);
                        crate::diagnostic_log::debug!(
                            "bluetooth: sighted Prns peer {:02x?} type={address_type:?} rssi={rssi:?}",
                            address.octets()
                        );
                        return BleEvent::Sighting { address, rssi };
                    }
                    Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                    None => core::future::pending().await,
                },
                Some(joined) = self.dials.join_next(), if pending_dials => {
                    if self.dials.is_empty() {
                        match self.dial_queue.pop_front() {
                            Some((address, address_type)) => self.begin_dial(address, address_type),
                            None => {
                                self.radio.scan_intent.release_dial_hold();
                                let _ = self.apply_scan_state();
                            }
                        }
                    }
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

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        let address_type = self
            .seen_address_types
            .get(&address)
            .copied()
            .unwrap_or(BluetoothAddressType::Unspecified);
        if self.dials.is_empty() {
            self.begin_dial(address, address_type);
        } else if !self.dial_queue.iter().any(|(queued, _)| *queued == address) {
            crate::diagnostic_log::debug!(
                "bluetooth: dial to {:02x?} queued behind the in-flight dial",
                address.octets()
            );
            self.dial_queue.push_back((address, address_type));
        }
        DialOutcome::Started
    }
}

impl WindowsBleBackend {
    fn apply_scan_state(&self) -> Result<(), WindowsBleError> {
        let status = self.radio.watcher.Status()?;
        match scan_action(self.radio.scan_intent.is_effective(), status) {
            ScanAction::Start => {
                self.radio.watcher.Start()?;
                crate::diagnostic_log::debug!("bluetooth: scanning for Prns peers");
            }
            ScanAction::Stop => {
                self.radio.watcher.Stop()?;
                crate::diagnostic_log::debug!("bluetooth: scanning paused");
            }
            ScanAction::None => {}
        }
        Ok(())
    }

    fn begin_dial(&mut self, address: BleAddress, address_type: BluetoothAddressType) {
        self.radio.scan_intent.hold_for_dial();
        let _ = self.apply_scan_state();
        crate::diagnostic_log::debug!(
            "bluetooth: dialling {:02x?} type={address_type:?} over LE (central role)",
            address.octets()
        );
        self.dials
            .spawn_blocking(move || match connect_blocking(address, address_type) {
                Ok(link) => Ok(link),
                Err(error) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {:02x?} failed ({error:?})",
                        address.octets()
                    );
                    Err(address)
                }
            });
    }
}
