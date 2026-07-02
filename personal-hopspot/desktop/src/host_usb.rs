use std::io;
use std::net::TcpListener as StdTcpListener;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::task::{Context, Poll};
use std::time::Duration;

use nusb::descriptors::TransferType;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, Direction, In, Out, Recipient};
use nusb::{DeviceInfo, MaybeFuture};
use personal_rns::interfaces::tcp::tokio_socket;
use personal_rns::interfaces::usb_auto::core::{
    ANDROID_ACCESSORY_DESCRIPTION, ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL,
    ANDROID_ACCESSORY_SERIAL, ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

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
const USBMUX_DEVICE_PORT: u16 = 42_700;
const DEFAULT_USBMUX_TARGET: &str = "127.0.0.1:42700";
const USBMUX_TARGET_ENV: &str = "HOPSPOT_USBMUX_TARGET";
const USBMUX_AUTO_ENV: &str = "HOPSPOT_USBMUX_AUTO";
const USBMUX_CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const USBMUX_CONNECT_POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Eq, PartialEq)]
enum UsbAutoTarget {
    Cdc(String),
    UsbMuxTcp {
        target: String,
    },
    WebUsbAuto {
        bus: String,
        address: u8,
        interface: u8,
    },
    UsbMuxIos {
        udid: String,
    },
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
            Self::UsbMuxTcp { target } => format!("usbmux:{target}"),
            Self::WebUsbAuto {
                bus,
                address,
                interface,
            } => format!("webusb:{bus}:{address}:{interface}"),
            Self::UsbMuxIos { udid } => format!("usbmux-ios:{udid}"),
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
        if let Some(target) = encoded.strip_prefix("usbmux:") {
            if target.is_empty() {
                return Err(malformed_target());
            }
            return Ok(Self::UsbMuxTcp {
                target: target.to_string(),
            });
        }
        if let Some(rest) = encoded.strip_prefix("webusb:") {
            let mut fields = rest.split(':');
            let bus = fields.next().ok_or_else(malformed_target)?.to_string();
            let address = parse_u8(fields.next())?;
            let interface = parse_u8(fields.next())?;
            if fields.next().is_some() {
                return Err(malformed_target());
            }
            return Ok(Self::WebUsbAuto {
                bus,
                address,
                interface,
            });
        }
        if let Some(udid) = encoded.strip_prefix("usbmux-ios:") {
            if udid.is_empty() {
                return Err(malformed_target());
            }
            return Ok(Self::UsbMuxIos {
                udid: udid.to_string(),
            });
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
    if let Some(target) = configured_usbmux_target() {
        targets.push(UsbAutoTarget::UsbMuxTcp { target }.encode());
    } else {
        targets.extend(
            scan_ios_usbmux_udids()
                .into_iter()
                .map(|udid| UsbAutoTarget::UsbMuxIos { udid }.encode()),
        );
    }

    let Ok(devices) = nusb::list_devices().wait() else {
        return targets;
    };
    for device in devices {
        if is_webusb_auto(&device) {
            targets.extend(
                webusb_auto_targets(&device)
                    .into_iter()
                    .map(|target| target.encode()),
            );
        } else if is_android_accessory(&device) {
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
        UsbAutoTarget::UsbMuxTcp { target } => open_usbmux_tcp(&target, None).await,
        UsbAutoTarget::UsbMuxIos { udid } => open_managed_usbmux_ios(&udid).await,
        UsbAutoTarget::WebUsbAuto {
            bus,
            address,
            interface,
        } => open_webusb_auto(&bus, address, interface).await,
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

fn configured_usbmux_target() -> Option<String> {
    if let Some(target) = std::env::var_os(USBMUX_TARGET_ENV)
        .and_then(|value| value.into_string().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(target);
    }
    std::env::var_os(USBMUX_AUTO_ENV).map(|_| DEFAULT_USBMUX_TARGET.to_string())
}

#[cfg(target_os = "macos")]
fn scan_ios_usbmux_udids() -> Vec<String> {
    let Ok(output) = Command::new("idevice_id").arg("-l").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn scan_ios_usbmux_udids() -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "macos")]
async fn open_managed_usbmux_ios(udid: &str) -> io::Result<HostUsb> {
    let local_port = reserve_local_port()?;
    let forwarder = UsbMuxForwarder::spawn(udid, local_port)?;
    let target = format!("127.0.0.1:{local_port}");
    match connect_usbmux_tcp(&target).await {
        Ok(stream) => {
            eprintln!("usb-auto: opened managed usbmux target {target} for iOS device {udid}");
            Ok(HostUsb::UsbMuxTcp(UsbMuxTcp {
                stream,
                _forwarder: Some(forwarder),
            }))
        }
        Err(error) => {
            drop(forwarder);
            Err(error)
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn open_managed_usbmux_ios(_udid: &str) -> io::Result<HostUsb> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "managed iOS usbmux is only supported on macOS",
    ))
}

async fn open_usbmux_tcp(target: &str, forwarder: Option<UsbMuxForwarder>) -> io::Result<HostUsb> {
    let stream = connect_usbmux_tcp(target).await?;
    eprintln!("usb-auto: opened usbmux TCP target {target}");
    Ok(HostUsb::UsbMuxTcp(UsbMuxTcp {
        stream,
        _forwarder: forwarder,
    }))
}

async fn connect_usbmux_tcp(target: &str) -> io::Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + USBMUX_CONNECT_TIMEOUT;
    loop {
        match TcpStream::connect(target).await {
            Ok(stream) => {
                tokio_socket::tune(&stream);
                return Ok(stream);
            }
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(USBMUX_CONNECT_POLL).await;
            }
            Err(error) => return Err(error),
        }
    }
}

fn reserve_local_port() -> io::Result<u16> {
    let listener = StdTcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

struct UsbMuxForwarder {
    child: Child,
}

impl UsbMuxForwarder {
    #[cfg(target_os = "macos")]
    fn spawn(udid: &str, local_port: u16) -> io::Result<Self> {
        let mapping = format!("{local_port}:{USBMUX_DEVICE_PORT}");
        let child = Command::new("iproxy")
            .args(["-u", udid, "-s", "127.0.0.1", &mapping])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self { child })
    }
}

impl Drop for UsbMuxForwarder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct UsbMuxTcp {
    stream: TcpStream,
    _forwarder: Option<UsbMuxForwarder>,
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

fn is_webusb_auto(device: &DeviceInfo) -> bool {
    device.vendor_id() == WEBUSB_VENDOR_ID && device.product_id() == WEBUSB_PRODUCT_ID
}

fn webusb_auto_targets(device: &DeviceInfo) -> Vec<UsbAutoTarget> {
    let bus = device.bus_id().to_string();
    let address = device.device_address();
    device
        .interfaces()
        .filter(|interface| {
            interface.class() == 0xFF && interface.subclass() == 0 && interface.protocol() == 0
        })
        .map(|interface| UsbAutoTarget::WebUsbAuto {
            bus: bus.clone(),
            address,
            interface: interface.interface_number(),
        })
        .collect()
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

async fn open_webusb_auto(bus: &str, address: u8, interface: u8) -> io::Result<HostUsb> {
    let (stream, actual_in, actual_out) =
        open_bulk_interface(bus, address, interface, None).await?;
    eprintln!(
        "usb-auto: opened WebUSB Auto {bus}:{address} interface={interface} in=0x{actual_in:02x} out=0x{actual_out:02x}"
    );
    Ok(HostUsb::WebUsbAuto(stream))
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
    let (stream, actual_in, actual_out) =
        open_bulk_interface(bus, address, interface, Some((in_endpoint, out_endpoint))).await?;
    eprintln!(
        "usb-auto: opened Android accessory {bus}:{address} interface={interface} in=0x{actual_in:02x} out=0x{actual_out:02x}"
    );
    Ok(HostUsb::AndroidAccessory(stream))
}

async fn open_bulk_interface(
    bus: &str,
    address: u8,
    interface: u8,
    endpoint_fallback: Option<(u8, u8)>,
) -> io::Result<(BulkUsb, u8, u8)> {
    let info = find_device(bus, address)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "USB Auto device vanished"))?;
    let device = info.open().await.map_err(nusb_error)?;
    let claimed = device
        .claim_interface(interface)
        .await
        .map_err(nusb_error)?;
    let (actual_in, actual_out) = find_bulk_endpoints(&claimed)
        .or(endpoint_fallback)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "USB Auto interface has no bulk endpoint pair",
            )
        })?;
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
    Ok((
        BulkUsb {
            _interface: claimed,
            reader,
            writer,
        },
        actual_in,
        actual_out,
    ))
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
    UsbMuxTcp(UsbMuxTcp),
    WebUsbAuto(BulkUsb),
    AndroidAccessory(BulkUsb),
}

impl AsyncRead for HostUsb {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::UsbMuxTcp(stream) => Pin::new(&mut stream.stream).poll_read(cx, buf),
            Self::WebUsbAuto(stream) | Self::AndroidAccessory(stream) => {
                Pin::new(stream).poll_read(cx, buf)
            }
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
            Self::UsbMuxTcp(stream) => Pin::new(&mut stream.stream).poll_write(cx, buf),
            Self::WebUsbAuto(stream) | Self::AndroidAccessory(stream) => {
                Pin::new(stream).poll_write(cx, buf)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_flush(cx),
            Self::UsbMuxTcp(stream) => Pin::new(&mut stream.stream).poll_flush(cx),
            Self::WebUsbAuto(stream) | Self::AndroidAccessory(stream) => {
                Pin::new(stream).poll_flush(cx)
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Serial(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::UsbMuxTcp(stream) => Pin::new(&mut stream.stream).poll_shutdown(cx),
            Self::WebUsbAuto(stream) | Self::AndroidAccessory(stream) => {
                Pin::new(stream).poll_shutdown(cx)
            }
        }
    }
}

pub struct BulkUsb {
    _interface: nusb::Interface,
    reader: EndpointRead<Bulk>,
    writer: EndpointWrite<Bulk>,
}

impl AsyncRead for BulkUsb {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for BulkUsb {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_codec_round_trips_managed_ios_usbmux() {
        let target = UsbAutoTarget::UsbMuxIos {
            udid: "00008027-000E05943E53802E".to_string(),
        };

        let encoded = target.encode();

        assert_eq!(encoded, "usbmux-ios:00008027-000E05943E53802E");
        assert_eq!(UsbAutoTarget::decode(&encoded).unwrap(), target);
    }

    #[test]
    fn empty_managed_ios_usbmux_target_is_rejected() {
        assert!(UsbAutoTarget::decode("usbmux-ios:").is_err());
    }
}
