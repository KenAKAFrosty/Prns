use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use espflash::connection::{ResetAfterOperation, ResetBeforeOperation};
use espflash::target::Chip;
use prns_flash_manifest::{provisioning_image, BoardCatalogEntry, ProvisioningAction};
use serialport::{SerialPortInfo, SerialPortType};

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedEspTarget;

mod session;

use session::{
    ChipSelection, DeviceIdentity, EspSession, EspflashSession, SessionError, SessionMode,
    SparsePart,
};

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
        Err(AppError::host_preflight(format!(
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
    target: &PreparedEspTarget,
    provisioning: &ProvisioningAction,
    port_name: Option<&str>,
    monitor: bool,
    reporter: Reporter,
) -> Result<(), AppError> {
    let selected = select_port(port_name)?;
    let expected = expected_device(board)?;
    let plan = sparse_plan(board, target, provisioning)?;
    let total = plan.iter().map(|part| part.bytes.len() as u64).sum::<u64>();

    reporter.phase(
        Phase::RequestingPort,
        Some(&board.slug),
        &format!("Opening {}…", selected.port_name),
    );
    let mut session = real_session(board, selected, SessionMode::Flash)?;
    run_flash_session(&mut session, expected, &plan, reporter, &cancelled)?;

    if monitor {
        monitor_port(session.port_name(), reporter)?;
    }
    if cancelled() {
        return Err(AppError::Cancelled);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorReport {
    pub(crate) port_name: String,
    pub(crate) detected_chip: String,
    pub(crate) flash_size: u32,
}

pub(crate) fn doctor(
    board: &BoardCatalogEntry,
    ports: Vec<SerialPortInfo>,
    port_name: Option<&str>,
) -> Result<DoctorReport, AppError> {
    let selected = select_port_from(ports, port_name)?;
    let selected_name = selected.port_name.clone();
    let expected = expected_device(board)?;
    // Doctor uses the ROM loader directly. It neither writes flash nor uploads
    // the RAM flashing stub used by the actual compressed flash operation.
    let mut session = real_session(board, selected, SessionMode::Inspect)?;
    let identity = run_doctor_session(&mut session, expected, &cancelled)?;
    if cancelled() {
        return Err(AppError::Cancelled);
    }
    Ok(DoctorReport {
        port_name: selected_name,
        detected_chip: identity.chip.to_string(),
        flash_size: identity.flash_size.ok_or_else(|| {
            AppError::device_identity("the device did not report a verifiable flash capacity")
        })?,
    })
}

#[derive(Clone, Copy)]
struct ExpectedDevice<'a> {
    board_slug: &'a str,
    chip: Chip,
    flash_size: Option<u32>,
}

fn expected_device(board: &BoardCatalogEntry) -> Result<ExpectedDevice<'_>, AppError> {
    let chip_name = board
        .expected_chip
        .as_deref()
        .ok_or_else(|| AppError::trust_manifest("ESP board is missing expected-chip metadata"))?;
    let chip = chip_name.parse::<Chip>().map_err(|error| {
        AppError::trust_manifest(format!("invalid expected chip {chip_name:?}: {error}"))
    })?;
    if !matches!(board.build, prns_flash_manifest::BoardBuild::Esp(_)) {
        return Err(AppError::unsupported_operation(
            "UF2 board cannot use the ESP engine",
        ));
    }
    Ok(ExpectedDevice {
        board_slug: &board.slug,
        chip,
        flash_size: board.flash_size,
    })
}

fn real_session(
    board: &BoardCatalogEntry,
    selected: SerialPortInfo,
    mode: SessionMode,
) -> Result<EspflashSession, AppError> {
    let build = match &board.build {
        prns_flash_manifest::BoardBuild::Esp(build) => build,
        prns_flash_manifest::BoardBuild::Uf2(_) => {
            return Err(AppError::unsupported_operation(
                "UF2 board cannot use the ESP engine",
            ));
        }
    };
    Ok(EspflashSession::new(
        selected,
        after_reset(&build.after_reset)?,
        before_reset(&build.before_reset)?,
        mode,
    ))
}

fn sparse_plan(
    board: &BoardCatalogEntry,
    target: &PreparedEspTarget,
    provisioning: &ProvisioningAction,
) -> Result<Vec<SparsePart>, AppError> {
    if matches!(provisioning, ProvisioningAction::ConfigureWithTcp { .. })
        && !target.supports_tcp_client_provisioning()
    {
        return Err(AppError::configuration(
            "this signed firmware release does not support TCP client provisioning",
        ));
    }
    let mut plan = target
        .parts()
        .iter()
        .map(|part| SparsePart {
            offset: part.offset(),
            bytes: part.bytes().to_vec(),
        })
        .collect::<Vec<_>>();
    if let Some(config) = provisioning_image(provisioning)
        .map_err(|error| AppError::configuration(error.to_string()))?
    {
        let slot = board
            .provisioning
            .as_ref()
            .ok_or_else(|| AppError::configuration("this board has no provisioning slot"))?;
        plan.push(SparsePart {
            offset: slot.offset,
            bytes: config,
        });
    }
    plan.sort_by_key(|part| part.offset);
    if plan.is_empty() {
        return Err(AppError::trust_manifest("ESP sparse flash plan is empty"));
    }
    Ok(plan)
}

fn run_flash_session(
    session: &mut dyn EspSession,
    expected: ExpectedDevice<'_>,
    plan: &[SparsePart],
    reporter: Reporter,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<DeviceIdentity, AppError> {
    let result = (|| {
        reporter.phase(
            Phase::Connecting,
            Some(expected.board_slug),
            "Connecting to the Espressif bootloader…",
        );
        session
            .connect(ChipSelection::Expected(expected.chip))
            .map_err(map_preflight_session_error)?;
        let identity = session.identity().map_err(map_preflight_session_error)?;
        let flash_capacity = identity
            .flash_size
            .map(|size| format!("{size} bytes of flash"))
            .unwrap_or_else(|| "an unreported flash capacity".to_string());
        reporter.phase(
            Phase::VerifyingTarget,
            Some(expected.board_slug),
            &format!("Detected {} with {flash_capacity}.", identity.chip),
        );
        validate_device_identity(expected.chip, expected.flash_size, identity)?;
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let total = plan.iter().map(|part| part.bytes.len() as u64).sum::<u64>();
        reporter.phase(
            Phase::Writing,
            Some(expected.board_slug),
            &format!("Writing and verifying {total} bytes without a full-chip erase…"),
        );
        session
            .write_and_verify(plan, expected.board_slug, reporter, is_cancelled)
            .map_err(map_flash_session_error)?;
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        reporter.phase(
            Phase::Resetting,
            Some(expected.board_slug),
            "Every sparse part verified; resetting the device…",
        );
        session.reset().map_err(map_flash_session_error)?;
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        Ok(identity)
    })();
    session.disconnect();
    result
}

fn run_doctor_session(
    session: &mut dyn EspSession,
    expected: ExpectedDevice<'_>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<DeviceIdentity, AppError> {
    let mut connected = false;
    let inspection = (|| {
        session
            .connect(ChipSelection::Detect)
            .map_err(map_preflight_session_error)?;
        connected = true;
        let identity = session.identity().map_err(map_preflight_session_error)?;
        validate_device_identity(expected.chip, expected.flash_size, identity)?;
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        Ok(identity)
    })();
    let reset = if connected {
        session.reset().map_err(map_preflight_session_error)
    } else {
        Ok(())
    };
    session.disconnect();
    inspection.and_then(|identity| {
        reset?;
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        Ok(identity)
    })
}

fn map_preflight_session_error(error: SessionError) -> AppError {
    match error {
        SessionError::Cancelled => AppError::Cancelled,
        SessionError::Connect(message) => {
            AppError::serial_port(format!("could not connect to the bootloader: {message}"))
        }
        SessionError::Identity(message) => {
            AppError::device_identity(format!("could not identify the device: {message}"))
        }
        SessionError::DeviceLost(message) => {
            AppError::device_identity(format!("device connection was lost: {message}"))
        }
        SessionError::Write(message)
        | SessionError::Verify(message)
        | SessionError::Reset(message) => AppError::device_identity(format!(
            "unexpected write-capable failure during non-writing preflight: {message}"
        )),
    }
}

fn map_flash_session_error(error: SessionError) -> AppError {
    match error {
        SessionError::Connect(message) => {
            AppError::serial_port(format!("could not connect to the bootloader: {message}"))
        }
        SessionError::Identity(message) => {
            AppError::device_identity(format!("could not identify the device: {message}"))
        }
        SessionError::Cancelled => AppError::Cancelled,
        SessionError::Write(message) => AppError::write(format!("sparse write failed: {message}")),
        SessionError::Verify(message) => {
            AppError::verify(format!("device-side verification failed: {message}"))
        }
        SessionError::Reset(message) => {
            AppError::reset(format!("could not reset device: {message}"))
        }
        SessionError::DeviceLost(message) => {
            AppError::device_lost(format!("device connection was lost: {message}"))
        }
    }
}

pub(crate) fn diagnostic_ports() -> Result<Vec<SerialPortInfo>, AppError> {
    serialport::available_ports().map_err(|error| {
        AppError::serial_port(format!("could not enumerate serial ports: {error}"))
    })
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
                AppError::serial_port(format!("serial port {requested:?} was not found"))
            });
    }
    let mut candidates = ports
        .into_iter()
        .filter(is_likely_device_port)
        .collect::<Vec<_>>();
    match candidates.len() {
        0 => Err(AppError::serial_port(
            "no usable serial device was found; connect the board with a USB data cable",
        )),
        1 => Ok(candidates.remove(0)),
        _ => Err(AppError::serial_port(format!(
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
    expected_flash_size: Option<u32>,
    identity: DeviceIdentity,
) -> Result<(), AppError> {
    if identity.chip != expected_chip {
        return Err(AppError::device_identity(format!(
            "wrong chip: expected {expected_chip}, detected {}",
            identity.chip
        )));
    }
    if identity.secure_download_mode {
        return Err(AppError::device_identity(
            "secure download mode prevents the required device-side verification",
        ));
    }
    match (expected_flash_size, identity.flash_size) {
        (Some(expected), Some(detected)) if expected == detected => Ok(()),
        (Some(expected), Some(detected)) => Err(AppError::device_identity(format!(
            "flash capacity mismatch: board catalog requires {expected} bytes, device reports {detected} bytes"
        ))),
        (Some(_), None) => Err(AppError::device_identity(
            "the device did not report a verifiable flash capacity",
        )),
        _ => Err(AppError::trust_catalog(
            "ESP board catalog is missing its flash capacity",
        )),
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

fn before_reset(value: &str) -> Result<ResetBeforeOperation, AppError> {
    match value {
        "default-reset" => Ok(ResetBeforeOperation::DefaultReset),
        "usb-reset" => Ok(ResetBeforeOperation::UsbReset),
        _ => Err(AppError::trust_catalog(format!(
            "unsupported before-reset mode {value:?}"
        ))),
    }
}

fn after_reset(value: &str) -> Result<ResetAfterOperation, AppError> {
    match value {
        "hard-reset" => Ok(ResetAfterOperation::HardReset),
        "watchdog-reset" => Ok(ResetAfterOperation::WatchdogReset),
        _ => Err(AppError::trust_catalog(format!(
            "unsupported after-reset mode {value:?}"
        ))),
    }
}

fn monitor_port(port_name: &str, reporter: Reporter) -> Result<(), AppError> {
    if cancelled() {
        return Err(AppError::Cancelled);
    }
    reporter.phase(
        Phase::Monitor,
        None,
        "Serial monitor active at 115200 baud; press Ctrl-C to close it.",
    );
    let mut port = reopen_monitor_port(
        port_name,
        &cancelled,
        || {
            serialport::new(port_name, 115_200)
                .timeout(Duration::from_millis(250))
                .open()
                .ok()
        },
        || std::thread::sleep(Duration::from_millis(250)),
    )?;
    stream_monitor(&mut *port, &cancelled, |bytes| {
        io::stdout()
            .write_all(bytes)
            .and_then(|_| io::stdout().flush())
    })
}

fn stream_monitor<R: Read + ?Sized>(
    port: &mut R,
    is_cancelled: &dyn Fn() -> bool,
    mut write_output: impl FnMut(&[u8]) -> io::Result<()>,
) -> Result<(), AppError> {
    let mut buffer = [0u8; 1024];
    loop {
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                write_output(&buffer[..count]).map_err(|error| {
                    AppError::monitor(format!("monitor output failed: {error}"))
                })?;
                if is_cancelled() {
                    return Err(AppError::Cancelled);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                if is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                return Err(AppError::monitor(format!(
                    "serial monitor disconnected: {error}"
                )));
            }
        }
    }
}

fn reopen_monitor_port<T>(
    port_name: &str,
    is_cancelled: &dyn Fn() -> bool,
    mut open: impl FnMut() -> Option<T>,
    mut wait: impl FnMut(),
) -> Result<T, AppError> {
    for _ in 0..20 {
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if let Some(port) = open() {
            if is_cancelled() {
                return Err(AppError::Cancelled);
            }
            return Ok(port);
        }
        if is_cancelled() {
            return Err(AppError::Cancelled);
        }
        wait();
    }
    if is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Err(AppError::monitor(format!(
            "could not reopen {port_name} for monitoring"
        )))
    }
}

#[cfg(test)]
mod port_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum InjectFailure {
        None,
        Connect,
        WrongChip,
        Write(usize),
        Verify(usize),
        Cancel(usize),
        DeviceLoss(usize),
        Reset,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum FakeCall {
        Connect(ChipSelection),
        Identity,
        Write(usize),
        Verify(usize),
        CancelBoundary(usize),
        Reset,
        Disconnect,
    }

    struct FakeSession {
        calls: Vec<FakeCall>,
        failure: InjectFailure,
        identity: DeviceIdentity,
        cancel_on_reset: Option<Rc<Cell<bool>>>,
    }

    impl FakeSession {
        fn new(failure: InjectFailure) -> Self {
            Self {
                calls: Vec::new(),
                failure,
                identity: matching_identity(),
                cancel_on_reset: None,
            }
        }

        fn cancelling_during_reset(signal: Rc<Cell<bool>>) -> Self {
            Self {
                cancel_on_reset: Some(signal),
                ..Self::new(InjectFailure::None)
            }
        }
    }

    impl EspSession for FakeSession {
        fn connect(&mut self, chip: ChipSelection) -> Result<(), SessionError> {
            self.calls.push(FakeCall::Connect(chip));
            if self.failure == InjectFailure::Connect {
                Err(SessionError::Connect(
                    "injected connect failure".to_string(),
                ))
            } else {
                Ok(())
            }
        }

        fn identity(&mut self) -> Result<DeviceIdentity, SessionError> {
            self.calls.push(FakeCall::Identity);
            if self.failure == InjectFailure::WrongChip {
                Ok(DeviceIdentity {
                    chip: Chip::Esp32c6,
                    ..self.identity
                })
            } else {
                Ok(self.identity)
            }
        }

        fn write_and_verify(
            &mut self,
            parts: &[SparsePart],
            _board_slug: &str,
            _reporter: Reporter,
            _is_cancelled: &dyn Fn() -> bool,
        ) -> Result<(), SessionError> {
            for (index, _) in parts.iter().enumerate() {
                if self.failure == InjectFailure::Cancel(index) {
                    self.calls.push(FakeCall::CancelBoundary(index));
                    return Err(SessionError::Cancelled);
                }
                self.calls.push(FakeCall::Write(index));
                match self.failure {
                    InjectFailure::Write(failed) if failed == index => {
                        return Err(SessionError::Write("injected write failure".to_string()));
                    }
                    InjectFailure::DeviceLoss(failed) if failed == index => {
                        return Err(SessionError::DeviceLost("injected disconnect".to_string()));
                    }
                    _ => {}
                }
                self.calls.push(FakeCall::Verify(index));
                if self.failure == InjectFailure::Verify(index) {
                    return Err(SessionError::Verify(
                        "injected verification mismatch".to_string(),
                    ));
                }
            }
            Ok(())
        }

        fn reset(&mut self) -> Result<(), SessionError> {
            self.calls.push(FakeCall::Reset);
            if let Some(signal) = &self.cancel_on_reset {
                signal.set(true);
            }
            if self.failure == InjectFailure::Reset {
                Err(SessionError::Reset("injected reset failure".to_string()))
            } else {
                Ok(())
            }
        }

        fn disconnect(&mut self) {
            self.calls.push(FakeCall::Disconnect);
        }
    }

    fn matching_identity() -> DeviceIdentity {
        DeviceIdentity {
            chip: Chip::Esp32s3,
            flash_size: Some(8 * 1024 * 1024),
            secure_download_mode: false,
        }
    }

    fn expected() -> ExpectedDevice<'static> {
        ExpectedDevice {
            board_slug: "heltec-v4",
            chip: Chip::Esp32s3,
            flash_size: Some(8 * 1024 * 1024),
        }
    }

    fn sparse_test_plan() -> Vec<SparsePart> {
        vec![
            SparsePart {
                offset: 0,
                bytes: vec![1],
            },
            SparsePart {
                offset: 0x8000,
                bytes: vec![2],
            },
            SparsePart {
                offset: 0x10000,
                bytes: vec![3],
            },
        ]
    }

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
                Some(8 * 1024 * 1024),
                DeviceIdentity {
                    chip: Chip::Esp32c6,
                    flash_size: Some(4 * 1024 * 1024),
                    secure_download_mode: false,
                },
            ),
            Err(AppError::Preflight(_))
        ));
        assert!(matches!(
            validate_device_identity(
                Chip::Esp32s3,
                Some(8 * 1024 * 1024),
                DeviceIdentity {
                    flash_size: None,
                    ..matching_identity()
                },
            ),
            Err(AppError::Preflight(_))
        ));
        assert!(matches!(
            validate_device_identity(
                Chip::Esp32s3,
                Some(8 * 1024 * 1024),
                DeviceIdentity {
                    secure_download_mode: true,
                    ..matching_identity()
                },
            ),
            Err(AppError::Preflight(_))
        ));
    }

    #[test]
    fn successful_flash_verifies_every_part_before_reset_and_disconnect() {
        let mut session = FakeSession::new(InjectFailure::None);
        run_flash_session(
            &mut session,
            expected(),
            &sparse_test_plan(),
            Reporter::human(),
            &|| false,
        )
        .expect("fake flash succeeds");
        assert_eq!(
            session.calls,
            vec![
                FakeCall::Connect(ChipSelection::Expected(Chip::Esp32s3)),
                FakeCall::Identity,
                FakeCall::Write(0),
                FakeCall::Verify(0),
                FakeCall::Write(1),
                FakeCall::Verify(1),
                FakeCall::Write(2),
                FakeCall::Verify(2),
                FakeCall::Reset,
                FakeCall::Disconnect,
            ]
        );
    }

    #[test]
    fn every_flash_failure_disconnects_without_an_unsafe_reset() {
        let cases = [
            (InjectFailure::Connect, "preflight"),
            (InjectFailure::WrongChip, "preflight"),
            (InjectFailure::Write(0), "flash"),
            (InjectFailure::Verify(0), "flash"),
            (InjectFailure::Cancel(1), "cancel"),
            (InjectFailure::DeviceLoss(0), "flash"),
        ];
        for (failure, category) in cases {
            let mut session = FakeSession::new(failure);
            let result = run_flash_session(
                &mut session,
                expected(),
                &sparse_test_plan(),
                Reporter::human(),
                &|| false,
            );
            match category {
                "preflight" => assert!(matches!(result, Err(AppError::Preflight(_)))),
                "flash" => assert!(matches!(result, Err(AppError::WriteVerifyReset(_)))),
                "cancel" => assert!(matches!(result, Err(AppError::Cancelled))),
                _ => unreachable!(),
            }
            assert_eq!(session.calls.last(), Some(&FakeCall::Disconnect));
            assert_eq!(
                session
                    .calls
                    .iter()
                    .filter(|call| matches!(call, FakeCall::Disconnect))
                    .count(),
                1
            );
            assert!(!session.calls.contains(&FakeCall::Reset));
        }
    }

    #[test]
    fn reset_failure_is_terminal_and_still_disconnects_once() {
        let mut session = FakeSession::new(InjectFailure::Reset);
        let result = run_flash_session(
            &mut session,
            expected(),
            &sparse_test_plan(),
            Reporter::human(),
            &|| false,
        );
        assert!(matches!(
            result,
            Err(AppError::WriteVerifyReset(message)) if message.to_string().contains("reset")
        ));
        assert_eq!(
            session.calls[session.calls.len() - 2..],
            [FakeCall::Reset, FakeCall::Disconnect]
        );
    }

    #[test]
    fn cancellation_during_final_flash_reset_never_reports_success() {
        let cancellation = Rc::new(Cell::new(false));
        let mut session = FakeSession::cancelling_during_reset(Rc::clone(&cancellation));
        let result = run_flash_session(
            &mut session,
            expected(),
            &sparse_test_plan(),
            Reporter::human(),
            &|| cancellation.get(),
        );

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(
            session.calls[session.calls.len() - 2..],
            [FakeCall::Reset, FakeCall::Disconnect]
        );
    }

    #[test]
    fn retry_always_restarts_the_complete_sparse_plan() {
        let plan = sparse_test_plan();
        let mut failed = FakeSession::new(InjectFailure::Verify(1));
        assert!(
            run_flash_session(&mut failed, expected(), &plan, Reporter::human(), &|| false,)
                .is_err()
        );

        let mut retry = FakeSession::new(InjectFailure::None);
        run_flash_session(&mut retry, expected(), &plan, Reporter::human(), &|| false)
            .expect("full retry succeeds");
        assert_eq!(retry.calls.get(2), Some(&FakeCall::Write(0)));
        assert_eq!(
            retry
                .calls
                .iter()
                .filter(|call| matches!(call, FakeCall::Write(_)))
                .count(),
            plan.len()
        );
    }

    #[test]
    fn doctor_is_non_writing_and_restores_then_disconnects_the_session() {
        let mut successful = FakeSession::new(InjectFailure::None);
        run_doctor_session(&mut successful, expected(), &|| false)
            .expect("doctor preflight succeeds");
        assert_eq!(
            successful.calls,
            vec![
                FakeCall::Connect(ChipSelection::Detect),
                FakeCall::Identity,
                FakeCall::Reset,
                FakeCall::Disconnect,
            ]
        );

        let mut wrong_chip = FakeSession::new(InjectFailure::WrongChip);
        assert!(matches!(
            run_doctor_session(&mut wrong_chip, expected(), &|| false),
            Err(AppError::Preflight(_))
        ));
        assert_eq!(
            wrong_chip.calls,
            vec![
                FakeCall::Connect(ChipSelection::Detect),
                FakeCall::Identity,
                FakeCall::Reset,
                FakeCall::Disconnect,
            ]
        );
    }

    #[test]
    fn cancellation_during_doctor_reset_is_not_reported_as_success() {
        let cancellation = Rc::new(Cell::new(false));
        let mut session = FakeSession::cancelling_during_reset(Rc::clone(&cancellation));
        let result = run_doctor_session(&mut session, expected(), &|| cancellation.get());

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(
            session.calls,
            vec![
                FakeCall::Connect(ChipSelection::Detect),
                FakeCall::Identity,
                FakeCall::Reset,
                FakeCall::Disconnect,
            ]
        );
    }

    #[test]
    fn monitor_reopen_stops_on_cancellation_instead_of_falling_through() {
        let cancellation = Cell::new(false);
        let attempts = Cell::new(0_u8);
        let result = reopen_monitor_port(
            "fake-port",
            &|| cancellation.get(),
            || {
                attempts.set(attempts.get() + 1);
                None::<()>
            },
            || cancellation.set(true),
        );

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn monitor_read_cancellation_is_exit_130_not_success() {
        struct CancelOnRead {
            cancellation: Rc<Cell<bool>>,
            reads: Rc<Cell<u8>>,
        }

        impl Read for CancelOnRead {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                self.reads.set(self.reads.get() + 1);
                self.cancellation.set(true);
                Err(io::Error::new(io::ErrorKind::TimedOut, "injected timeout"))
            }
        }

        let cancellation = Rc::new(Cell::new(false));
        let reads = Rc::new(Cell::new(0_u8));
        let mut port = CancelOnRead {
            cancellation: Rc::clone(&cancellation),
            reads: Rc::clone(&reads),
        };
        let result = stream_monitor(&mut port, &|| cancellation.get(), |_| Ok(()));

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(reads.get(), 1);
        assert_eq!(AppError::Cancelled.code(), 130);
    }
}
