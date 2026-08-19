use std::ffi::OsString;
use std::io;
#[cfg(target_os = "macos")]
use std::net::TcpListener as StdTcpListener;
use std::pin::Pin;
use std::process::Child;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::task::{Context, Poll};
use std::time::Duration;

use crate::tcp::tune;
use nusb::descriptors::TransferType;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, Direction, In, Out, Recipient};
use nusb::{DeviceInfo, MaybeFuture};
use prns_core::interfaces::usb_auto::{
    ANDROID_ACCESSORY_DESCRIPTION, ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL,
    ANDROID_ACCESSORY_SERIAL, ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::diagnostic_log::info;
use crate::serial::{open_host_serial, scan_usb_serial_ports, HostSerial};

use super::{UsbAutoCandidate, UsbAutoIncarnation};

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
#[cfg(target_os = "macos")]
const USBMUX_DEVICE_PORT: u16 = 42_700;
const DEFAULT_USBMUX_TARGET: &str = "127.0.0.1:42700";
const USBMUX_TARGET_ENV: &str = "PRNS_USB_AUTO_USBMUX_TARGET";
const USBMUX_AUTO_ENV: &str = "PRNS_USB_AUTO_USBMUX_AUTO";
const COMPAT_USBMUX_TARGET_ENV: &str = "HOPSPOT_USBMUX_TARGET";
const COMPAT_USBMUX_AUTO_ENV: &str = "HOPSPOT_USBMUX_AUTO";
const ANDROID_ACCESSORY_ENV: &str = "PRNS_USB_AUTO_ANDROID_ACCESSORY";
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
            if path.is_empty() {
                return Err(malformed_target());
            }
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
            if bus.is_empty() || fields.next().is_some() {
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
            if bus.is_empty() || fields.next().is_some() {
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
            if bus.is_empty() || fields.next().is_some() {
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

pub fn scan_native_usb_auto_targets() -> Vec<UsbAutoCandidate> {
    let mut targets: Vec<UsbAutoCandidate> = scan_usb_serial_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|port| {
            let locator = UsbAutoTarget::Cdc(port.path().to_string()).encode();
            UsbAutoCandidate::unclassified_attachment(
                locator,
                UsbAutoIncarnation::new(port.incarnation()),
            )
        })
        .collect();
    if let Some(target) = configured_usbmux_target() {
        targets.push(UsbAutoCandidate::prns_specific(
            UsbAutoTarget::UsbMuxTcp { target }.encode(),
        ));
    } else {
        targets.extend(scan_ios_usbmux_udids().into_iter().map(|udid| {
            UsbAutoCandidate::prns_specific(UsbAutoTarget::UsbMuxIos { udid }.encode())
        }));
    }

    let Ok(devices) = nusb::list_devices().wait() else {
        return targets;
    };
    for device in devices {
        if is_webusb_auto(&device) {
            targets.extend(
                webusb_auto_targets(&device)
                    .into_iter()
                    .map(|target| UsbAutoCandidate::prns_specific(target.encode())),
            );
        } else if is_android_accessory(&device) {
            targets.extend(
                accessory_targets(&device)
                    .into_iter()
                    .map(|target| UsbAutoCandidate::prns_specific(target.encode())),
            );
        } else if android_accessory_switch_enabled() && may_support_android_open_accessory(&device)
        {
            targets.push(UsbAutoCandidate::prns_specific(
                UsbAutoTarget::AndroidStartAccessory {
                    bus: device.bus_id().to_string(),
                    address: device.device_address(),
                }
                .encode(),
            ));
        }
    }
    targets
}

pub async fn open_native_usb_auto_target(
    candidate: UsbAutoCandidate,
    baud: u32,
) -> io::Result<NativeUsbAutoStream> {
    match UsbAutoTarget::decode(candidate.locator())? {
        UsbAutoTarget::Cdc(path) => open_host_serial(&path, baud)
            .map(|stream| NativeUsbAutoStream(NativeUsbAutoStreamInner::Serial(stream))),
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
            info!("usb-auto: requesting Android Open Accessory on {bus}:{address}");
            start_android_accessory(&bus, address).await
        }
    }
}

fn configured_usbmux_target() -> Option<String> {
    configured_usbmux_target_from(
        std::env::var_os(USBMUX_TARGET_ENV),
        std::env::var_os(USBMUX_AUTO_ENV),
        std::env::var_os(COMPAT_USBMUX_TARGET_ENV),
        std::env::var_os(COMPAT_USBMUX_AUTO_ENV),
    )
}

fn configured_usbmux_target_from(
    target: Option<OsString>,
    auto: Option<OsString>,
    compat_target: Option<OsString>,
    compat_auto: Option<OsString>,
) -> Option<String> {
    normalized_target(target)
        .or_else(|| auto.map(|_| DEFAULT_USBMUX_TARGET.to_string()))
        .or_else(|| normalized_target(compat_target))
        .or_else(|| compat_auto.map(|_| DEFAULT_USBMUX_TARGET.to_string()))
}

fn normalized_target(value: Option<OsString>) -> Option<String> {
    value
        .and_then(|value| value.into_string().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn android_accessory_switch_enabled() -> bool {
    android_accessory_switch_enabled_from(std::env::var_os(ANDROID_ACCESSORY_ENV))
}

fn android_accessory_switch_enabled_from(value: Option<OsString>) -> bool {
    value.is_some()
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
async fn open_managed_usbmux_ios(udid: &str) -> io::Result<NativeUsbAutoStream> {
    let local_port = reserve_local_port()?;
    let forwarder = UsbMuxForwarder::spawn(udid, local_port)?;
    let target = format!("127.0.0.1:{local_port}");
    match connect_usbmux_tcp(&target).await {
        Ok(stream) => {
            info!("usb-auto: opened managed usbmux target {target} for iOS device {udid}");
            Ok(NativeUsbAutoStream(NativeUsbAutoStreamInner::UsbMuxTcp(
                UsbMuxTcp {
                    stream,
                    _forwarder: Some(forwarder),
                },
            )))
        }
        Err(error) => {
            drop(forwarder);
            Err(error)
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn open_managed_usbmux_ios(_udid: &str) -> io::Result<NativeUsbAutoStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "managed iOS usbmux is only supported on macOS",
    ))
}

async fn open_usbmux_tcp(
    target: &str,
    forwarder: Option<UsbMuxForwarder>,
) -> io::Result<NativeUsbAutoStream> {
    let stream = connect_usbmux_tcp(target).await?;
    info!("usb-auto: opened usbmux TCP target {target}");
    Ok(NativeUsbAutoStream(NativeUsbAutoStreamInner::UsbMuxTcp(
        UsbMuxTcp {
            stream,
            _forwarder: forwarder,
        },
    )))
}

async fn connect_usbmux_tcp(target: &str) -> io::Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + USBMUX_CONNECT_TIMEOUT;
    loop {
        match TcpStream::connect(target).await {
            Ok(stream) => {
                tune(&stream);
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

#[cfg(target_os = "macos")]
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
    is_webusb_auto_identity(device.vendor_id(), device.product_id())
}

fn is_webusb_auto_identity(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == WEBUSB_VENDOR_ID && product_id == WEBUSB_PRODUCT_ID
}

fn webusb_auto_targets(device: &DeviceInfo) -> Vec<UsbAutoTarget> {
    let bus = device.bus_id().to_string();
    let address = device.device_address();
    webusb_interface_numbers(device.interfaces().map(|interface| {
        (
            interface.class(),
            interface.subclass(),
            interface.protocol(),
            interface.interface_number(),
        )
    }))
    .map(|interface| UsbAutoTarget::WebUsbAuto {
        bus: bus.clone(),
        address,
        interface,
    })
    .collect()
}

fn webusb_interface_numbers(
    interfaces: impl Iterator<Item = (u8, u8, u8, u8)>,
) -> impl Iterator<Item = u8> {
    interfaces.filter_map(|(class, subclass, protocol, interface)| {
        (class == 0xFF && subclass == 0 && protocol == 0).then_some(interface)
    })
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

async fn open_webusb_auto(
    bus: &str,
    address: u8,
    interface: u8,
) -> io::Result<NativeUsbAutoStream> {
    let (stream, actual_in, actual_out) =
        open_bulk_interface(bus, address, interface, None).await?;
    info!(
        "usb-auto: opened WebUSB Auto {bus}:{address} interface={interface} in=0x{actual_in:02x} out=0x{actual_out:02x}"
    );
    Ok(NativeUsbAutoStream(NativeUsbAutoStreamInner::WebUsbAuto(
        stream,
    )))
}

async fn start_android_accessory(bus: &str, address: u8) -> io::Result<NativeUsbAutoStream> {
    let info = find_device(bus, address)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Android USB device vanished"))?;
    let device = info.open().await.map_err(nusb_error)?;
    let control = aoa_control(&info, &device).await?;
    let protocol = aoa_control_in(
        &control,
        ControlIn {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: AOA_GET_PROTOCOL,
            value: 0,
            index: 0,
            length: 2,
        },
    )
    .await?;
    if protocol.len() < 2 || u16::from_le_bytes([protocol[0], protocol[1]]) == 0 {
        return Err(io::Error::other("Android device does not support AOA"));
    }
    let protocol = u16::from_le_bytes([protocol[0], protocol[1]]);
    info!("usb-auto: Android Open Accessory protocol v{protocol}");
    send_aoa_string(
        &control,
        AOA_STRING_MANUFACTURER,
        ANDROID_ACCESSORY_MANUFACTURER,
    )
    .await?;
    send_aoa_string(&control, AOA_STRING_MODEL, ANDROID_ACCESSORY_MODEL).await?;
    send_aoa_string(
        &control,
        AOA_STRING_DESCRIPTION,
        ANDROID_ACCESSORY_DESCRIPTION,
    )
    .await?;
    send_aoa_string(&control, AOA_STRING_VERSION, ANDROID_ACCESSORY_VERSION).await?;
    send_aoa_string(&control, AOA_STRING_URI, ANDROID_ACCESSORY_URI).await?;
    send_aoa_string(&control, AOA_STRING_SERIAL, ANDROID_ACCESSORY_SERIAL).await?;
    aoa_control_out(
        &control,
        ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: AOA_START,
            value: 0,
            index: 0,
            data: &[],
        },
    )
    .await?;

    tokio::time::sleep(AOA_REENUMERATE_GRACE).await;
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "Android accessory re-enumeration requested",
    ))
}

#[cfg(target_os = "windows")]
type AoaControl = nusb::Interface;

#[cfg(not(target_os = "windows"))]
type AoaControl = nusb::Device;

#[cfg(target_os = "windows")]
async fn aoa_control(info: &DeviceInfo, device: &nusb::Device) -> io::Result<AoaControl> {
    let interface = info
        .interfaces()
        .next()
        .ok_or_else(|| io::Error::other("Android USB device has no claimable interface"))?
        .interface_number();
    device.claim_interface(interface).await.map_err(nusb_error)
}

#[cfg(not(target_os = "windows"))]
async fn aoa_control(_info: &DeviceInfo, device: &nusb::Device) -> io::Result<AoaControl> {
    Ok(device.clone())
}

async fn aoa_control_in(control: &AoaControl, data: ControlIn) -> io::Result<Vec<u8>> {
    control
        .control_in(data, USB_CONTROL_TIMEOUT)
        .await
        .map_err(nusb_transfer_error)
}

async fn aoa_control_out(control: &AoaControl, data: ControlOut<'_>) -> io::Result<()> {
    control
        .control_out(data, USB_CONTROL_TIMEOUT)
        .await
        .map_err(nusb_transfer_error)
}

async fn send_aoa_string(control: &AoaControl, index: u16, value: &str) -> io::Result<()> {
    let mut nul_terminated = Vec::with_capacity(value.len() + 1);
    nul_terminated.extend_from_slice(value.as_bytes());
    nul_terminated.push(0);
    aoa_control_out(
        control,
        ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: AOA_SEND_STRING,
            value: 0,
            index,
            data: &nul_terminated,
        },
    )
    .await
}

async fn open_android_accessory(
    bus: &str,
    address: u8,
    interface: u8,
    in_endpoint: u8,
    out_endpoint: u8,
) -> io::Result<NativeUsbAutoStream> {
    let (stream, actual_in, actual_out) =
        open_bulk_interface(bus, address, interface, Some((in_endpoint, out_endpoint))).await?;
    info!(
        "usb-auto: opened Android accessory {bus}:{address} interface={interface} in=0x{actual_in:02x} out=0x{actual_out:02x}"
    );
    Ok(NativeUsbAutoStream(
        NativeUsbAutoStreamInner::AndroidAccessory(stream),
    ))
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
    select_bulk_endpoints(descriptor.endpoints().map(|endpoint| {
        (
            endpoint.transfer_type(),
            endpoint.direction(),
            endpoint.address(),
        )
    }))
}

fn select_bulk_endpoints(
    endpoints: impl Iterator<Item = (TransferType, Direction, u8)>,
) -> Option<(u8, u8)> {
    let mut in_endpoint = None;
    let mut out_endpoint = None;
    for (transfer_type, direction, address) in endpoints {
        if transfer_type != TransferType::Bulk {
            continue;
        }
        match direction {
            Direction::In => in_endpoint.get_or_insert(address),
            Direction::Out => out_endpoint.get_or_insert(address),
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

pub struct NativeUsbAutoStream(NativeUsbAutoStreamInner);

enum NativeUsbAutoStreamInner {
    Serial(HostSerial),
    UsbMuxTcp(UsbMuxTcp),
    WebUsbAuto(BulkUsb),
    AndroidAccessory(BulkUsb),
}

impl AsyncRead for NativeUsbAutoStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            NativeUsbAutoStreamInner::Serial(stream) => Pin::new(stream).poll_read(cx, buf),
            NativeUsbAutoStreamInner::UsbMuxTcp(stream) => {
                Pin::new(&mut stream.stream).poll_read(cx, buf)
            }
            NativeUsbAutoStreamInner::WebUsbAuto(stream)
            | NativeUsbAutoStreamInner::AndroidAccessory(stream) => {
                Pin::new(stream).poll_read(cx, buf)
            }
        }
    }
}

impl AsyncWrite for NativeUsbAutoStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            NativeUsbAutoStreamInner::Serial(stream) => Pin::new(stream).poll_write(cx, buf),
            NativeUsbAutoStreamInner::UsbMuxTcp(stream) => {
                Pin::new(&mut stream.stream).poll_write(cx, buf)
            }
            NativeUsbAutoStreamInner::WebUsbAuto(stream)
            | NativeUsbAutoStreamInner::AndroidAccessory(stream) => {
                Pin::new(stream).poll_write(cx, buf)
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            NativeUsbAutoStreamInner::Serial(stream) => Pin::new(stream).poll_flush(cx),
            NativeUsbAutoStreamInner::UsbMuxTcp(stream) => {
                Pin::new(&mut stream.stream).poll_flush(cx)
            }
            NativeUsbAutoStreamInner::WebUsbAuto(stream)
            | NativeUsbAutoStreamInner::AndroidAccessory(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            NativeUsbAutoStreamInner::Serial(stream) => Pin::new(stream).poll_shutdown(cx),
            NativeUsbAutoStreamInner::UsbMuxTcp(stream) => {
                Pin::new(&mut stream.stream).poll_shutdown(cx)
            }
            NativeUsbAutoStreamInner::WebUsbAuto(stream)
            | NativeUsbAutoStreamInner::AndroidAccessory(stream) => {
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
    fn target_codec_round_trips_every_transport() {
        let cases = [
            UsbAutoTarget::Cdc("/dev/ttyACM0".to_string()),
            UsbAutoTarget::UsbMuxTcp {
                target: "127.0.0.1:42700".to_string(),
            },
            UsbAutoTarget::WebUsbAuto {
                bus: "usb-bus".to_string(),
                address: 7,
                interface: 2,
            },
            UsbAutoTarget::UsbMuxIos {
                udid: "00008027-000E05943E53802E".to_string(),
            },
            UsbAutoTarget::AndroidAccessory {
                bus: "android-bus".to_string(),
                address: 8,
                interface: 3,
                in_endpoint: 0x84,
                out_endpoint: 0x05,
            },
            UsbAutoTarget::AndroidStartAccessory {
                bus: "android-start-bus".to_string(),
                address: 9,
            },
        ];

        for target in cases {
            let encoded = target.encode();
            assert_eq!(UsbAutoTarget::decode(&encoded).as_ref().ok(), Some(&target));
        }
    }

    #[test]
    fn malformed_targets_are_rejected() {
        let malformed = [
            "",
            "cdc:",
            "usbmux:",
            "usbmux-ios:",
            "webusb::1:2",
            "webusb:bus:x:2",
            "webusb:bus:1",
            "webusb:bus:1:2:3",
            "aoa::1:2:129:1",
            "aoa:bus:1:2:129",
            "aoa:bus:1:2:129:1:0",
            "aoa-start::1",
            "aoa-start:bus:x",
            "aoa-start:bus:1:2",
        ];

        for encoded in malformed {
            assert!(UsbAutoTarget::decode(encoded).is_err(), "{encoded}");
        }
    }

    #[test]
    fn webusb_identity_and_interface_selection_are_exact() {
        assert!(is_webusb_auto_identity(WEBUSB_VENDOR_ID, WEBUSB_PRODUCT_ID));
        assert!(!is_webusb_auto_identity(
            WEBUSB_VENDOR_ID,
            WEBUSB_PRODUCT_ID + 1
        ));
        assert!(!is_webusb_auto_identity(
            WEBUSB_VENDOR_ID + 1,
            WEBUSB_PRODUCT_ID
        ));

        let selected: Vec<_> = webusb_interface_numbers(
            [(0xFF, 0, 0, 4), (0xFF, 1, 0, 5), (0x02, 0, 0, 6)].into_iter(),
        )
        .collect();
        assert_eq!(selected, vec![4]);
    }

    #[test]
    fn bulk_endpoint_selection_requires_both_directions() {
        let selected = select_bulk_endpoints(
            [
                (TransferType::Interrupt, Direction::In, 0x82),
                (TransferType::Bulk, Direction::Out, 0x03),
                (TransferType::Bulk, Direction::In, 0x84),
                (TransferType::Bulk, Direction::In, 0x85),
            ]
            .into_iter(),
        );
        assert_eq!(selected, Some((0x84, 0x03)));
        assert_eq!(
            select_bulk_endpoints([(TransferType::Bulk, Direction::In, 0x81)].into_iter()),
            None
        );
    }

    #[test]
    fn android_accessory_switch_requires_explicit_presence() {
        assert!(!android_accessory_switch_enabled_from(None));
        assert!(android_accessory_switch_enabled_from(Some(OsString::new())));
    }

    #[test]
    fn usbmux_environment_precedence_favors_prns_names() {
        let configured = configured_usbmux_target_from(
            Some(OsString::from("  primary:1  ")),
            Some(OsString::new()),
            Some(OsString::from("compat:2")),
            Some(OsString::new()),
        );
        assert_eq!(configured.as_deref(), Some("primary:1"));

        let primary_auto = configured_usbmux_target_from(
            None,
            Some(OsString::new()),
            Some(OsString::from("compat:2")),
            Some(OsString::new()),
        );
        assert_eq!(primary_auto.as_deref(), Some(DEFAULT_USBMUX_TARGET));

        let compatibility = configured_usbmux_target_from(
            None,
            None,
            Some(OsString::from(" compat:2 ")),
            Some(OsString::new()),
        );
        assert_eq!(compatibility.as_deref(), Some("compat:2"));

        let compatibility_auto =
            configured_usbmux_target_from(None, None, None, Some(OsString::new()));
        assert_eq!(compatibility_auto.as_deref(), Some(DEFAULT_USBMUX_TARGET));
        assert_eq!(configured_usbmux_target_from(None, None, None, None), None);
    }
}
