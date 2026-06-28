use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use nusb::descriptors::TransferType;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, Direction, In, Out, Recipient};
use nusb::{DeviceInfo, MaybeFuture};
use personal_rns::interfaces::usb_auto::core::{
    ANDROID_ACCESSORY_DESCRIPTION, ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL,
    ANDROID_ACCESSORY_SERIAL, ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::host_serial::{open_host_serial, HostSerial};

const GOOGLE_VENDOR_ID: u16 = 0x18D1;
const AOA_PRODUCT_ACCESSORY: u16 = 0x2D00;
const AOA_PRODUCT_ACCESSORY_ADB: u16 = 0x2D01;

const AOA_GET_PROTOCOL: u8 = 51;
const AOA_SEND_STRING: u8 = 52;
const AOA_START: u8 = 53;

const AOA_STRING_MANUFACTURER: u16 = 0;
const AOA_STRING_MODEL: u16 = 1;
const AOA_STRING_DESCRIPTION: u16 = 2;
const AOA_STRING_VERSION: u16 = 3;
const AOA_STRING_URI: u16 = 4;
const AOA_STRING_SERIAL: u16 = 5;

const USB_CONTROL_TIMEOUT: Duration = Duration::from_millis(250);
const AOA_REENUMERATE_GRACE: Duration = Duration::from_secs(2);
const BULK_TRANSFER_BYTES: usize = 8 * 1024;
const BULK_TRANSFERS: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
enum UsbAutoTarget {
    Cdc(String),
    AndroidAccessory {
        bus: String,
        address: u8,
        interface: u8,
        in_endpoint: u8,
        out_endpoint: u8,
    },
    AndroidStartAccessory {
        bus: String,
        address: u8,
    },
}

impl UsbAutoTarget {
    fn encode(&self) -> String {
        match self {
            Self::Cdc(path) => format!("cdc:{path}"),
            Self::AndroidAccessory {
                bus,
                address,
                interface,
                in_endpoint,
                out_endpoint,
            } => format!("aoa:{bus}:{address}:{interface}:{in_endpoint}:{out_endpoint}"),
            Self::AndroidStartAccessory { bus, address } => {
                format!("aoa-start:{bus}:{address}")
            }
        }
    }

    fn decode(encoded: &str) -> io::Result<Self> {
        if let Some(path) = encoded.strip_prefix("cdc:") {
            return Ok(Self::Cdc(path.to_string()));
        }
        if let Some(rest) = encoded.strip_prefix("aoa:") {
            let mut fields = rest.split(':');
            let bus = fields.next().ok_or_else(malformed_target)?.to_string();
            let address = parse_u8(fields.next())?;
            let interface = parse_u8(fields.next())?;
            let in_endpoint = parse_u8(fields.next())?;
            let out_endpoint = parse_u8(fields.next())?;
            if fields.next().is_some() {
                return Err(malformed_target());
            }
            return Ok(Self::AndroidAccessory {
                bus,
                address,
                interface,
                in_endpoint,
                out_endpoint,
            });
        }
        if let Some(rest) = encoded.strip_prefix("aoa-start:") {
            let mut fields = rest.split(':');
            let bus = fields.next().ok_or_else(malformed_target)?.to_string();
            let address = parse_u8(fields.next())?;
            if fields.next().is_some() {
                return Err(malformed_target());
            }
            return Ok(Self::AndroidStartAccessory { bus, address });
        }
        Err(malformed_target())
    }
}

fn parse_u8(value: Option<&str>) -> io::Result<u8> {
    value
        .ok_or_else(malformed_target)?
        .parse::<u8>()
        .map_err(|_| malformed_target())
}

fn malformed_target() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "malformed USB Auto target")
}

pub fn scan_usb_auto_targets() -> Vec<String> {
    let mut targets: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| UsbAutoTarget::Cdc(info.port_name).encode())
        .collect();

    let Ok(devices) = nusb::list_devices().wait() else {
        return targets;
    };
    for device in devices {
        if is_android_accessory(&device) {
            targets.extend(
                accessory_targets(&device)
                    .into_iter()
                    .map(|target| target.encode()),
            );
        } else if may_support_android_open_accessory(&device) {
            targets.push(
                UsbAutoTarget::AndroidStartAccessory {
                    bus: device.bus_id().to_string(),
                    address: device.device_address(),
                }
                .encode(),
            );
        }
    }
    targets
}

pub async fn open_usb_auto_target(encoded: String, baud: u32) -> io::Result<HostUsb> {
    match UsbAutoTarget::decode(&encoded)? {
        UsbAutoTarget::Cdc(path) => open_host_serial(&path, baud).map(HostUsb::Serial),
        UsbAutoTarget::AndroidAccessory {
            bus,
            address,
            interface,
            in_endpoint,
            out_endpoint,
        } => open_android_accessory(&bus, address, interface, in_endpoint, out_endpoint).await,
        UsbAutoTarget::AndroidStartAccessory { bus, address } => {
            eprintln!("usb-auto: requesting Android Open Accessory on {bus}:{address}");
            start_android_accessory(&bus, address).await
        }
    }
}

fn is_android_accessory(device: &DeviceInfo) -> bool {
    device.vendor_id() == GOOGLE_VENDOR_ID
        && matches!(
            device.product_id(),
            AOA_PRODUCT_ACCESSORY | AOA_PRODUCT_ACCESSORY_ADB
        )
}

fn may_support_android_open_accessory(device: &DeviceInfo) -> bool {
    device.vendor_id() == GOOGLE_VENDOR_ID && !is_android_accessory(device)
}

fn accessory_targets(device: &DeviceInfo) -> Vec<UsbAutoTarget> {
    let bus = device.bus_id().to_string();
    let address = device.device_address();
    device
        .interfaces()
        .filter_map(|interface| {
            if interface.class() != 0xFF
                || interface.subclass() != 0xFF
                || interface.protocol() != 0
            {
                return None;
            }
            Some(UsbAutoTarget::AndroidAccessory {
                bus: bus.clone(),
                address,
                interface: interface.interface_number(),
                in_endpoint: 0x81,
                out_endpoint: 0x01,
            })
        })
        .collect()
}

async fn start_android_accessory(bus: &str, address: u8) -> io::Result<HostUsb> {
    let info = find_device(bus, address)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Android USB device vanished"))?;
    let device = info.open().await.map_err(nusb_error)?;
    let protocol = device
        .control_in(
            ControlIn {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: AOA_GET_PROTOCOL,
                value: 0,
                index: 0,
                length: 2,
            },
            USB_CONTROL_TIMEOUT,
        )
        .await
        .map_err(nusb_transfer_error)?;
    if protocol.len() < 2 || u16::from_le_bytes([protocol[0], protocol[1]]) == 0 {
        return Err(io::Error::other("Android device does not support AOA"));
    }
    let protocol = u16::from_le_bytes([protocol[0], protocol[1]]);
    eprintln!("usb-auto: Android Open Accessory protocol v{protocol}");
    send_aoa_string(
        &device,
        AOA_STRING_MANUFACTURER,
        ANDROID_ACCESSORY_MANUFACTURER,
    )
    .await?;
    send_aoa_string(&device, AOA_STRING_MODEL, ANDROID_ACCESSORY_MODEL).await?;
    send_aoa_string(
        &device,
        AOA_STRING_DESCRIPTION,
        ANDROID_ACCESSORY_DESCRIPTION,
    )
    .await?;
    send_aoa_string(&device, AOA_STRING_VERSION, ANDROID_ACCESSORY_VERSION).await?;
    send_aoa_string(&device, AOA_STRING_URI, ANDROID_ACCESSORY_URI).await?;
    send_aoa_string(&device, AOA_STRING_SERIAL, ANDROID_ACCESSORY_SERIAL).await?;
    device
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: AOA_START,
                value: 0,
                index: 0,
                data: &[],
            },
            USB_CONTROL_TIMEOUT,
        )
        .await
        .map_err(nusb_transfer_error)?;

    tokio::time::sleep(AOA_REENUMERATE_GRACE).await;
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "Android accessory re-enumeration requested",
    ))
}

async fn send_aoa_string(device: &nusb::Device, index: u16, value: &str) -> io::Result<()> {
    let mut nul_terminated = Vec::with_capacity(value.len() + 1);
    nul_terminated.extend_from_slice(value.as_bytes());
    nul_terminated.push(0);
    device
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: AOA_SEND_STRING,
                value: 0,
                index,
                data: &nul_terminated,
            },
            USB_CONTROL_TIMEOUT,
        )
        .await
        .map_err(nusb_transfer_error)
}

async fn open_android_accessory(
    bus: &str,
    address: u8,
    interface: u8,
    in_endpoint: u8,
    out_endpoint: u8,
) -> io::Result<HostUsb> {
    let info = find_device(bus, address)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Android accessory vanished"))?;
    let device = info.open().await.map_err(nusb_error)?;
    let claimed = device
        .claim_interface(interface)
        .await
        .map_err(nusb_error)?;
    let (actual_in, actual_out) =
        find_bulk_endpoints(&claimed).unwrap_or((in_endpoint, out_endpoint));
    eprintln!(
        "usb-auto: opened Android accessory {bus}:{address} interface={interface} in=0x{actual_in:02x} out=0x{actual_out:02x}"
    );
    let reader = claimed
        .endpoint::<Bulk, In>(actual_in)
        .map_err(nusb_error)?
        .reader(BULK_TRANSFER_BYTES)
        .with_num_transfers(BULK_TRANSFERS);
    let writer = claimed
        .endpoint::<Bulk, Out>(actual_out)
        .map_err(nusb_error)?
        .writer(BULK_TRANSFER_BYTES)
        .with_num_transfers(BULK_TRANSFERS);
    Ok(HostUsb::AndroidAccessory(AndroidAccessoryUsb {
        _interface: claimed,
        reader,
        writer,
    }))
}

fn find_bulk_endpoints(interface: &nusb::Interface) -> Option<(u8, u8)> {
    let descriptor = interface.descriptor()?;
    let mut in_endpoint = None;
    let mut out_endpoint = None;
    for endpoint in descriptor.endpoints() {
        if endpoint.transfer_type() != TransferType::Bulk {
            continue;
        }
        match endpoint.direction() {
            Direction::In => in_endpoint.get_or_insert(endpoint.address()),
            Direction::Out => out_endpoint.get_or_insert(endpoint.address()),
        };
    }
    Some((in_endpoint?, out_endpoint?))
}

fn find_device(bus: &str, address: u8) -> Option<DeviceInfo> {
    nusb::list_devices()
        .wait()
        .ok()?
        .find(|device| device.bus_id() == bus && device.device_address() == address)
}

fn nusb_error(error: nusb::Error) -> io::Error {
    io::Error::other(error)
}

fn nusb_transfer_error(error: nusb::transfer::TransferError) -> io::Error {
    io::Error::other(error)
}

pub enum HostUsb {
    Serial(HostSerial),
    AndroidAccessory(AndroidAccessoryUsb),
}

impl AsyncRead for HostUsb {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::AndroidAccessory(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for HostUsb {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::AndroidAccessory(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_flush(cx),
            Self::AndroidAccessory(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::AndroidAccessory(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

pub struct AndroidAccessoryUsb {
    _interface: nusb::Interface,
    reader: EndpointRead<Bulk>,
    writer: EndpointWrite<Bulk>,
}

impl AsyncRead for AndroidAccessoryUsb {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for AndroidAccessoryUsb {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(cx)
    }
}
