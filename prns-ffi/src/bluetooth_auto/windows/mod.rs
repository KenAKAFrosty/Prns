mod backend;
mod central;
mod data_plane;
mod peripheral;
mod watcher;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use prns_core::interfaces::bluetooth_auto::{BleAddress, BleUuid};
use windows::core::GUID;
use windows::Devices::Bluetooth::Advertisement::BluetoothLEAdvertisementWatcher;
use windows::Devices::Bluetooth::BluetoothAddressType;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCommunicationStatus, GattLocalCharacteristic, GattServiceProvider,
};
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

use data_plane::WinGattLink;

pub use backend::WindowsBleBackend;
pub use data_plane::{WinGattSink, WinGattSource};

#[derive(Debug)]
pub enum WindowsBleError {
    NoAdapter,
    PeripheralRoleUnsupported,
    RadioOff,
    ServicePublishFailed,
    DialFailed,
    Closed,
    PowerOnTimeout,
    ControlTooLarge,
    FrameTooLarge,
    WriteFailed,
    MissingSubscribedClient,
    InvalidNotificationMtu { available: usize },
    NotificationTooLarge { len: usize, available: usize },
    NotificationFailed { status: GattCommunicationStatus },
    MissingColumbaIdentity,
    Winrt(windows::core::Error),
}

impl From<windows::core::Error> for WindowsBleError {
    fn from(error: windows::core::Error) -> Self {
        WindowsBleError::Winrt(error)
    }
}

struct Radio {
    provider: GattServiceProvider,
    _control: GattLocalCharacteristic,
    _data: GattLocalCharacteristic,
    _columba_rx: GattLocalCharacteristic,
    _columba_tx: GattLocalCharacteristic,
    _columba_identity: GattLocalCharacteristic,
    watcher: BluetoothLEAdvertisementWatcher,
    adverts: Arc<AtomicU64>,
    scan_intent: ScanIntent,
}

#[derive(Clone)]
struct ScanIntent(Arc<AtomicBool>);

impl ScanIntent {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    fn set(&self, requested: bool) {
        self.0.store(requested, Ordering::Release);
    }

    fn is_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

enum Event {
    Sighting {
        address: BleAddress,
        address_type: BluetoothAddressType,
        rssi: Option<i8>,
    },
    Inbound(WinGattLink),
}

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

/// The 48-bit Bluetooth LE address WinRT works in `u64`s; the sighting kept the low six bytes big-endian, so
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
