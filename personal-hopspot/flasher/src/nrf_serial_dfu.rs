use std::fmt;
use std::io;
use std::time::{Duration, Instant};

use nusb::transfer::{ControlOut, ControlType, Recipient};
use nusb::{DeviceInfo, MaybeFuture};
use prns_flash_manifest::{
    BoardBuild, BoardCatalogEntry, UsbVidPid, ValidatedNrfSerialDfuSerialTransport,
};
use prns_nrf_dfu::{
    Acknowledgement, AcknowledgementDecoder, AcknowledgementError, DfuTransfer, TransferError,
    TransferState, RELIABLE_FRAME_ATTEMPT_LIMIT,
};
use serde::Serialize;
use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPortInfo, SerialPortType, StopBits,
    UsbPortInfo,
};
use thiserror::Error;

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedNrfSerialDfuTarget;

const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(100);
const APPLICATION_TOUCH_HOLD: Duration = Duration::from_millis(100);
const BOOTLOADER_INITIALIZATION_WAIT: Duration = Duration::from_millis(1_500);
const BOOTLOADER_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(10);
const BOOTLOADER_ENUMERATION_INTERVAL: Duration = Duration::from_millis(100);
const BOOTLOADER_PORT_OPEN_WAIT: Duration = Duration::from_millis(100);
const ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(1);
const BOOTLOADER_ENTRY_CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
const CANCELLABLE_WAIT_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeviceMode {
    TouchApplicationOrBootloader,
    ManagedApplication,
}

impl fmt::Display for DeviceMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TouchApplicationOrBootloader => {
                formatter.write_str("touch application or bootloader")
            }
            Self::ManagedApplication => formatter.write_str("managed application"),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum SerialDfuError {
    #[error("could not enumerate serial ports: {0}")]
    PortEnumeration(#[source] serialport::Error),
    #[error("could not enumerate USB devices: {0}")]
    UsbEnumeration(#[source] nusb::Error),
    #[error("requested serial port {port:?} is not present")]
    RequestedPortMissing { port: String },
    #[error("requested serial port {port:?} is not an identifiable USB serial device")]
    RequestedPortNotUsb { port: String },
    #[error(
        "requested serial port {port:?} has USB identity {vendor_id:04x}:{product_id:04x}, which does not match this target"
    )]
    RequestedPortIdentity {
        port: String,
        vendor_id: u16,
        product_id: u16,
    },
    #[error("no exact touch application, managed application, or bootloader device is connected")]
    DeviceMissing,
    #[error("multiple matching devices are connected: {devices:?}; select a serial one with --port or disconnect the extras")]
    AmbiguousDevices { devices: Vec<String> },
    #[error("could not open application serial port {port:?} for bootloader entry: {source}")]
    TouchOpen {
        port: String,
        source: serialport::Error,
    },
    #[error("could not open managed application USB device {device:?}: {source}")]
    ManagedApplicationOpen { device: String, source: nusb::Error },
    #[error("could not claim managed application USB interface {interface_number}: {source}")]
    ManagedApplicationInterface {
        interface_number: u8,
        source: nusb::Error,
    },
    #[error("managed application rejected the bootloader-entry control request: {0}")]
    ManagedApplicationControl(#[source] nusb::transfer::TransferError),
    #[error("the exact bootloader did not appear after requesting it from {application:?}")]
    BootloaderDidNotAppear { application: String },
    #[error(
        "multiple exact bootloaders appeared after requesting one from {application:?}: {ports:?}"
    )]
    AmbiguousBootloadersAfterEntry {
        application: String,
        ports: Vec<String>,
    },
    #[error("could not open bootloader serial port {port:?}: {source}")]
    BootloaderOpen {
        port: String,
        source: serialport::Error,
    },
    #[error("could not clear bootloader serial buffers on {port:?}: {source}")]
    BootloaderClear {
        port: String,
        source: serialport::Error,
    },
    #[error(
        "Nordic DFU frame {sequence_number} failed after {attempts} reliable attempts: {source}"
    )]
    ReliableFrame {
        sequence_number: u8,
        attempts: u8,
        source: ReliableAttemptError,
    },
    #[error("Nordic DFU transfer state failed: {0}")]
    Transfer(#[from] TransferError),
    #[error("operation cancelled; no success was reported")]
    Cancelled,
}

#[derive(Debug, Error)]
pub(crate) enum ReliableAttemptError {
    #[error(transparent)]
    Write(#[from] FrameWriteError),
    #[error(transparent)]
    Acknowledgement(#[from] AcknowledgementReceiveError),
}

#[derive(Debug, Error)]
pub(crate) enum FrameWriteError {
    #[error("serial write failed: {0}")]
    Write(#[source] io::Error),
    #[error("serial flush failed: {0}")]
    Flush(#[source] io::Error),
}

#[derive(Debug, Error)]
pub(crate) enum AcknowledgementReceiveError {
    #[error("serial read failed: {0}")]
    Read(#[source] io::Error),
    #[error("received malformed acknowledgement: {0}")]
    Malformed(#[from] AcknowledgementError),
    #[error("expected acknowledgement {expected}, received {actual}")]
    Unexpected { expected: u8, actual: u8 },
    #[error("timed out waiting for acknowledgement {expected}")]
    Timeout { expected: u8 },
    #[error("operation cancelled while waiting for an acknowledgement")]
    Cancelled,
}

#[derive(Clone)]
struct SelectedSerialDevice {
    port: SerialPortInfo,
    usb: UsbPortInfo,
}

enum SelectedDevice {
    TouchApplicationOrBootloader(SelectedSerialDevice),
    ManagedApplication(DeviceInfo),
}

impl SelectedDevice {
    const fn mode(&self) -> DeviceMode {
        match self {
            Self::TouchApplicationOrBootloader(_) => DeviceMode::TouchApplicationOrBootloader,
            Self::ManagedApplication(_) => DeviceMode::ManagedApplication,
        }
    }

    fn connection_name(&self) -> String {
        match self {
            Self::TouchApplicationOrBootloader(device) => device.port.port_name.clone(),
            Self::ManagedApplication(device) => managed_device_name(device),
        }
    }

    fn observed_usb_identity(&self) -> (u16, u16) {
        match self {
            Self::TouchApplicationOrBootloader(device) => (device.usb.vid, device.usb.pid),
            Self::ManagedApplication(device) => (device.vendor_id(), device.product_id()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DoctorReport {
    pub(crate) port_name: String,
    pub(crate) mode: DeviceMode,
    pub(crate) vendor_id: u16,
    pub(crate) product_id: u16,
}

pub(crate) fn doctor(
    board: &BoardCatalogEntry,
    ports: Vec<SerialPortInfo>,
    requested_port: Option<&str>,
) -> Result<DoctorReport, AppError> {
    let transport = catalog_serial_transport(board)?;
    let managed_applications = if requested_port.is_some() {
        Vec::new()
    } else {
        matching_managed_applications(&transport).map_err(map_preflight_error)?
    };
    let selected = select_device(&ports, &managed_applications, requested_port, &transport)
        .map_err(map_preflight_error)?;
    let mode = selected.mode();
    let (vendor_id, product_id) = selected.observed_usb_identity();
    Ok(DoctorReport {
        port_name: selected.connection_name(),
        mode,
        vendor_id,
        product_id,
    })
}

pub(crate) fn flash(
    board: &BoardCatalogEntry,
    target: &PreparedNrfSerialDfuTarget,
    requested_port: Option<&str>,
    reporter: Reporter,
) -> Result<(), AppError> {
    let transport = target.serial_transport();
    let ports = serialport::available_ports()
        .map_err(SerialDfuError::PortEnumeration)
        .map_err(map_preflight_error)?;
    let managed_applications = if requested_port.is_some() {
        Vec::new()
    } else {
        matching_managed_applications(transport).map_err(map_preflight_error)?
    };
    let selected = select_device(&ports, &managed_applications, requested_port, transport)
        .map_err(map_preflight_error)?;
    let bootloader = match selected {
        SelectedDevice::TouchApplicationOrBootloader(device) => {
            reporter.phase(
                Phase::Resetting,
                Some(&board.slug),
                "Ensuring the exact device is in its Nordic serial bootloader…",
            );
            touch_application(&device.port, transport.touch_baud_rate())
                .map_err(map_preflight_error)?;
            let expected = match device.usb.serial_number.as_deref() {
                Some(serial_number) => {
                    ExpectedBootloader::UsbSerialNumber(serial_number.to_string())
                }
                None => ExpectedBootloader::UniqueExactDevice,
            };
            await_bootloader(&device.port.port_name, expected, transport)
                .map_err(AppError::nrf_serial_dfu)?
        }
        SelectedDevice::ManagedApplication(device) => {
            reporter.phase(
                Phase::Resetting,
                Some(&board.slug),
                "Requesting the exact Nordic serial bootloader from Personal Hopspot USB…",
            );
            let application = managed_device_name(&device);
            request_managed_bootloader(&device, transport).map_err(map_preflight_error)?;
            await_bootloader(
                &application,
                ExpectedBootloader::UniqueExactDevice,
                transport,
            )
            .map_err(AppError::nrf_serial_dfu)?
        }
    };
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    reporter.phase(
        Phase::Connecting,
        Some(&board.slug),
        &format!("Opening Nordic bootloader {}…", bootloader.port_name),
    );
    let mut port = open_bootloader(&bootloader.port_name, transport.transfer_baud_rate())
        .map_err(AppError::nrf_serial_dfu)?;
    let image = target
        .image()
        .map_err(|error| AppError::trust_artifact(error.to_string()))?;
    let total = image.firmware().len() as u64;
    reporter.phase(
        Phase::Writing,
        Some(&board.slug),
        &format!("Transferring {total} bytes through reliable Nordic serial DFU…"),
    );
    let mut io = SerialPortDfuIo {
        port: &mut *port,
        is_cancelled: &crate::esp::cancelled,
    };
    run_transfer(
        &mut io,
        DfuTransfer::new(image, target.bank_layout()),
        |duration| cancellable_sleep(duration, &crate::esp::cancelled),
        &crate::esp::cancelled,
        |written, total| {
            reporter.progress(
                Phase::Writing,
                Some(&board.slug),
                written as u64,
                total as u64,
            );
        },
    )
    .map_err(|error| match error {
        SerialDfuError::Cancelled => AppError::Cancelled,
        error => AppError::nrf_serial_dfu(error),
    })?;
    reporter.phase(
        Phase::VerifyingFlash,
        Some(&board.slug),
        "The bootloader accepted the complete transfer and finished its activation window.",
    );
    reporter.success(
        &board.slug,
        &format!(
            "Nordic serial DFU complete for {} ({total} bytes).",
            board.display_name
        ),
    );
    Ok(())
}

fn catalog_serial_transport(
    board: &BoardCatalogEntry,
) -> Result<ValidatedNrfSerialDfuSerialTransport, AppError> {
    let BoardBuild::NrfSerialDfu(build) = &board.build else {
        return Err(AppError::trust_catalog(format!(
            "{} is not a Nordic serial DFU target",
            board.display_name
        )));
    };
    build
        .serial
        .clone()
        .into_validated()
        .map_err(|error| AppError::trust_catalog(error.to_string()))
}

fn select_device(
    ports: &[SerialPortInfo],
    managed_applications: &[DeviceInfo],
    requested_port: Option<&str>,
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> Result<SelectedDevice, SerialDfuError> {
    if let Some(requested_port) = requested_port {
        let port = ports
            .iter()
            .find(|port| port_names_match(&port.port_name, requested_port))
            .cloned()
            .ok_or_else(|| SerialDfuError::RequestedPortMissing {
                port: requested_port.to_string(),
            })?;
        let SerialPortType::UsbPort(usb) = &port.port_type else {
            return Err(SerialDfuError::RequestedPortNotUsb {
                port: requested_port.to_string(),
            });
        };
        if !serial_usb_matches(usb, transport) {
            return Err(SerialDfuError::RequestedPortIdentity {
                port: requested_port.to_string(),
                vendor_id: usb.vid,
                product_id: usb.pid,
            });
        }
        let usb = usb.clone();
        return Ok(SelectedDevice::TouchApplicationOrBootloader(
            SelectedSerialDevice { port, usb },
        ));
    }

    let serial_devices = matching_selectable_ports(ports, transport);
    let match_count = serial_devices.len() + managed_applications.len();
    if match_count == 0 {
        return Err(SerialDfuError::DeviceMissing);
    }
    if match_count > 1 {
        let mut devices = serial_devices
            .iter()
            .map(|device| format!("touch application or bootloader {}", device.port.port_name))
            .chain(
                managed_applications
                    .iter()
                    .map(|device| format!("managed application {}", managed_device_name(device))),
            )
            .collect::<Vec<_>>();
        devices.sort();
        return Err(SerialDfuError::AmbiguousDevices { devices });
    }
    if let [serial_device] = serial_devices.as_slice() {
        return Ok(SelectedDevice::TouchApplicationOrBootloader(
            serial_device.clone(),
        ));
    }
    let [managed_application] = managed_applications else {
        return Err(SerialDfuError::DeviceMissing);
    };
    Ok(SelectedDevice::ManagedApplication(
        managed_application.clone(),
    ))
}

fn port_names_match(discovered: &str, requested: &str) -> bool {
    if discovered == requested {
        return true;
    }
    let discovered = std::fs::canonicalize(discovered);
    let requested = std::fs::canonicalize(requested);
    matches!((discovered, requested), (Ok(discovered), Ok(requested)) if discovered == requested)
}

fn matching_ports(ports: &[SerialPortInfo], identity: UsbVidPid) -> Vec<&SerialPortInfo> {
    ports
        .iter()
        .filter(|port| {
            matches!(
                &port.port_type,
                SerialPortType::UsbPort(usb) if usb_matches(usb.vid, usb.pid, identity)
            )
        })
        .collect()
}

fn matching_selectable_ports(
    ports: &[SerialPortInfo],
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> Vec<SelectedSerialDevice> {
    ports
        .iter()
        .filter_map(|port| {
            let SerialPortType::UsbPort(usb) = &port.port_type else {
                return None;
            };
            serial_usb_matches(usb, transport).then(|| SelectedSerialDevice {
                port: port.clone(),
                usb: usb.clone(),
            })
        })
        .collect()
}

fn serial_usb_matches(usb: &UsbPortInfo, transport: &ValidatedNrfSerialDfuSerialTransport) -> bool {
    usb_matches(
        usb.vid,
        usb.pid,
        transport.touch_application_and_bootloader_usb(),
    ) || (usb_matches(usb.vid, usb.pid, transport.recovery_bootloader_usb())
        && usb.manufacturer.as_deref() == Some(transport.recovery_bootloader_manufacturer())
        && usb.product.as_deref() == Some(transport.recovery_bootloader_product()))
}

fn matching_managed_applications(
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> Result<Vec<DeviceInfo>, SerialDfuError> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(SerialDfuError::UsbEnumeration)?;
    Ok(devices
        .filter(|device| managed_usb_matches(device, transport))
        .collect())
}

fn managed_usb_matches(
    device: &DeviceInfo,
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> bool {
    let identity = transport.managed_application_usb();
    device.vendor_id() == identity.vendor_id()
        && device.product_id() == identity.product_id()
        && device.manufacturer_string() == Some(transport.managed_application_manufacturer())
        && device.product_string() == Some(transport.managed_application_product())
        && device.serial_number() == Some(transport.managed_application_serial_number())
}

fn managed_device_name(device: &DeviceInfo) -> String {
    match device.serial_number() {
        Some(serial_number) => format!(
            "{:04x}:{:04x} {serial_number}",
            device.vendor_id(),
            device.product_id()
        ),
        None => format!("{:04x}:{:04x}", device.vendor_id(), device.product_id()),
    }
}

fn port_names(ports: &[&SerialPortInfo]) -> Vec<String> {
    ports.iter().map(|port| port.port_name.clone()).collect()
}

fn usb_matches(vendor_id: u16, product_id: u16, identity: UsbVidPid) -> bool {
    vendor_id == identity.vendor_id() && product_id == identity.product_id()
}

fn touch_application(port: &SerialPortInfo, baud_rate: u32) -> Result<(), SerialDfuError> {
    if crate::esp::cancelled() {
        return Err(SerialDfuError::Cancelled);
    }
    let touch_port = serial_builder(&port.port_name, baud_rate)
        .dtr_on_open(true)
        .open()
        .map_err(|source| SerialDfuError::TouchOpen {
            port: port.port_name.clone(),
            source,
        })?;
    std::thread::sleep(APPLICATION_TOUCH_HOLD);
    drop(touch_port);
    if crate::esp::cancelled() {
        Err(SerialDfuError::Cancelled)
    } else {
        Ok(())
    }
}

fn request_managed_bootloader(
    application: &DeviceInfo,
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> Result<(), SerialDfuError> {
    if crate::esp::cancelled() {
        return Err(SerialDfuError::Cancelled);
    }
    let device_name = managed_device_name(application);
    let device =
        application
            .open()
            .wait()
            .map_err(|source| SerialDfuError::ManagedApplicationOpen {
                device: device_name,
                source,
            })?;
    let interface_number = transport.managed_application_interface_number();
    let interface = device
        .claim_interface(interface_number)
        .wait()
        .map_err(|source| SerialDfuError::ManagedApplicationInterface {
            interface_number,
            source,
        })?;
    interface
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: transport.managed_application_request(),
                value: transport.managed_application_value(),
                index: transport.managed_application_index(),
                data: &[],
            },
            BOOTLOADER_ENTRY_CONTROL_TIMEOUT,
        )
        .wait()
        .map_err(SerialDfuError::ManagedApplicationControl)
}

enum ExpectedBootloader {
    UsbSerialNumber(String),
    UniqueExactDevice,
}

fn await_bootloader(
    application: &str,
    expected: ExpectedBootloader,
    transport: &ValidatedNrfSerialDfuSerialTransport,
) -> Result<SerialPortInfo, SerialDfuError> {
    cancellable_sleep(BOOTLOADER_INITIALIZATION_WAIT, &crate::esp::cancelled)?;
    let deadline = Instant::now() + BOOTLOADER_ENUMERATION_TIMEOUT;
    loop {
        if crate::esp::cancelled() {
            return Err(SerialDfuError::Cancelled);
        }
        let ports = serialport::available_ports().map_err(SerialDfuError::PortEnumeration)?;
        let exact_devices =
            matching_ports(&ports, transport.touch_application_and_bootloader_usb());
        let bootloaders = match &expected {
            ExpectedBootloader::UsbSerialNumber(serial_number) => exact_devices
                .iter()
                .filter(|port| {
                    matches!(
                        &port.port_type,
                        SerialPortType::UsbPort(usb)
                            if usb.serial_number.as_deref() == Some(serial_number.as_str())
                    )
                })
                .copied()
                .collect::<Vec<_>>(),
            ExpectedBootloader::UniqueExactDevice => exact_devices,
        };
        match bootloaders.as_slice() {
            [bootloader] => return Ok((*bootloader).clone()),
            [] => {}
            _ => {
                return Err(SerialDfuError::AmbiguousBootloadersAfterEntry {
                    application: application.to_string(),
                    ports: port_names(&bootloaders),
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(SerialDfuError::BootloaderDidNotAppear {
                application: application.to_string(),
            });
        }
        cancellable_sleep(BOOTLOADER_ENUMERATION_INTERVAL, &crate::esp::cancelled)?;
    }
}

fn serial_builder(port: &str, baud_rate: u32) -> serialport::SerialPortBuilder {
    serialport::new(port, baud_rate)
        .data_bits(DataBits::Eight)
        .flow_control(FlowControl::None)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .timeout(SERIAL_READ_TIMEOUT)
}

fn open_bootloader(
    port_name: &str,
    baud_rate: u32,
) -> Result<Box<dyn serialport::SerialPort>, SerialDfuError> {
    let port = serial_builder(port_name, baud_rate)
        .dtr_on_open(true)
        .open()
        .map_err(|source| SerialDfuError::BootloaderOpen {
            port: port_name.to_string(),
            source,
        })?;
    cancellable_sleep(BOOTLOADER_PORT_OPEN_WAIT, &crate::esp::cancelled)?;
    port.clear(ClearBuffer::All)
        .map_err(|source| SerialDfuError::BootloaderClear {
            port: port_name.to_string(),
            source,
        })?;
    Ok(port)
}

trait DfuTransportIo {
    fn write_frame(&mut self, frame: &[u8]) -> Result<(), FrameWriteError>;

    fn receive_expected_acknowledgement(
        &mut self,
        expected: Acknowledgement,
    ) -> Result<(), AcknowledgementReceiveError>;
}

struct SerialPortDfuIo<'a> {
    port: &'a mut dyn serialport::SerialPort,
    is_cancelled: &'a dyn Fn() -> bool,
}

impl DfuTransportIo for SerialPortDfuIo<'_> {
    fn write_frame(&mut self, frame: &[u8]) -> Result<(), FrameWriteError> {
        self.port.write_all(frame).map_err(FrameWriteError::Write)?;
        self.port.flush().map_err(FrameWriteError::Flush)
    }

    fn receive_expected_acknowledgement(
        &mut self,
        expected: Acknowledgement,
    ) -> Result<(), AcknowledgementReceiveError> {
        let deadline = Instant::now() + ACKNOWLEDGEMENT_TIMEOUT;
        let mut decoder = AcknowledgementDecoder::new();
        let mut buffer = [0_u8; 64];
        loop {
            if (self.is_cancelled)() {
                return Err(AcknowledgementReceiveError::Cancelled);
            }
            match self.port.read(&mut buffer) {
                Ok(0) => {}
                Ok(count) => {
                    for byte in &buffer[..count] {
                        if let Some(received) = decoder.push(*byte)? {
                            if received == expected {
                                return Ok(());
                            }
                            return Err(AcknowledgementReceiveError::Unexpected {
                                expected: expected.number(),
                                actual: received.number(),
                            });
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(AcknowledgementReceiveError::Read(error)),
            }
            if Instant::now() >= deadline {
                return Err(AcknowledgementReceiveError::Timeout {
                    expected: expected.number(),
                });
            }
        }
    }
}

fn run_transfer(
    io: &mut dyn DfuTransportIo,
    mut transfer: DfuTransfer<'_>,
    mut wait: impl FnMut(Duration) -> Result<(), SerialDfuError>,
    is_cancelled: &dyn Fn() -> bool,
    mut progress: impl FnMut(usize, usize),
) -> Result<(), SerialDfuError> {
    while transfer.state() != TransferState::Complete {
        if is_cancelled() {
            return Err(SerialDfuError::Cancelled);
        }
        let pending = transfer
            .next_frame()?
            .ok_or(TransferError::AlreadyComplete)?;
        let mut last_error = None;
        for _ in 0..RELIABLE_FRAME_ATTEMPT_LIMIT {
            if is_cancelled() {
                return Err(SerialDfuError::Cancelled);
            }
            let attempt = io
                .write_frame(pending.frame().bytes())
                .map_err(ReliableAttemptError::from)
                .and_then(|()| {
                    if is_cancelled() {
                        return Err(ReliableAttemptError::Acknowledgement(
                            AcknowledgementReceiveError::Cancelled,
                        ));
                    }
                    io.receive_expected_acknowledgement(pending.frame().expected_acknowledgement())
                        .map_err(ReliableAttemptError::from)
                });
            match attempt {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(ReliableAttemptError::Acknowledgement(
                    AcknowledgementReceiveError::Cancelled,
                )) => return Err(SerialDfuError::Cancelled),
                Err(error) => last_error = Some(error),
            }
        }
        if let Some(source) = last_error {
            return Err(SerialDfuError::ReliableFrame {
                sequence_number: pending.frame().sequence_number(),
                attempts: RELIABLE_FRAME_ATTEMPT_LIMIT,
                source,
            });
        }
        let acknowledgement = pending.frame().expected_acknowledgement();
        let required_wait = pending.wait_after_acknowledgement().duration();
        let transfer_progress = pending.progress_after_acknowledgement();
        transfer.acknowledge(acknowledgement)?;
        wait(required_wait)?;
        progress(
            transfer_progress.written_bytes(),
            transfer_progress.total_bytes(),
        );
    }
    Ok(())
}

fn cancellable_sleep(
    duration: Duration,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), SerialDfuError> {
    let deadline = Instant::now() + duration;
    loop {
        if is_cancelled() {
            return Err(SerialDfuError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(CANCELLABLE_WAIT_INTERVAL));
    }
}

fn map_preflight_error(error: SerialDfuError) -> AppError {
    match error {
        SerialDfuError::Cancelled => AppError::Cancelled,
        SerialDfuError::RequestedPortIdentity { .. }
        | SerialDfuError::RequestedPortNotUsb { .. }
        | SerialDfuError::DeviceMissing
        | SerialDfuError::AmbiguousDevices { .. } => AppError::device_identity(error.to_string()),
        SerialDfuError::UsbEnumeration(_)
        | SerialDfuError::ManagedApplicationOpen { .. }
        | SerialDfuError::ManagedApplicationInterface { .. }
        | SerialDfuError::ManagedApplicationControl(_) => {
            AppError::host_preflight(error.to_string())
        }
        SerialDfuError::PortEnumeration(_)
        | SerialDfuError::RequestedPortMissing { .. }
        | SerialDfuError::TouchOpen { .. } => AppError::serial_port(error.to_string()),
        error => AppError::nrf_serial_dfu(error),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use prns_flash_manifest::{
        NrfSerialDfuControlApplication, NrfSerialDfuRecoveryBootloader,
        NrfSerialDfuSerialTransport, NrfSerialDfuTouchApplicationAndBootloader, UsbVendorProductId,
        ValidatedNrfSerialDfuSerialTransport,
    };
    use prns_nrf_dfu::{
        ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuBankLayout,
        DfuDeviceRevision, DfuDeviceType, DfuImage, SoftdeviceFirmwareId, SoftdeviceRequirements,
    };
    use serialport::UsbPortInfo;

    use super::*;

    enum Reply {
        Expected,
        Timeout,
    }

    struct FakeDfuIo {
        writes: Vec<Vec<u8>>,
        replies: VecDeque<Reply>,
        cancel_after_write: Option<Rc<Cell<bool>>>,
    }

    impl DfuTransportIo for FakeDfuIo {
        fn write_frame(&mut self, frame: &[u8]) -> Result<(), FrameWriteError> {
            self.writes.push(frame.to_vec());
            if let Some(cancelled) = &self.cancel_after_write {
                cancelled.set(true);
            }
            Ok(())
        }

        fn receive_expected_acknowledgement(
            &mut self,
            expected: Acknowledgement,
        ) -> Result<(), AcknowledgementReceiveError> {
            match self.replies.pop_front().unwrap_or(Reply::Expected) {
                Reply::Expected => Ok(()),
                Reply::Timeout => Err(AcknowledgementReceiveError::Timeout {
                    expected: expected.number(),
                }),
            }
        }
    }

    fn transport() -> ValidatedNrfSerialDfuSerialTransport {
        NrfSerialDfuSerialTransport {
            touch_application_and_bootloader: NrfSerialDfuTouchApplicationAndBootloader {
                usb: UsbVendorProductId {
                    vendor_id: "0x2886".to_string(),
                    product_id: "0x0057".to_string(),
                },
                touch_baud_rate: 1_200,
                transfer_baud_rate: 115_200,
            },
            recovery_bootloader: NrfSerialDfuRecoveryBootloader {
                usb: UsbVendorProductId {
                    vendor_id: "0x239a".to_string(),
                    product_id: "0x8029".to_string(),
                },
                manufacturer: "Seeed Studio".to_string(),
                product: "T1000-E-BOOT".to_string(),
            },
            managed_application: NrfSerialDfuControlApplication {
                usb: UsbVendorProductId {
                    vendor_id: "0x1209".to_string(),
                    product_id: "0x0001".to_string(),
                },
                manufacturer: "Stay Personal".to_string(),
                product: "Personal Hopspot (T1000-E)".to_string(),
                serial_number: "PERSONAL-RNS-T1000E-HOP".to_string(),
                interface_number: 0,
                request: "0x50".to_string(),
                value: "0x5052".to_string(),
                index: "0x4e53".to_string(),
            },
        }
        .into_validated()
        .expect("valid transport")
    }

    fn usb_port(name: &str, vendor_id: u16, product_id: u16) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.to_string(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: vendor_id,
                pid: product_id,
                serial_number: Some(format!("serial-{name}")),
                manufacturer: None,
                product: None,
            }),
        }
    }

    fn recovery_bootloader_port(name: &str) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.to_string(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x239a,
                pid: 0x8029,
                serial_number: Some(format!("serial-{name}")),
                manufacturer: Some("Seeed Studio".to_string()),
                product: Some("T1000-E-BOOT".to_string()),
            }),
        }
    }

    fn image(firmware: &[u8]) -> DfuImage<'_> {
        let fwid = SoftdeviceFirmwareId::new(0x0123).expect("FWID");
        let init_packet = ApplicationInitPacket::build(
            firmware,
            &ApplicationInitPacketSpec {
                device_type: DfuDeviceType::new(0x0052),
                device_revision: DfuDeviceRevision::new(52840),
                application_version: ApplicationVersion::NotEnforced,
                softdevices: SoftdeviceRequirements::new(fwid, std::iter::empty())
                    .expect("requirements"),
            },
        )
        .expect("init packet");
        DfuImage::new(firmware, init_packet).expect("DFU image")
    }

    #[test]
    fn multiple_exact_devices_are_never_selected_implicitly() {
        let ports = vec![
            usb_port("first", 0x2886, 0x0057),
            usb_port("second", 0x2886, 0x0057),
        ];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Err(SerialDfuError::AmbiguousDevices { .. })
        ));
    }

    #[test]
    fn one_exact_touch_application_or_bootloader_is_selected() {
        let ports = vec![usb_port("serial", 0x2886, 0x0057)];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Ok(SelectedDevice::TouchApplicationOrBootloader(device)) if device.port == ports[0]
        ));
    }

    #[test]
    fn exact_recovery_bootloader_is_selected() {
        let ports = vec![recovery_bootloader_port("recovery")];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Ok(SelectedDevice::TouchApplicationOrBootloader(device)) if device.port == ports[0]
        ));
    }

    #[test]
    fn shared_recovery_usb_pair_requires_exact_strings() {
        let ports = vec![usb_port("other", 0x239a, 0x8029)];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Err(SerialDfuError::DeviceMissing)
        ));
    }

    #[test]
    fn doctor_reports_the_observed_recovery_identity() {
        let catalog = prns_flash_manifest::board_catalog().expect("catalog");
        let board = catalog.board("t1000-e").expect("T1000-E");
        let report = doctor(
            board,
            vec![recovery_bootloader_port("recovery")],
            Some("recovery"),
        )
        .expect("recovery doctor report");
        assert_eq!(
            report,
            DoctorReport {
                port_name: "recovery".to_string(),
                mode: DeviceMode::TouchApplicationOrBootloader,
                vendor_id: 0x239a,
                product_id: 0x8029,
            }
        );
    }

    #[test]
    fn canonical_and_recovery_devices_are_ambiguous_together() {
        let ports = vec![
            usb_port("canonical", 0x2886, 0x0057),
            recovery_bootloader_port("recovery"),
        ];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Err(SerialDfuError::AmbiguousDevices { .. })
        ));
    }

    #[test]
    fn explicit_port_must_have_an_exact_target_identity() {
        let ports = vec![usb_port("other", 0x1234, 0x5678)];
        assert!(matches!(
            select_device(&ports, &[], Some("other"), &transport()),
            Err(SerialDfuError::RequestedPortIdentity {
                vendor_id: 0x1234,
                product_id: 0x5678,
                ..
            })
        ));
    }

    #[test]
    fn ambiguous_devices_require_an_explicit_port() {
        let ports = vec![
            usb_port("first", 0x2886, 0x0057),
            usb_port("second", 0x2886, 0x0057),
        ];
        assert!(matches!(
            select_device(&ports, &[], None, &transport()),
            Err(SerialDfuError::AmbiguousDevices { .. })
        ));
    }

    #[test]
    fn reliable_transfer_retries_the_identical_frame() {
        let firmware = [0x5a; 513];
        let mut io = FakeDfuIo {
            writes: Vec::new(),
            replies: VecDeque::from([Reply::Timeout, Reply::Expected]),
            cancel_after_write: None,
        };
        let mut progress = Vec::new();
        run_transfer(
            &mut io,
            DfuTransfer::new(image(&firmware), DfuBankLayout::Single),
            |_| Ok(()),
            &|| false,
            |written, total| progress.push((written, total)),
        )
        .expect("transfer");
        assert_eq!(io.writes[0], io.writes[1]);
        assert_eq!(progress.last(), Some(&(firmware.len(), firmware.len())));
    }

    #[test]
    fn reliable_transfer_stops_after_the_bounded_attempt_count() {
        let firmware = [0x5a; 1];
        let mut io = FakeDfuIo {
            writes: Vec::new(),
            replies: VecDeque::from([Reply::Timeout, Reply::Timeout, Reply::Timeout]),
            cancel_after_write: None,
        };
        assert!(matches!(
            run_transfer(
                &mut io,
                DfuTransfer::new(image(&firmware), DfuBankLayout::Single),
                |_| Ok(()),
                &|| false,
                |_, _| {},
            ),
            Err(SerialDfuError::ReliableFrame { attempts: 3, .. })
        ));
        assert_eq!(io.writes.len(), usize::from(RELIABLE_FRAME_ATTEMPT_LIMIT));
        assert!(io.writes.windows(2).all(|frames| frames[0] == frames[1]));
    }

    #[test]
    fn cancellation_after_a_write_never_reports_transfer_success() {
        let firmware = [0x5a; 1];
        let cancelled = Rc::new(Cell::new(false));
        let mut io = FakeDfuIo {
            writes: Vec::new(),
            replies: VecDeque::new(),
            cancel_after_write: Some(Rc::clone(&cancelled)),
        };
        assert!(matches!(
            run_transfer(
                &mut io,
                DfuTransfer::new(image(&firmware), DfuBankLayout::Single),
                |_| Ok(()),
                &|| cancelled.get(),
                |_, _| {},
            ),
            Err(SerialDfuError::Cancelled)
        ));
        assert_eq!(io.writes.len(), 1);
    }
}
