use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc as sync_mpsc;
use std::sync::Arc;
use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::core::{
    BleAddress, BLE_SERVICE_UUID, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
use prns_core::interfaces::bluetooth_auto::limits;
use prns_core::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, Origin};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattServiceProvider, GattServiceProviderAdvertisingParameters,
};
use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothAddressType, BluetoothError};
use windows::Devices::Radios::RadioState;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use super::central::connect_blocking;
use super::data_plane::WinGattLink;
use super::peripheral::{publish_characteristic, wire_inbound};
use super::watcher::{build_watcher, spawn_watcher_heartbeat};
use super::{guid_of, Event, Radio, WindowsBleError};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(35);
const ADAPTER_ATTEMPTS: usize = 12;
const ADAPTER_RETRY_DELAY: Duration = Duration::from_secs(2);

pub struct WindowsBleBackend {
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    radio: Radio,
    dials: JoinSet<Result<WinGattLink, BleAddress>>,
    seen_address_types: HashMap<BleAddress, BluetoothAddressType>,
}

impl WindowsBleBackend {
    pub async fn new() -> Result<Self, WindowsBleError> {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<Radio, WindowsBleError>>();

        std::thread::Builder::new()
            .name("prns-ble-winrt".into())
            .spawn(move || {
                let _ = ready_tx.send(winrt_setup(events_tx));
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
                    seen_address_types: HashMap::new(),
                })
            }
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(WindowsBleError::Closed),
            Err(_) => Err(WindowsBleError::PowerOnTimeout),
        }
    }
}

fn winrt_setup(events_tx: tokio_mpsc::UnboundedSender<Event>) -> Result<Radio, WindowsBleError> {
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
    wire_inbound(&control, &data, events_tx.clone())?;

    let adverts = Arc::new(AtomicU64::new(0));
    let watcher = build_watcher(events_tx, adverts.clone())?;
    watcher.Start()?;

    crate::diagnostic_log::debug!(
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
            crate::diagnostic_log::debug!(
                "bluetooth: advertising the Prns service (connectable + discoverable)"
            );
        } else {
            self.radio.provider.StopAdvertising()?;
            crate::diagnostic_log::debug!("bluetooth: stopped advertising");
        }
        Ok(())
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
        let address_type = self
            .seen_address_types
            .get(&address)
            .copied()
            .unwrap_or(BluetoothAddressType::Unspecified);
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
