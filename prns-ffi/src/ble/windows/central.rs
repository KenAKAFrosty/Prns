use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::core::{
    BleAddress, BleUuid, Control, BLE_SERVICE_UUID, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
    GattCommunicationStatus, GattSession, GattValueChangedEventArgs, GattWriteOption,
};
use windows::Devices::Bluetooth::{
    BluetoothAddressType, BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
};
use windows::Foundation::TypedEventHandler;

use super::data_plane::{request_throughput, LinkPlane, WinGattLink};
use super::{address_to_u64, bytes_from, guid_of, ibuffer_from, WindowsBleError};

const DIAL_DISCOVERY_ATTEMPTS: usize = 4;
const DIAL_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(400);
pub(super) async fn gatt_write(
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

pub(super) fn connect_blocking(
    address: BleAddress,
    address_type: BluetoothAddressType,
) -> Result<WinGattLink, WindowsBleError> {
    let raw_address = address_to_u64(address);
    let device = if address_type == BluetoothAddressType::Unspecified {
        BluetoothLEDevice::FromBluetoothAddressAsync(raw_address)?.get()?
    } else {
        BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(
            raw_address,
            address_type,
        )?
        .get()?
    };

    // Pin the connection up. WinRT otherwise drops an idle GATT client link shortly after discovery,
    // which is the "connected then dormant" flakiness; MaintainConnection holds it for the session's
    // (== link's) lifetime.
    let session = GattSession::FromDeviceIdAsync(&device.BluetoothDeviceId()?)?.get()?;
    session.SetMaintainConnection(true)?;
    let connection_request = request_throughput(&device);

    let (closed_tx, closed_rx) = watch::channel(false);
    device.ConnectionStatusChanged(&TypedEventHandler::new(
        move |sender: &Option<BluetoothLEDevice>, _args: &Option<windows::core::IInspectable>| {
            let disconnected = sender
                .as_ref()
                .and_then(|device| device.ConnectionStatus().ok())
                .map(|status| status == BluetoothConnectionStatus::Disconnected)
                .unwrap_or(true);
            if disconnected {
                crate::diagnostic_log::debug!(
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
                    crate::diagnostic_log::debug!(
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

    crate::diagnostic_log::debug!(
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
        crate::diagnostic_log::warn!(
            "bluetooth: service discovery failed (connection={connection:?}, status={service_status:?})"
        );
        return Err(WindowsBleError::DialFailed);
    }
    let service = match services.Services()?.into_iter().next() {
        Some(service) => service,
        None => {
            crate::diagnostic_log::warn!(
                "bluetooth: service discovery succeeded but the Prns service was absent"
            );
            return Err(WindowsBleError::DialFailed);
        }
    };
    let chars = service
        .GetCharacteristicsForUuidWithCacheModeAsync(guid_of(uuid), BluetoothCacheMode::Uncached)?
        .get()?;
    let char_status = chars.Status()?;
    if char_status != GattCommunicationStatus::Success {
        crate::diagnostic_log::warn!(
            "bluetooth: characteristic discovery failed (status={char_status:?})"
        );
        return Err(WindowsBleError::DialFailed);
    }
    match chars.Characteristics()?.into_iter().next() {
        Some(characteristic) => Ok(characteristic),
        None => {
            crate::diagnostic_log::warn!(
                "bluetooth: characteristic discovery succeeded but the characteristic was absent"
            );
            Err(WindowsBleError::DialFailed)
        }
    }
}

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
                        crate::diagnostic_log::debug!(
                            "bluetooth: notify in {label} {} bytes",
                            bytes.len()
                        );
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
