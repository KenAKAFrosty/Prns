use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use prns_core::interfaces::bluetooth_auto::core::{BleAddress, Control};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use windows::core::{IInspectable, GUID};
use windows::Devices::Bluetooth::BluetoothError;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattLocalService, GattProtectionLevel, GattSubscribedClient, GattWriteOption,
    GattWriteRequestedEventArgs,
};
use windows::Foundation::TypedEventHandler;

use super::data_plane::{ClientSlot, LinkPlane, WinGattLink};
use super::{bytes_from, ibuffer_from, Event, WindowsBleError};

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

pub(super) async fn notify_local(
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

pub(super) fn wire_inbound(
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
        crate::diagnostic_log::warn!("bluetooth: inbound control write failed ({error:?})");
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
    crate::diagnostic_log::debug!(
        "bluetooth: inbound peer {:02x?} connected (accepted role)",
        address.octets()
    );
    let _ = events_tx.send(Event::Inbound(link));
    Ok(control_tx)
}

fn handle_data_write(args: Option<&GattWriteRequestedEventArgs>, registry: &InboundRegistry) {
    let Some(args) = args else { return };
    if let Err(error) = process_data_write(args, registry) {
        crate::diagnostic_log::warn!("bluetooth: inbound data write failed ({error:?})");
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
        crate::diagnostic_log::warn!("bluetooth: inbound subscription sync failed ({error:?})");
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

fn address_standin(device_id: &str) -> [u8; 6] {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let bytes = hash.to_be_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
}

pub(super) fn publish_characteristic(
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
