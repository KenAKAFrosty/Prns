use std::borrow::Cow;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::{Flasher, SpiAttachParams};
use espflash::image_format::Segment;
use espflash::target::{Chip, ProgressCallbacks};
use prns_flash_manifest::{provisioning_image, BoardCatalogEntry, ProvisioningAction};
use serialport::{FlowControl, SerialPortInfo, SerialPortType, UsbPortInfo};

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedPart;

static CANCELLED: AtomicBool = AtomicBool::new(false);
static CANCEL_HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn begin_cancellable_operation() -> Result<(), AppError> {
    let result = CANCEL_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| {
            CANCELLED.store(true, Ordering::SeqCst);
        })
        .map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        Err(AppError::preflight(format!(
            "could not install cancellation handler: {error}"
        )))
    } else {
        CANCELLED.store(false, Ordering::SeqCst);
        Ok(())
    }
}

pub(crate) fn cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

pub(crate) fn flash(
    board: &BoardCatalogEntry,
    parts: &[PreparedPart],
    provisioning: &ProvisioningAction,
    port_name: Option<&str>,
    monitor: bool,
    reporter: Reporter,
) -> Result<(), AppError> {
    let selected = select_port(port_name)?;
    let chip_name = board
        .expected_chip
        .as_deref()
        .ok_or_else(|| AppError::usage("ESP board is missing expected-chip metadata"))?;
    let chip = chip_name.parse::<Chip>().map_err(|error| {
        AppError::trust(format!("invalid expected chip {chip_name:?}: {error}"))
    })?;
    let build = match &board.build {
        prns_flash_manifest::BoardBuild::Esp(build) => build,
        prns_flash_manifest::BoardBuild::Uf2(_) => {
            return Err(AppError::usage("UF2 board cannot use the ESP engine"));
        }
    };

    reporter.phase(
        Phase::RequestingPort,
        Some(&board.slug),
        &format!("Opening {}…", selected.port_name),
    );
    let serial = serialport::new(&selected.port_name, 115_200)
        .flow_control(FlowControl::None)
        .timeout(Duration::from_secs(3))
        .open_native()
        .map_err(|error| {
            AppError::preflight(format!(
                "could not open serial port {}: {error}",
                selected.port_name
            ))
        })?;
    let usb = usb_info(&selected);
    let connection = Connection::new(
        serial,
        usb,
        after_reset(&build.after_reset)?,
        before_reset(&build.before_reset)?,
        921_600,
    );
    reporter.phase(
        Phase::Connecting,
        Some(&board.slug),
        "Connecting to the Espressif bootloader…",
    );
    let mut flasher = Flasher::connect(connection, true, true, false, Some(chip), Some(921_600))
        .map_err(|error| {
            AppError::preflight(format!("could not connect to {chip_name}: {error}"))
        })?;
    let detected_chip = flasher.chip();
    if flasher.secure_download_mode() {
        return Err(AppError::preflight(
            "secure download mode prevents the required device-side verification",
        ));
    }
    let detected_flash_size = flasher
        .flash_detect()
        .map_err(|error| AppError::preflight(format!("could not detect flash capacity: {error}")))?
        .map(|detected| detected.size());
    validate_device_identity(chip, detected_chip, board.flash_size, detected_flash_size)?;
    if cancelled() {
        return Err(AppError::Cancelled);
    }

    let mut owned = parts
        .iter()
        .map(|part| {
            let offset = part.descriptor.offset.ok_or_else(|| {
                AppError::trust(format!("ESP part {:?} has no offset", part.descriptor.path))
            })?;
            Ok((offset, part.bytes.clone()))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    if let Some(config) =
        provisioning_image(provisioning).map_err(|error| AppError::usage(error.to_string()))?
    {
        let slot = board
            .provisioning
            .as_ref()
            .ok_or_else(|| AppError::usage("this board has no provisioning slot"))?;
        owned.push((slot.offset, config));
    }
    owned.sort_by_key(|(offset, _)| *offset);
    let total = owned.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    reporter.phase(
        Phase::Writing,
        Some(&board.slug),
        &format!("Writing and verifying {total} bytes without a full-chip erase…"),
    );
    let mut progress = FlashProgress {
        reporter,
        board: &board.slug,
        completed_bytes: 0,
        part_bytes: 0,
        part_blocks: 0,
        operation_total: total,
    };
    let mut target = chip.flash_target(SpiAttachParams::default(), true, true, false);
    target
        .begin(flasher.connection())
        .map_err(|error| AppError::flash(format!("could not begin sparse flash: {error}")))?;
    for (offset, bytes) in &owned {
        if cancelled() {
            return Err(AppError::Cancelled);
        }
        progress.part_bytes = bytes.len() as u64;
        target
            .write_segment(
                flasher.connection(),
                Segment {
                    addr: *offset,
                    data: Cow::Borrowed(bytes.as_slice()),
                },
                &mut progress,
            )
            .map_err(map_part_error)?;
        if cancelled() {
            return Err(AppError::Cancelled);
        }
    }
    target.finish(flasher.connection(), true).map_err(|error| {
        AppError::flash(format!("final reset failed after verification: {error}"))
    })?;
    drop(flasher);

    if monitor {
        monitor_port(&selected.port_name, reporter)?;
    }
    reporter.success(
        &board.slug,
        &format!(
            "Verified flash complete for {} ({total} bytes).",
            board.display_name
        ),
    );
    Ok(())
}

pub(crate) fn diagnostic_ports() -> Result<Vec<SerialPortInfo>, AppError> {
    serialport::available_ports()
        .map_err(|error| AppError::preflight(format!("could not enumerate serial ports: {error}")))
}

fn select_port(requested: Option<&str>) -> Result<SerialPortInfo, AppError> {
    select_port_from(diagnostic_ports()?, requested)
}

fn select_port_from(
    ports: Vec<SerialPortInfo>,
    requested: Option<&str>,
) -> Result<SerialPortInfo, AppError> {
    if let Some(requested) = requested {
        return ports
            .into_iter()
            .find(|port| port.port_name == requested)
            .ok_or_else(|| {
                AppError::preflight(format!("serial port {requested:?} was not found"))
            });
    }
    let mut candidates = ports
        .into_iter()
        .filter(is_likely_device_port)
        .collect::<Vec<_>>();
    match candidates.len() {
        0 => Err(AppError::preflight(
            "no usable serial device was found; connect the board with a USB data cable",
        )),
        1 => Ok(candidates.remove(0)),
        _ => Err(AppError::preflight(format!(
            "multiple serial devices are present ({}); rerun with --port",
            candidates
                .iter()
                .map(|port| port.port_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn validate_device_identity(
    expected_chip: Chip,
    detected_chip: Chip,
    expected_flash_size: Option<u32>,
    detected_flash_size: Option<u32>,
) -> Result<(), AppError> {
    if detected_chip != expected_chip {
        return Err(AppError::preflight(format!(
            "wrong chip: expected {expected_chip}, detected {detected_chip}"
        )));
    }
    match (expected_flash_size, detected_flash_size) {
        (Some(expected), Some(detected)) if expected == detected => Ok(()),
        (Some(expected), Some(detected)) => Err(AppError::preflight(format!(
            "flash capacity mismatch: board catalog requires {expected} bytes, device reports {detected} bytes"
        ))),
        (Some(_), None) => Err(AppError::preflight(
            "the device did not report a verifiable flash capacity",
        )),
        _ => Err(AppError::trust(
            "ESP board catalog is missing its flash capacity",
        )),
    }
}

fn map_part_error(error: espflash::Error) -> AppError {
    match error {
        espflash::Error::VerifyFailed | espflash::Error::DigestMismatch(_, _) => {
            AppError::flash(format!("device-side flash verification failed: {error}"))
        }
        _ => AppError::flash(format!("ESP part write failed: {error}")),
    }
}

fn is_likely_device_port(port: &SerialPortInfo) -> bool {
    if matches!(port.port_type, SerialPortType::UsbPort(_)) {
        return true;
    }
    let name = port.port_name.to_ascii_lowercase();
    name.contains("ttyacm")
        || name.contains("ttyusb")
        || name.contains("usbmodem")
        || name.contains("usbserial")
        || (cfg!(windows)
            && name.strip_prefix("com").is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            }))
}

fn usb_info(port: &SerialPortInfo) -> UsbPortInfo {
    match &port.port_type {
        SerialPortType::UsbPort(info) => info.clone(),
        _ => UsbPortInfo {
            vid: 0,
            pid: 0,
            serial_number: None,
            manufacturer: None,
            product: None,
        },
    }
}

fn before_reset(value: &str) -> Result<ResetBeforeOperation, AppError> {
    match value {
        "default-reset" => Ok(ResetBeforeOperation::DefaultReset),
        "usb-reset" => Ok(ResetBeforeOperation::UsbReset),
        _ => Err(AppError::trust(format!(
            "unsupported before-reset mode {value:?}"
        ))),
    }
}

fn after_reset(value: &str) -> Result<ResetAfterOperation, AppError> {
    match value {
        "hard-reset" => Ok(ResetAfterOperation::HardReset),
        "watchdog-reset" => Ok(ResetAfterOperation::WatchdogReset),
        _ => Err(AppError::trust(format!(
            "unsupported after-reset mode {value:?}"
        ))),
    }
}

fn monitor_port(port_name: &str, reporter: Reporter) -> Result<(), AppError> {
    CANCELLED.store(false, Ordering::SeqCst);
    reporter.phase(
        Phase::Monitor,
        None,
        "Serial monitor active at 115200 baud; press Ctrl-C to close it.",
    );
    let mut port = None;
    for _ in 0..20 {
        match serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(250))
            .open()
        {
            Ok(opened) => {
                port = Some(opened);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    let mut port = port.ok_or_else(|| {
        AppError::preflight(format!("could not reopen {port_name} for monitoring"))
    })?;
    let mut buffer = [0u8; 1024];
    while !CANCELLED.load(Ordering::SeqCst) {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                io::stdout()
                    .write_all(&buffer[..count])
                    .and_then(|_| io::stdout().flush())
                    .map_err(|error| {
                        AppError::preflight(format!("monitor output failed: {error}"))
                    })?;
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                return Err(AppError::preflight(format!(
                    "serial monitor disconnected: {error}"
                )));
            }
        }
    }
    Ok(())
}

struct FlashProgress<'a> {
    reporter: Reporter,
    board: &'a str,
    completed_bytes: u64,
    part_bytes: u64,
    part_blocks: u64,
    operation_total: u64,
}

impl ProgressCallbacks for FlashProgress<'_> {
    fn init(&mut self, _addr: u32, total: usize) {
        self.part_blocks = total as u64;
    }

    fn update(&mut self, current: usize) {
        let part_current = if self.part_blocks == 0 {
            0
        } else {
            self.part_bytes
                .saturating_mul(current as u64)
                .checked_div(self.part_blocks)
                .unwrap_or_default()
        };
        self.reporter.progress(
            Phase::Writing,
            Some(self.board),
            self.completed_bytes
                .saturating_add(part_current)
                .min(self.operation_total),
            self.operation_total,
        );
    }

    fn verifying(&mut self) {
        self.reporter.phase(
            Phase::VerifyingFlash,
            Some(self.board),
            "Verifying bytes on the device…",
        );
    }

    fn finish(&mut self, _skipped: bool) {
        // An already-matching segment is complete too; count it so total progress
        // remains monotonic and reaches 100% when espflash skips a write.
        self.completed_bytes = self.completed_bytes.saturating_add(self.part_bytes);
        self.reporter.progress(
            Phase::Writing,
            Some(self.board),
            self.completed_bytes.min(self.operation_total),
            self.operation_total,
        );
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;

    fn port(name: &str, port_type: SerialPortType) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.to_string(),
            port_type,
        }
    }

    #[test]
    fn filters_platform_debug_and_bluetooth_ports() {
        assert!(!is_likely_device_port(&port(
            "/dev/cu.debug-console",
            SerialPortType::PciPort,
        )));
        assert!(!is_likely_device_port(&port(
            "/dev/cu.Bluetooth-Incoming-Port",
            SerialPortType::BluetoothPort,
        )));
        assert!(is_likely_device_port(&port(
            "/dev/cu.usbmodem2101",
            SerialPortType::Unknown,
        )));
    }

    #[test]
    fn selection_requires_an_explicit_port_when_multiple_devices_exist() {
        let ports = vec![
            port("/dev/cu.usbmodem1", SerialPortType::Unknown),
            port("/dev/cu.usbmodem2", SerialPortType::Unknown),
        ];
        assert!(matches!(
            select_port_from(ports.clone(), None),
            Err(AppError::Preflight(_))
        ));
        assert_eq!(
            select_port_from(ports, Some("/dev/cu.usbmodem2"))
                .expect("explicit fake port")
                .port_name,
            "/dev/cu.usbmodem2"
        );
    }

    #[test]
    fn wrong_chip_and_unknown_flash_capacity_are_preflight_failures() {
        assert!(matches!(
            validate_device_identity(
                Chip::Esp32s3,
                Chip::Esp32c6,
                Some(8 * 1024 * 1024),
                Some(4 * 1024 * 1024),
            ),
            Err(AppError::Preflight(_))
        ));
        assert!(matches!(
            validate_device_identity(Chip::Esp32s3, Chip::Esp32s3, Some(8 * 1024 * 1024), None),
            Err(AppError::Preflight(_))
        ));
    }

    #[test]
    fn verification_and_write_failures_are_distinguished() {
        let verification = map_part_error(espflash::Error::VerifyFailed);
        let write = map_part_error(espflash::Error::FlashConnect);
        assert!(matches!(
            verification,
            AppError::Flash(message) if message.contains("verification")
        ));
        assert!(matches!(
            write,
            AppError::Flash(message) if message.contains("write")
        ));
    }
}
