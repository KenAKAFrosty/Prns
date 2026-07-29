use std::borrow::Cow;
use std::time::Duration;

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::{Flasher, SpiAttachParams};
use espflash::image_format::Segment;
use espflash::target::{Chip, ProgressCallbacks};
use serialport::{FlowControl, SerialPortInfo, SerialPortType, UsbPortInfo};
use thiserror::Error;

use crate::events::{Phase, Reporter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeviceIdentity {
    pub(super) chip: Chip,
    pub(super) flash_size: Option<u32>,
    pub(super) secure_download_mode: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SparsePart {
    pub(super) offset: u32,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionMode {
    Inspect,
    Flash,
}

impl SessionMode {
    const fn uses_stub(self) -> bool {
        matches!(self, Self::Flash)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChipSelection {
    /// Detect the chip without trusting a caller-provided identity. Doctor uses
    /// this path with the ROM loader and never uploads the flashing stub.
    Detect,
    /// Require this chip during the initial ROM handshake. Espflash performs
    /// this comparison before it uploads the RAM flashing stub.
    Expected(Chip),
}

#[derive(Debug, Error)]
pub(super) enum SessionError {
    #[error("could not connect to the Espressif bootloader: {0}")]
    Connect(String),
    #[error("could not identify the connected Espressif device: {0}")]
    Identity(String),
    #[error("ESP sparse write failed: {0}")]
    Write(String),
    #[error("device-side flash verification failed: {0}")]
    Verify(String),
    #[error("the ESP device connection was lost: {0}")]
    DeviceLost(String),
    #[error("could not reset the ESP device: {0}")]
    Reset(String),
    #[error("operation cancelled at a safe sparse-part boundary")]
    Cancelled,
}

pub(super) trait EspSession {
    fn connect(&mut self, chip: ChipSelection) -> Result<(), SessionError>;
    fn identity(&mut self) -> Result<DeviceIdentity, SessionError>;
    fn write_and_verify(
        &mut self,
        parts: &[SparsePart],
        board_slug: &str,
        reporter: Reporter,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(), SessionError>;
    fn reset(&mut self) -> Result<(), SessionError>;
    fn disconnect(&mut self);
}

pub(super) struct EspflashSession {
    port: SerialPortInfo,
    after_reset: ResetAfterOperation,
    before_reset: ResetBeforeOperation,
    mode: SessionMode,
    flasher: Option<Flasher>,
}

impl EspflashSession {
    pub(super) fn new(
        port: SerialPortInfo,
        after_reset: ResetAfterOperation,
        before_reset: ResetBeforeOperation,
        mode: SessionMode,
    ) -> Self {
        Self {
            port,
            after_reset,
            before_reset,
            mode,
            flasher: None,
        }
    }

    pub(super) fn port_name(&self) -> &str {
        &self.port.port_name
    }

    fn flasher(&mut self) -> Result<&mut Flasher, SessionError> {
        self.flasher
            .as_mut()
            .ok_or_else(|| SessionError::Connect("session is not connected".to_string()))
    }

    fn release(&mut self) {
        self.flasher.take();
    }
}

impl EspSession for EspflashSession {
    fn connect(&mut self, chip: ChipSelection) -> Result<(), SessionError> {
        if self.flasher.is_some() {
            return Err(SessionError::Connect(
                "session is already connected".to_string(),
            ));
        }
        let expected_chip = match (self.mode, chip) {
            (SessionMode::Inspect, ChipSelection::Detect) => None,
            (SessionMode::Flash, ChipSelection::Expected(chip)) => Some(chip),
            (SessionMode::Inspect, ChipSelection::Expected(_)) => {
                return Err(SessionError::Connect(
                    "doctor must autodetect the connected chip".to_string(),
                ));
            }
            (SessionMode::Flash, ChipSelection::Detect) => {
                return Err(SessionError::Connect(
                    "flashing requires the signed expected-chip identity".to_string(),
                ));
            }
        };
        let serial = serialport::new(&self.port.port_name, 115_200)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_secs(3))
            .open_native()
            .map_err(|error| {
                SessionError::Connect(format!(
                    "could not open serial port {}: {error}",
                    self.port.port_name
                ))
            })?;
        let connection = Connection::new(
            serial,
            usb_info(&self.port),
            self.after_reset,
            self.before_reset,
            921_600,
        );
        self.flasher = Some(
            match Flasher::try_connect(
                connection,
                self.mode.uses_stub(),
                true,
                false,
                expected_chip,
                Some(921_600),
            ) {
                Ok(flasher) => flasher,
                Err(error_and_connection) => {
                    let (error, mut connection) = *error_and_connection;
                    // try_connect returns ownership on every partial failure. Make
                    // one best-effort attempt to leave the device out of the ROM
                    // loader, then close the port. Cleanup must never replace the
                    // original diagnostic that explains why connection failed.
                    let original_error = error.to_string();
                    let _ = connection.reset();
                    drop(connection);
                    return Err(SessionError::Connect(original_error));
                }
            },
        );
        Ok(())
    }

    fn identity(&mut self) -> Result<DeviceIdentity, SessionError> {
        let flasher = self.flasher()?;
        let chip = flasher.chip();
        let secure_download_mode = flasher.secure_download_mode();
        let flash_size = flasher
            .flash_detect()
            .map_err(|error| SessionError::Identity(error.to_string()))?
            .map(|size| size.size());
        Ok(DeviceIdentity {
            chip,
            flash_size,
            secure_download_mode,
        })
    }

    fn write_and_verify(
        &mut self,
        parts: &[SparsePart],
        board_slug: &str,
        reporter: Reporter,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<(), SessionError> {
        let use_stub = self.mode.uses_stub();
        let flasher = self.flasher()?;
        let chip = flasher.chip();
        let total = parts
            .iter()
            .map(|part| part.bytes.len() as u64)
            .sum::<u64>();
        let mut progress = FlashProgress {
            reporter,
            board: board_slug,
            completed_bytes: 0,
            part_bytes: 0,
            part_blocks: 0,
            operation_total: total,
            reported_bytes: None,
        };
        let mut target = chip.flash_target(SpiAttachParams::default(), use_stub, true, false);
        target
            .begin(flasher.connection())
            .map_err(map_write_error)?;
        for part in parts {
            if is_cancelled() {
                return Err(SessionError::Cancelled);
            }
            progress.part_bytes = part.bytes.len() as u64;
            target
                .write_segment(
                    flasher.connection(),
                    Segment {
                        addr: part.offset,
                        data: Cow::Borrowed(&part.bytes),
                    },
                    &mut progress,
                )
                .map_err(map_write_error)?;
            if is_cancelled() {
                return Err(SessionError::Cancelled);
            }
        }
        // End flash mode without rebooting. Reset is a separate operation and
        // the orchestrator invokes it only after every segment returned from
        // espflash's device-side verification successfully.
        target
            .finish(flasher.connection(), false)
            .map_err(map_write_error)
    }

    fn reset(&mut self) -> Result<(), SessionError> {
        let use_stub = self.mode.uses_stub();
        let flasher = self.flasher()?;
        let chip = flasher.chip();
        flasher
            .connection()
            .reset_after(use_stub, chip)
            .map_err(|error| SessionError::Reset(error.to_string()))
    }

    fn disconnect(&mut self) {
        self.release();
    }
}

impl Drop for EspflashSession {
    fn drop(&mut self) {
        self.release();
    }
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

fn map_write_error(error: espflash::Error) -> SessionError {
    match error {
        espflash::Error::VerifyFailed | espflash::Error::DigestMismatch(_, _) => {
            SessionError::Verify(error.to_string())
        }
        espflash::Error::Cancelled => SessionError::Cancelled,
        espflash::Error::Connection(_)
        | espflash::Error::Flashing(_)
        | espflash::Error::IncorrectResponse
        | espflash::Error::CorruptData(_, _) => SessionError::DeviceLost(error.to_string()),
        _ => SessionError::Write(error.to_string()),
    }
}

struct FlashProgress<'a> {
    reporter: Reporter,
    board: &'a str,
    completed_bytes: u64,
    part_bytes: u64,
    part_blocks: u64,
    operation_total: u64,
    reported_bytes: Option<u64>,
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
        self.report_progress(
            self.completed_bytes
                .saturating_add(part_current)
                .min(self.operation_total),
        );
    }

    fn verifying(&mut self) {
        self.reporter.finish_progress();
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
        self.report_progress(self.completed_bytes.min(self.operation_total));
    }
}

impl FlashProgress<'_> {
    fn report_progress(&mut self, current: u64) {
        if self.reported_bytes == Some(current) {
            return;
        }
        self.reporter.progress(
            Phase::Writing,
            Some(self.board),
            current,
            self.operation_total,
        );
        self.reported_bytes = Some(current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inert_session(mode: SessionMode) -> EspflashSession {
        EspflashSession::new(
            SerialPortInfo {
                port_name: "must-not-be-opened".to_string(),
                port_type: SerialPortType::Unknown,
            },
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            mode,
        )
    }

    #[test]
    fn inspection_never_loads_the_ram_flashing_stub() {
        assert!(!SessionMode::Inspect.uses_stub());
        assert!(SessionMode::Flash.uses_stub());
    }

    #[test]
    fn connection_mode_requires_expected_chip_only_for_flashing() {
        let mut inspection = inert_session(SessionMode::Inspect);
        assert!(matches!(
            inspection.connect(ChipSelection::Expected(Chip::Esp32s3)),
            Err(SessionError::Connect(message)) if message.contains("autodetect")
        ));

        let mut flash = inert_session(SessionMode::Flash);
        assert!(matches!(
            flash.connect(ChipSelection::Detect),
            Err(SessionError::Connect(message)) if message.contains("expected-chip")
        ));
    }

    #[test]
    fn espflash_errors_keep_verification_device_loss_and_cancellation_distinct() {
        assert!(matches!(
            map_write_error(espflash::Error::VerifyFailed),
            SessionError::Verify(_)
        ));
        assert!(matches!(
            map_write_error(espflash::Error::IncorrectResponse),
            SessionError::DeviceLost(_)
        ));
        assert!(matches!(
            map_write_error(espflash::Error::FlashConnect),
            SessionError::Write(_)
        ));
        assert!(matches!(
            map_write_error(espflash::Error::Cancelled),
            SessionError::Cancelled
        ));
    }
}
